//! juliex is a concurrent executor for Rust futures. It is implemented as a
//! threadpool executor using a single, shared queue. Algorithmically, it is very
//! similar to the Threadpool executor provided by the futures crate. The main
//! difference is that juliex uses a crossbeam channel and performs a single
//! allocation per spawned future, whereas the futures Threadpool uses std
//! concurrency primitives and multiple allocations.
//!
//! Similar to [romio][romio] - an IO reactor - juliex currently provides no user
//! configuration. It exposes only the simplest API possible.
//!
//! [romio]: https://github.com/withoutboats/romio
//!
//! ## Example
//! ```rust,no_run
//! #![feature(async_await, await_macro, futures_api)]
//!
//! use std::io;
//!
//! use futures::StreamExt;
//! use futures::executor;
//! use futures::io::AsyncReadExt;
//!
//! use romio::{TcpListener, TcpStream};
//!
//! fn main() -> io::Result<()> {
//!     executor::block_on(async {
//!         let mut listener = TcpListener::bind(&"127.0.0.1:7878".parse().unwrap())?;
//!         let mut incoming = listener.incoming();
//!
//!         println!("Listening on 127.0.0.1:7878");
//!
//!         while let Some(stream) = await!(incoming.next()) {
//!             let stream = stream?;
//!             let addr = stream.peer_addr()?;
//!
//!             juliex::spawn(async move {
//!                 println!("Accepting stream from: {}", addr);
//!
//!                 await!(echo_on(stream)).unwrap();
//!
//!                 println!("Closing stream from: {}", addr);
//!             });
//!         }
//!
//!         Ok(())
//!     })
//! }
//!
//! async fn echo_on(stream: TcpStream) -> io::Result<()> {
//!     let (mut reader, mut writer) = stream.split();
//!     await!(reader.copy_into(&mut writer))?;
//!     Ok(())
//! }
//! ```

#![feature(arbitrary_self_types, futures_api)]

use std::cell::{RefCell, UnsafeCell};
use std::fmt;
use std::future::Future;
use std::mem::{forget, ManuallyDrop};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering::SeqCst},
    Arc, Weak,
};
use std::task::{Poll, RawWaker, RawWakerVTable, Waker};
use std::thread;

use crossbeam::channel;

lazy_static::lazy_static! {
    static ref THREAD_POOL: ThreadPool = ThreadPool::new();
}

thread_local! {
    static QUEUE: RefCell<Weak<TaskQueue>> = RefCell::new(Weak::new());
}

/// A threadpool that futures can be spawned on.
///
/// This is useful when you want to perform some setup logic around the
/// threadpool. If you don't need to setup extra logic, it's recommended to use
/// `juliex::spawn()` directly.
pub struct ThreadPool {
    queue: Arc<TaskQueue>,
}

impl ThreadPool {
    /// Create a new threadpool instance.
    pub fn new() -> Self {
        Self::with_setup(|| ())
    }

    /// Create a new instance with a method that's called for every thread
    /// that's spawned.
    pub fn with_setup<F>(f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        let (tx, rx) = channel::unbounded();
        let queue = Arc::new(TaskQueue { tx, rx });
        let max_cpus = num_cpus::get() * 2;
        for _ in 0..max_cpus {
            let f = f.clone();
            let rx = queue.rx.clone();
            let queue = Arc::downgrade(&queue);
            thread::spawn(move || {
                QUEUE.with(|q| *q.borrow_mut() = queue.clone());
                f();
                for task in rx {
                    unsafe { task.poll() }
                }
            });
        }
        ThreadPool { queue }
    }

    /// Spawn a new future on the threadpool.
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.queue
            .tx
            .send(Task::new(future, self.queue.clone()))
            .unwrap();
    }
}

/// Spawn a task on the threadpool.
///
/// ## Example
/// ```rust,no_run
/// #![feature(async_await, await_macro, futures_api)]
/// use std::thread;
/// use futures::executor;
///
/// fn main() {
///     for _ in 0..10 {
///         juliex::spawn(async move {
///             let id = thread::current().id();
///             println!("Running on thread {:?}", id);
///         })
///     }
/// }
/// ```
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    QUEUE.with(|q| {
        if let Some(q) = q.borrow().upgrade() {
            q.tx.send(Task::new(future, q.clone())).unwrap();
        } else {
            THREAD_POOL.spawn(future);
        }
    });
}

struct TaskQueue {
    tx: channel::Sender<Task>,
    rx: channel::Receiver<Task>,
}

#[derive(Clone, Debug)]
#[repr(transparent)]
struct Task(Arc<AtomicFuture>);

struct AtomicFuture {
    queue: Arc<TaskQueue>,
    status: AtomicUsize,
    future: UnsafeCell<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
}

impl fmt::Debug for AtomicFuture {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "AtomicFuture".fmt(f)
    }
}

unsafe impl Send for AtomicFuture {}
unsafe impl Sync for AtomicFuture {}

const WAITING: usize = 0; // --> POLLING
const POLLING: usize = 1; // --> WAITING, REPOLL, or COMPLETE
const REPOLL: usize = 2; // --> POLLING
const COMPLETE: usize = 3; // No transitions out

impl Task {
    fn new<F: Future<Output = ()> + Send + 'static>(future: F, queue: Arc<TaskQueue>) -> Task {
        let future: Arc<AtomicFuture> = Arc::new(AtomicFuture {
            queue,
            status: AtomicUsize::new(WAITING),
            future: UnsafeCell::new(Box::pin(future)),
        });
        let future: *const AtomicFuture = Arc::into_raw(future) as *const AtomicFuture;
        unsafe { task(future) }
    }

    unsafe fn poll(self) {
        self.0.status.store(POLLING, SeqCst);
        let waker = ManuallyDrop::new(waker(&*self.0));
        loop {
            if let Poll::Ready(_) = (&mut *self.0.future.get()).as_mut().poll(&waker) {
                break self.0.status.store(COMPLETE, SeqCst);
            }
            match self
                .0
                .status
                .compare_exchange(POLLING, WAITING, SeqCst, SeqCst)
            {
                Ok(_) => break,
                Err(_) => self.0.status.store(POLLING, SeqCst),
            }
        }
    }
}

unsafe fn waker(task: *const AtomicFuture) -> Waker {
    Waker::new_unchecked(RawWaker::new(
        task as *const (),
        &RawWakerVTable {
            clone: clone_raw,
            wake: wake_raw,
            drop: drop_raw,
        },
    ))
}

unsafe fn clone_raw(this: *const ()) -> RawWaker {
    let task = clone_task(this as *const AtomicFuture);
    RawWaker::new(
        Arc::into_raw(task.0) as *const (),
        &RawWakerVTable {
            clone: clone_raw,
            wake: wake_raw,
            drop: drop_raw,
        },
    )
}

unsafe fn drop_raw(this: *const ()) {
    drop(task(this as *const AtomicFuture))
}

unsafe fn wake_raw(this: *const ()) {
    let task = ManuallyDrop::new(task(this as *const AtomicFuture));
    let mut status = task.0.status.load(SeqCst);
    loop {
        match status {
            WAITING => {
                match task
                    .0
                    .status
                    .compare_exchange(WAITING, POLLING, SeqCst, SeqCst)
                {
                    Ok(_) => {
                        task.0.queue.tx.send(clone_task(&*task.0)).unwrap();
                        break;
                    }
                    Err(cur) => status = cur,
                }
            }
            POLLING => {
                match task
                    .0
                    .status
                    .compare_exchange(POLLING, REPOLL, SeqCst, SeqCst)
                {
                    Ok(_) => break,
                    Err(cur) => status = cur,
                }
            }
            _ => break,
        }
    }
}

unsafe fn task(future: *const AtomicFuture) -> Task {
    Task(Arc::from_raw(future))
}

unsafe fn clone_task(future: *const AtomicFuture) -> Task {
    let task = task(future);
    forget(task.clone());
    task
}

//! juliex is a concurrent executor for Rust futures. It is implemented as a
//! threadpool executor using a single, shared queue. Algorithmically, it is very
//! similar to the Threadpool executor provided by the futures crate. The main
//! difference is that juliex uses a crossbeam channel and performs a single
//! allocation per spawned future, whereas the futures Threadpool uses std
//! concurrency primitives and multiple allocations.
//!
//! Similar to [romio][romio] - an IO reactor - juliex currently provides no user
//! configuration. It exposes the most minimal API possible.
//!
//! [romio]: https://github.com/withoutboats/romio
//!
//! ## Example
//! ```rust,no_run
//! #![feature(async_await)]
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
//!         while let Some(stream) = incoming.next().await {
//!             let stream = stream?;
//!             let addr = stream.peer_addr()?;
//!
//!             juliex::spawn(async move {
//!                 println!("Accepting stream from: {}", addr);
//!
//!                 echo_on(stream).await.unwrap();
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
//!     reader.copy_into(&mut writer).await?;
//!     Ok(())
//! }
//! ```

#![cfg_attr(test, feature(async_await))]

use std::cell::RefCell;
use std::future::Future;
use std::sync::{Arc, Weak};
use std::thread;
use crossbeam::channel;
use futures::future::BoxFuture;

#[cfg(test)]
mod tests;

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
    #[inline]
    pub fn new() -> Self {
        Self::with_setup(|| ())
    }

    /// Create a new instance with a method that's called for every thread
    /// that's spawned.
    #[inline]
    pub fn with_setup<F>(f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        let (tx, rx) = channel::unbounded();
        let queue = Arc::new(tx);
        let max_cpus = num_cpus::get() * 2;
        for _ in 0..max_cpus {
            let f = f.clone();
            let rx = rx.clone();
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
    #[inline]
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.queue
            .send(Task::new(future, self.queue.clone()))
            .unwrap();
    }

    /// Spawn a boxed future on the threadpool.
    #[inline]
    pub fn spawn_boxed(&self, future: BoxFuture<'static, ()>) {
        self.queue
            .send(Task::new_boxed(future, self.queue.clone()))
            .unwrap();
    }
}

/// Spawn a task on the threadpool.
///
/// ## Example
/// ```rust,ignore
/// #![feature(async_await)]
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
#[inline]
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    QUEUE.with(|q| {
        if let Some(q) = q.borrow().upgrade() {
            q.send(Task::new(future, q.clone())).unwrap();
        } else {
            THREAD_POOL.spawn(future);
        }
    });
}

pub mod task;
use task::{Task, TaskQueue};

pub mod scoped;

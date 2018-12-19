#![feature(pin, arbitrary_self_types, futures_api)]
use std::cell::UnsafeCell;
use std::future::Future;
use std::mem::{transmute, forget};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering::SeqCst}};
use std::task::{Poll, LocalWaker, Waker, UnsafeWake};
use std::thread;

use crossbeam::channel;

lazy_static::lazy_static! {
    static ref QUEUE: Arc<TaskQueue> = {
        let (tx, rx) = channel::unbounded();
        let queue = Arc::new(TaskQueue { tx, rx});
        for _ in 0..num_cpus::get() {
            let queue = queue.clone();
            thread::spawn(move || {
                for task in &queue.rx {
                    unsafe { task.poll() }
                }
            });
        }
        queue
    };
}

pub fn spawn<F>(future: F) where F: Future<Output = ()> + Send + 'static {
    let vtable = unsafe {
        transmute::<&(dyn Future<Output = ()> + Send + 'static), Obj>(&future).vtable
    };
    let future: Arc<AtomicFuture<F>> = Arc::new(AtomicFuture {
        status: AtomicUsize::new(WAITING),
        vtable: vtable,
        future: UnsafeCell::new(future)
    });
    let future: *const AtomicFuture = Arc::into_raw(future) as *const AtomicFuture;
    let task = unsafe { task(future) };
    QUEUE.tx.send(task).unwrap();
}

struct TaskQueue {
    tx: channel::Sender<Task>,
    rx: channel::Receiver<Task>,
}

#[derive(Clone, Debug)]
#[repr(transparent)]
struct Task(Arc<AtomicFuture>);

#[derive(Debug)]
struct AtomicFuture<F: ?Sized = ()> {
    status: AtomicUsize,
    vtable: *const (),
    future: UnsafeCell<F>,
}

#[repr(C)] struct Obj { data: *mut (), vtable: *const () }

unsafe impl Send for AtomicFuture { }
unsafe impl Sync for AtomicFuture { }

const WAITING: usize = 0;       // --> POLLING
const POLLING: usize = 1;       // --> WAITING, REPOLL, or COMPLETE
const REPOLL: usize = 2;        // --> POLLING
const COMPLETE: usize = 3;      // No transitions out

impl Task {
    unsafe fn poll(self) {
        self.0.status.store(POLLING, SeqCst);
        let mut future: Pin<&mut (dyn Future<Output = ()> + Send + 'static)> = {
            Pin::new_unchecked(transmute(Obj {
                data: self.0.future.get(),
                vtable: self.0.vtable,
            }))
        };
        let waker = waker(&*self.0);
        loop {
            if let Poll::Ready(_) = future.as_mut().poll(&waker) {
                break self.0.status.store(COMPLETE, SeqCst);
            }
            match self.0.status.compare_exchange(POLLING, WAITING, SeqCst, SeqCst) {
                Ok(_)   => break,
                Err(_)  => self.0.status.store(POLLING, SeqCst),
            }
        }
        forget(waker)
    }
}

unsafe impl UnsafeWake for AtomicFuture {
    unsafe fn wake(&self) {
        let mut status = self.status.load(SeqCst);
        loop {
            match status {
                WAITING => {
                    match self.status.compare_exchange(WAITING, POLLING, SeqCst, SeqCst) {
                        Ok(_) => {
                            return QUEUE.tx.send(clone_task(self)).unwrap()
                        }
                        Err(cur) => status = cur,
                    }
                }
                POLLING => {
                    if let Err(cur) = self.status.compare_exchange(POLLING, REPOLL, SeqCst, SeqCst) {
                        status = cur;
                    }
                }
                _ => break,
            }
        }
    }

    unsafe fn clone_raw(&self) -> Waker {
        Waker::new(NonNull::new_unchecked(Arc::into_raw(clone_task(self).0) as *mut AtomicFuture))
    }

    unsafe fn drop_raw(&self) {
        drop(task(self))
    }
}

unsafe fn waker(task: *const AtomicFuture) -> LocalWaker {
    LocalWaker::new(NonNull::new_unchecked(task as *mut AtomicFuture))
}

unsafe fn task(future: *const AtomicFuture) -> Task {
    Task(Arc::from_raw(future))
}

unsafe fn clone_task(future: *const AtomicFuture) -> Task {
    let task = task(future);
    forget(task.clone());
    task
}

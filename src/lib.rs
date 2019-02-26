#![feature(futures_api)]

#[cfg(test)]
mod tests;
use std::cell::UnsafeCell;
use std::future::Future;
use std::mem::{ManuallyDrop, transmute, forget};
use std::pin::Pin;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering::SeqCst}};
use std::task::{Poll, Waker, RawWaker, RawWakerVTable};
use std::thread;

use crossbeam::channel;

lazy_static::lazy_static! {
    static ref QUEUE: Arc<TaskQueue> = {
        let (tx, rx) = channel::unbounded();
        let queue = Arc::new(TaskQueue { tx, rx});
        let max_cpus = num_cpus::get() * 2;
        for _ in 0..max_cpus {
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
    QUEUE.tx.send(Task::new(future)).unwrap();
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
    future: ManuallyDrop<UnsafeCell<F>>,
}

impl<F: ?Sized> Drop for AtomicFuture<F> {
    fn drop(&mut self) {
        unsafe {
            let future: &mut ManuallyDrop<dyn Future<Output = ()> + Send + 'static> = {
                transmute(Obj {
                    data: self.future.get() as *mut (),
                    vtable: self.vtable,
                })
            };
            ManuallyDrop::drop(future);
        }
    }
}

#[repr(C)] struct Obj { data: *mut (), vtable: *const () }

unsafe impl Send for AtomicFuture { }
unsafe impl Sync for AtomicFuture { }

const WAITING: usize = 0;       // --> POLLING
const POLLING: usize = 1;       // --> WAITING, REPOLL, or COMPLETE
const REPOLL: usize = 2;        // --> POLLING
const COMPLETE: usize = 3;      // No transitions out

impl Task {
    fn new<F: Future<Output = ()> + Send + 'static>(future: F) -> Task {
        let vtable = unsafe {
            transmute::<&(dyn Future<Output = ()> + Send + 'static), Obj>(&future).vtable
        };
        let future: Arc<AtomicFuture<F>> = Arc::new(AtomicFuture {
            status: AtomicUsize::new(WAITING),
            vtable: vtable,
            future: ManuallyDrop::new(UnsafeCell::new(future))
        });
        let future: *const AtomicFuture = Arc::into_raw(future) as *const AtomicFuture;
        unsafe { task(future) }
    }

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

unsafe fn waker(task: *const AtomicFuture) -> Waker {
    Waker::new_unchecked(RawWaker::new(task as *const (), &RawWakerVTable {
        clone: clone_raw,
        wake: wake_raw,
        drop: drop_raw,
    }))
}

unsafe fn clone_raw(this: *const ()) -> RawWaker {
    let task = clone_task(this as *const AtomicFuture);
    RawWaker::new(Arc::into_raw(task.0) as *const (), &RawWakerVTable {
        clone: clone_raw,
        wake: wake_raw,
        drop: drop_raw,
    })
}

unsafe fn drop_raw(this: *const ()) {
    drop(task(this as *const AtomicFuture))
}

unsafe fn wake_raw(this: *const ()) {
    let task = task(this as *const AtomicFuture);
    let mut status = task.0.status.load(SeqCst);
    loop {
        match status {
            WAITING => {
                match task.0.status.compare_exchange(WAITING, POLLING, SeqCst, SeqCst) {
                    Ok(_) => {
                        return QUEUE.tx.send(clone_task(Arc::into_raw(task.0))).unwrap()
                    }
                    Err(cur) => status = cur,
                }
            }
            POLLING => {
                if let Err(cur) = task.0.status.compare_exchange(POLLING, REPOLL, SeqCst, SeqCst) {
                    status = cur;
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

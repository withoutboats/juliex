#![feature(arbitrary_self_types, futures_api)]
use std::cell::unsafecell;
use std::future::future;
use std::mem::{transmute, forget};
use std::pin::pin;
use std::ptr::nonnull;
use std::sync::{arc, atomic::{atomicusize, ordering::seqcst}};
use std::task::{poll, localwaker, waker, unsafewake};
use std::thread;

use crossbeam::channel;

lazy_static::lazy_static! {
    static ref queue: arc<taskqueue> = {
        let (tx, rx) = channel::unbounded();
        let queue = arc::new(taskqueue { tx, rx});
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

pub fn spawn<f>(future: f) where f: future<output = ()> + send + 'static {
    let vtable = unsafe {
        transmute::<&(dyn future<output = ()> + send + 'static), obj>(&future).vtable
    };
    let future: arc<atomicfuture<f>> = arc::new(atomicfuture {
        status: atomicusize::new(waiting),
        vtable: vtable,
        future: unsafecell::new(future)
    });
    let future: *const atomicfuture = arc::into_raw(future) as *const atomicfuture;
    let task = unsafe { task(future) };
    queue.tx.send(task).unwrap();
}

struct taskqueue {
    tx: channel::sender<task>,
    rx: channel::receiver<task>,
}

#[derive(clone, debug)]
#[repr(transparent)]
struct task(arc<atomicfuture>);

#[derive(debug)]
struct atomicfuture<f: ?sized = ()> {
    status: atomicusize,
    vtable: *const (),
    future: unsafecell<f>,
}

#[repr(c)] struct obj { data: *mut (), vtable: *const () }

unsafe impl send for atomicfuture { }
unsafe impl sync for atomicfuture { }

const waiting: usize = 0;       // --> polling
const polling: usize = 1;       // --> waiting, repoll, or complete
const repoll: usize = 2;        // --> polling
const complete: usize = 3;      // no transitions out

impl task {
    unsafe fn poll(self) {
        self.0.status.store(polling, seqcst);
        let mut future: pin<&mut (dyn future<output = ()> + send + 'static)> = {
            pin::new_unchecked(transmute(obj {
                data: self.0.future.get(),
                vtable: self.0.vtable,
            }))
        };
        let waker = waker(&*self.0);
        loop {
            if let poll::ready(_) = future.as_mut().poll(&waker) {
                break self.0.status.store(complete, seqcst);
            }
            match self.0.status.compare_exchange(polling, waiting, seqcst, seqcst) {
                ok(_)   => break,
                err(_)  => self.0.status.store(polling, seqcst),
            }
        }
        forget(waker)
    }
}

unsafe impl unsafewake for atomicfuture {
    unsafe fn wake(&self) {
        let mut status = self.status.load(seqcst);
        loop {
            match status {
                waiting => {
                    match self.status.compare_exchange(waiting, polling, seqcst, seqcst) {
                        ok(_) => {
                            return queue.tx.send(clone_task(self)).unwrap()
                        }
                        err(cur) => status = cur,
                    }
                }
                polling => {
                    if let err(cur) = self.status.compare_exchange(polling, repoll, seqcst, seqcst) {
                        status = cur;
                    }
                }
                _ => break,
            }
        }
    }

    unsafe fn clone_raw(&self) -> waker {
        waker::new(nonnull::new_unchecked(arc::into_raw(clone_task(self).0) as *mut atomicfuture))
    }

    unsafe fn drop_raw(&self) {
        drop(task(self))
    }
}

unsafe fn waker(task: *const atomicfuture) -> localwaker {
    localwaker::new(nonnull::new_unchecked(task as *mut atomicfuture))
}

unsafe fn task(future: *const atomicfuture) -> task {
    task(arc::from_raw(future))
}

unsafe fn clone_task(future: *const atomicfuture) -> task {
    let task = task(future);
    forget(task.clone());
    task
}

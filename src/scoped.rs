//! Pretty much exactly the default executor, except
//! integrates with `crossbeam::scope` and executes non
//! `'static` futures.
//!
//! ## Example
//! ```rust,no_run
//! #![feature(async_await)]
//! fn main() {
//!     let not_copy = String::from("hello world!");
//!     crossbeam::scope(|scope| {
//!
//!         // Compare with this (does not compile)
//!         // use futures::executor::ThreadPool;
//!         // use futures::task::SpawnExt;
//!         // let tp = ThreadPool::new().expect("error creating thread pool");
//!
//!         let tp = juliex::scoped::Pool::new(&scope);
//!         let fut = || async {
//!             println!("{} from future!", not_copy);
//!         };
//!         tp.spawn(fut());
//!
//!     }).expect("threads panicked");
//! }
//! ```
use super::task::{Task, TaskQueue};
use std::{sync::Arc, marker::PhantomData, mem::drop};
#[derive(Clone)]
pub struct Pool<'a> {
    queue: Arc<TaskQueue>,
    waiter: WaitGroup,
    _marker: PhantomData<fn(&'a ())>
}

use crossbeam::{channel, thread::Scope, sync::WaitGroup};
use futures::future::*;
impl<'a> Pool<'a> {
    /// Spawn a future on the threadpool.
    #[inline] pub fn spawn<F: Future<Output=()> + Send + 'a>(&self, f: F) {
        self.spawn_boxed(f.boxed())
    }

    /// Spawn a boxed future on the threadpool.
    /// # Safety
    #[inline] pub fn spawn_boxed(&self, future: BoxFuture<'a, ()>) {
        let task;

        // See #safety above
        unsafe {
            task = Task::new_boxed(std::mem::transmute(future), self.queue.clone());
        }
        self.queue
            .send(task)
            .unwrap();
    }

    /// Create a new threadpool with default settings
    pub fn new(scope: &Scope<'a>) -> Self {
        Self::with_setup(scope, 2*num_cpus::get(), ||{})
    }

    /// Create a new threadpool with specified workers, and
    /// initialization function.
    pub fn with_setup<F>(scope: &Scope<'a>, num_workers: usize, f: F) -> Self
    where F: Fn() + Send + Sync + 'a {
        let f = Arc::new(f);
        let (tx, rx) = channel::unbounded();
        let waiter = WaitGroup::new();

        for _ in 0..num_workers {
            let rx = rx.clone();
            let f = f.clone();
            let waiter = waiter.clone();
            scope.spawn(move |_| { f(); worker_loop(rx); drop(waiter); });
        }

        Pool { queue: Arc::new(tx), waiter,
               _marker: PhantomData }
    }

    /// Return the wait group (``)
    pub fn finish(self) -> WaitGroup {
        self.waiter
    }
}


use crossbeam::channel::Receiver;
#[inline] fn worker_loop(rx: Receiver<Task>) {
    for task in rx {
        unsafe { task.poll() }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scoped_pool() {
        let not_copy = String::from("hello world!");
        let not_copy = &not_copy; // ensure no move
        crossbeam::scope(|scope| {
            let tp = super::Pool::new(&scope);
            let fut = || async move {
                assert_eq!(not_copy, "hello world!");
            };
            tp.spawn(fut());

        }).expect("threads panicked");
    }
}

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::*;
use std::sync::mpsc::*;

use super::*;

struct DropFuture(Sender<()>);

impl Future for DropFuture {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &Waker) -> Poll<()> {
        Poll::Ready(())
    }
}

impl Drop for DropFuture {
    fn drop(&mut self) {
        self.0.send(()).unwrap();
    }
}

#[test]
fn destructor_runs() {
    // Test that the destructor runs
    let (tx, rx) = channel();
    drop(Task::new(DropFuture(tx)));
    rx.try_recv().unwrap();

    // Test that the destructor doesn't run if we forget the task
    let (tx, rx) = channel();
    std::mem::forget(Task::new(DropFuture(tx)));
    assert!(rx.try_recv().is_err());
}

#[test]
fn with_init() {
    thread_local! {
        static FLAG: Cell<bool> = Cell::new(false);
    }

    // Test that the init function gets called.
    let pool = ThreadPool::with_init(|| FLAG.with(|f| f.set(true)));
    let (tx, rx) = channel();

    pool.spawn(async move {
        assert_eq!(FLAG.with(|f| f.get()), true);
        tx.send(()).unwrap();
    });

    rx.recv().unwrap();
}

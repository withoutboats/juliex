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
        self.0.send(());
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

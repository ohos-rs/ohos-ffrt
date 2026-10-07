use crate::lock::{Mutex, MutexGuard};
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

struct WatchState<T> {
    value: T,
    version: u64,
    senders: usize,
    receivers: usize,
    waiters: VecDeque<Waker>,
    closed_waiters: VecDeque<Waker>,
}

/// Creates a watch channel: a single-producer, multi-consumer channel where
/// each receiver observes the latest value.
pub fn channel<T>(init: T) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Mutex::new(WatchState {
        value: init,
        version: 0,
        senders: 1,
        receivers: 1,
        waiters: VecDeque::new(),
        closed_waiters: VecDeque::new(),
    }));

    (
        Sender {
            shared: shared.clone(),
        },
        Receiver { shared, seen: 0 },
    )
}

/// Error returned when a watch receiver fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "watch channel closed")
    }
}

impl std::error::Error for RecvError {}

/// Error returned when a watch value cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError<T>(pub T);

impl<T> SendError<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "watch channel has no receivers")
    }
}

impl<T: fmt::Debug> std::error::Error for SendError<T> {}

/// Tokio-compatible error namespace.
pub mod error {
    pub use super::{RecvError, SendError};
}

/// The producing end of a watch channel.
pub struct Sender<T> {
    shared: Arc<Mutex<WatchState<T>>>,
}

impl<T> Sender<T> {
    /// Creates a sender without an initial receiver.
    pub fn new(init: T) -> Self {
        Self {
            shared: Arc::new(Mutex::new(WatchState {
                value: init,
                version: 0,
                senders: 1,
                receivers: 0,
                waiters: VecDeque::new(),
                closed_waiters: VecDeque::new(),
            })),
        }
    }

    /// Sends a new value, waking all waiting receivers.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut state = self.shared.lock().unwrap();
        if state.receivers == 0 {
            return Err(SendError(value));
        }

        state.value = value;
        state.version = state.version.wrapping_add(1);
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
        Ok(())
    }

    /// Sends a new value and returns the previous value.
    pub fn send_replace(&self, value: T) -> T {
        let mut state = self.shared.lock().unwrap();
        let old = std::mem::replace(&mut state.value, value);
        state.version = state.version.wrapping_add(1);
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
        old
    }

    /// Applies `func` to the current value and notifies receivers.
    pub fn send_modify<F>(&self, func: F)
    where
        F: FnOnce(&mut T),
    {
        let mut state = self.shared.lock().unwrap();
        func(&mut state.value);
        state.version = state.version.wrapping_add(1);
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
    }

    /// Applies `func` to the current value and notifies receivers if it returns `true`.
    pub fn send_if_modified<F>(&self, func: F) -> bool
    where
        F: FnOnce(&mut T) -> bool,
    {
        let mut state = self.shared.lock().unwrap();
        if !func(&mut state.value) {
            return false;
        }
        state.version = state.version.wrapping_add(1);
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
        true
    }

    /// Sends a value even if no receivers are currently connected.
    pub fn broadcast(&self, value: T) {
        let mut state = self.shared.lock().unwrap();
        state.value = value;
        state.version = state.version.wrapping_add(1);
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
    }

    /// Borrows the current value.
    pub fn borrow(&self) -> Ref<'_, T> {
        let guard = self.shared.lock().unwrap();
        Ref {
            guard: Some(guard),
            has_changed: false,
        }
    }

    /// Creates a new receiver that initially considers the current value seen.
    pub fn subscribe(&self) -> Receiver<T> {
        let mut state = self.shared.lock().unwrap();
        state.receivers += 1;
        Receiver {
            shared: self.shared.clone(),
            seen: state.version,
        }
    }

    /// Returns the number of connected receivers.
    pub fn receiver_count(&self) -> usize {
        let state = self.shared.lock().unwrap();
        state.receivers
    }

    pub fn sender_count(&self) -> usize {
        self.shared.lock().unwrap().senders
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Returns `true` when all receivers have been dropped.
    pub fn is_closed(&self) -> bool {
        let state = self.shared.lock().unwrap();
        state.receivers == 0
    }

    /// Waits until all receivers have been dropped.
    pub fn closed(&self) -> Closed<'_, T> {
        Closed { sender: self }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.lock().unwrap().senders += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap();
        state.senders -= 1;
        if state.senders == 0 {
            while let Some(waker) = state.waiters.pop_front() {
                waker.wake();
            }
        }
    }
}

/// The receiving end of a watch channel.
pub struct Receiver<T> {
    shared: Arc<Mutex<WatchState<T>>>,
    seen: u64,
}

impl<T> Receiver<T> {
    /// Borrows the current value.
    pub fn borrow(&self) -> Ref<'_, T> {
        let guard = self.shared.lock().unwrap();
        let has_changed = guard.version != self.seen;
        Ref {
            guard: Some(guard),
            has_changed,
        }
    }

    /// Borrows the current value and marks it as seen.
    pub fn borrow_and_update(&mut self) -> Ref<'_, T> {
        let state = self.shared.lock().unwrap();
        let has_changed = state.version != self.seen;
        self.seen = state.version;
        Ref {
            guard: Some(state),
            has_changed,
        }
    }

    /// Returns `true` if a new value is available that has not been seen.
    pub fn has_changed(&self) -> Result<bool, RecvError> {
        let state = self.shared.lock().unwrap();
        if state.version != self.seen {
            Ok(true)
        } else if state.senders == 0 {
            Err(RecvError)
        } else {
            Ok(false)
        }
    }

    /// Marks the current value as seen.
    pub fn mark_seen(&mut self) {
        let state = self.shared.lock().unwrap();
        self.seen = state.version;
    }

    /// Marks the current value as unchanged.
    pub fn mark_unchanged(&mut self) {
        self.mark_seen();
    }

    /// Marks the current value as unseen, so `changed` returns immediately.
    pub fn mark_changed(&mut self) {
        let state = self.shared.lock().unwrap();
        self.seen = state.version.wrapping_sub(1);
    }

    /// Waits for a value that has not been seen yet.
    pub fn changed(&mut self) -> Changed<'_, T> {
        Changed { receiver: self }
    }

    /// Waits for a value that satisfies `predicate`.
    pub fn wait_for<F>(&mut self, predicate: F) -> WaitFor<'_, T, F>
    where
        F: FnMut(&T) -> bool,
    {
        WaitFor {
            receiver: self,
            predicate,
        }
    }

    /// Returns the version of the most recently observed value.
    pub fn borrow_and_update_version(&mut self) -> u64 {
        let state = self.shared.lock().unwrap();
        self.seen = state.version;
        state.version
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let mut state = self.shared.lock().unwrap();
        state.receivers += 1;
        Receiver {
            shared: self.shared.clone(),
            seen: self.seen,
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap();
        state.receivers -= 1;
        if state.receivers == 0 {
            while let Some(waker) = state.closed_waiters.pop_front() {
                waker.wake();
            }
        }
    }
}

/// Future returned by [`Receiver::changed`].
#[must_use = "futures do nothing unless polled"]
pub struct Changed<'a, T> {
    receiver: &'a mut Receiver<T>,
}

impl<T> Future for Changed<'_, T> {
    type Output = Result<(), RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.receiver.shared.lock().unwrap();

        if state.version != this.receiver.seen {
            this.receiver.seen = state.version;
            return Poll::Ready(Ok(()));
        }

        if state.senders == 0 {
            return Poll::Ready(Err(RecvError));
        }

        state.waiters.push_back(cx.waker().clone());
        Poll::Pending
    }
}

/// Future returned by [`Receiver::wait_for`].
#[must_use = "futures do nothing unless polled"]
pub struct WaitFor<'a, T, F> {
    receiver: &'a mut Receiver<T>,
    predicate: F,
}

impl<'a, T, F> Future for WaitFor<'a, T, F>
where
    F: FnMut(&T) -> bool,
{
    type Output = Result<Ref<'a, T>, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `predicate` is pinned through `self` and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        let mut state = this.receiver.shared.lock().unwrap();

        let has_changed = state.version != this.receiver.seen;
        if (this.predicate)(&state.value) {
            this.receiver.seen = state.version;
            let state = unsafe {
                std::mem::transmute::<MutexGuard<'_, WatchState<T>>, MutexGuard<'a, WatchState<T>>>(
                    state,
                )
            };
            return Poll::Ready(Ok(Ref {
                guard: Some(state),
                has_changed,
            }));
        }

        if state.senders == 0 {
            return Poll::Ready(Err(RecvError));
        }

        state.waiters.push_back(cx.waker().clone());
        Poll::Pending
    }
}

/// Future returned by [`Sender::closed`].
#[must_use = "futures do nothing unless polled"]
pub struct Closed<'a, T> {
    sender: &'a Sender<T>,
}

impl<T> Future for Closed<'_, T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.sender.shared.lock().unwrap();

        if state.receivers == 0 {
            Poll::Ready(())
        } else {
            state.closed_waiters.push_back(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// A guard that keeps the watch state locked while borrowing a value.
pub struct Ref<'a, T> {
    guard: Option<MutexGuard<'a, WatchState<T>>>,
    has_changed: bool,
}

impl<T> Ref<'_, T> {
    pub fn has_changed(&self) -> bool {
        self.has_changed
    }
}

impl<T> Deref for Ref<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard.as_ref().expect("missing watch guard").value
    }
}

impl<T> Drop for Ref<'_, T> {
    fn drop(&mut self) {
        self.guard.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_and_changed() {
        let (tx, mut rx) = channel(1);
        tx.send(2).unwrap();
        let result = crate::Runtime::new().unwrap().block_on(async move {
            rx.changed().await.unwrap();
            assert_eq!(*rx.borrow(), 2);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn wait_for_predicate() {
        let (tx, mut rx) = channel(1);
        tx.send(5).unwrap();
        let result = crate::Runtime::new().unwrap().block_on(async move {
            rx.wait_for(|value| *value >= 5).await.unwrap();
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn send_modify_updates_value() {
        let (tx, mut rx) = channel(1);
        tx.send_modify(|value| *value += 1);
        assert_eq!(*rx.borrow_and_update(), 2);
    }
}

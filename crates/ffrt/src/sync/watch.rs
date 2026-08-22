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

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "watch channel has no receivers")
    }
}

impl<T: fmt::Debug> std::error::Error for SendError<T> {}

/// The producing end of a watch channel.
pub struct Sender<T> {
    shared: Arc<Mutex<WatchState<T>>>,
}

impl<T> Sender<T> {
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
    pub fn send_replace(&self, value: T) -> Result<T, SendError<T>> {
        let mut state = self.shared.lock().unwrap();
        if state.receivers == 0 {
            return Err(SendError(value));
        }

        let old = std::mem::replace(&mut state.value, value);
        state.version = state.version.wrapping_add(1);
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
        Ok(old)
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
        Ref { guard: Some(guard) }
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

    /// Returns `true` when all receivers have been dropped.
    pub fn is_closed(&self) -> bool {
        let state = self.shared.lock().unwrap();
        state.receivers == 0
    }

    /// Waits until all receivers have been dropped.
    pub fn closed(&mut self) -> Closed<'_, T> {
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
        Ref { guard: Some(guard) }
    }

    /// Borrows the current value and marks it as seen.
    pub fn borrow_and_update(&mut self) -> Ref<'_, T> {
        let state = self.shared.lock().unwrap();
        self.seen = state.version;
        Ref { guard: Some(state) }
    }

    /// Returns `true` if a new value is available that has not been seen.
    pub fn has_changed(&self) -> bool {
        let state = self.shared.lock().unwrap();
        state.version != self.seen
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
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let mut state = self.shared.lock().unwrap();
        state.receivers += 1;
        Receiver {
            shared: self.shared.clone(),
            seen: state.version,
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

impl<T, F> Future for WaitFor<'_, T, F>
where
    F: FnMut(&T) -> bool,
{
    type Output = Result<(), RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `predicate` is pinned through `self` and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        let mut state = this.receiver.shared.lock().unwrap();

        if (this.predicate)(&state.value) {
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

/// Future returned by [`Sender::closed`].
#[must_use = "futures do nothing unless polled"]
pub struct Closed<'a, T> {
    sender: &'a mut Sender<T>,
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

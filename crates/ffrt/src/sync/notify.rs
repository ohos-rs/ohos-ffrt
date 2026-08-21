use crate::lock::Mutex;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

struct NotifyState {
    permit: bool,
    waiters: VecDeque<Waker>,
}

/// A tokio-style asynchronous notification primitive.
///
/// `Notify` holds at most one permit for [`notify_one`](Notify::notify_one).
/// [`notify_waiters`](Notify::notify_waiters) wakes every currently waiting
/// task.
pub struct Notify {
    state: Mutex<NotifyState>,
}

impl Notify {
    /// Creates a new `Notify`, initialized without a permit.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(NotifyState {
                permit: false,
                waiters: VecDeque::new(),
            }),
        }
    }

    /// Notifies one waiting task, or stores a permit if no task is waiting.
    pub fn notify_one(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        } else {
            state.permit = true;
        }
    }

    /// Notifies all currently waiting tasks.
    pub fn notify_waiters(&self) {
        let mut state = self.state.lock().unwrap();
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
    }

    /// Returns a future that completes when a notification is received.
    pub fn notified(&self) -> Notified<'_> {
        Notified { notify: self }
    }
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

/// Future returned by [`Notify::notified`].
pub struct Notified<'a> {
    notify: &'a Notify,
}

impl Future for Notified<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.notify.state.lock().unwrap();

        if state.permit {
            state.permit = false;
            Poll::Ready(())
        } else {
            state.waiters.push_back(cx.waker().clone());
            Poll::Pending
        }
    }
}

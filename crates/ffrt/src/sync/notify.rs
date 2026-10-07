use crate::lock::LazyMutex;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};

const WAITING: u8 = 0;
const ONE: u8 = 1;
const ALL: u8 = 2;
const CONSUMED: u8 = 3;

struct Waiter {
    notified: Weak<AtomicU8>,
    waker: Waker,
}

struct NotifyState {
    permit: bool,
    generation: u64,
    waiters: VecDeque<Waiter>,
}

/// A Tokio-style asynchronous notification primitive.
pub struct Notify {
    state: LazyMutex<NotifyState>,
}

impl Notify {
    pub fn new() -> Self {
        Self::const_new()
    }

    pub const fn const_new() -> Self {
        Self {
            state: LazyMutex::new(NotifyState {
                permit: false,
                generation: 0,
                waiters: VecDeque::new(),
            }),
        }
    }

    pub fn notify_one(&self) {
        self.notify(false);
    }
    pub fn notify_last(&self) {
        self.notify(true);
    }

    fn notify(&self, last: bool) {
        let waker = notify_locked(&mut self.state.lock().unwrap(), last);
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Notifies every future created before this call, including unpolled ones.
    pub fn notify_waiters(&self) {
        let wakers = {
            let mut state = self.state.lock().unwrap();
            state.generation = state.generation.wrapping_add(1);
            state
                .waiters
                .drain(..)
                .filter_map(|waiter| {
                    let notified = waiter.notified.upgrade()?;
                    notified.store(ALL, Ordering::Release);
                    Some(waiter.waker)
                })
                .collect::<Vec<_>>()
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub fn notified(&self) -> Notified<'_> {
        Notified {
            notify: self,
            state: FutureState::new(self),
        }
    }

    pub fn notified_owned(self: Arc<Self>) -> OwnedNotified {
        let state = FutureState::new(&self);
        OwnedNotified {
            notify: self,
            state,
        }
    }

    fn poll_notified(&self, future: &mut FutureState, waker: Option<&Waker>) -> Poll<()> {
        let mut state = self.state.lock().unwrap();
        if future.notified.load(Ordering::Acquire) != WAITING
            || future.generation != state.generation
        {
            remove_waiter(&mut state, &future.notified);
            future.notified.store(CONSUMED, Ordering::Release);
            return Poll::Ready(());
        }
        if state.permit {
            state.permit = false;
            remove_waiter(&mut state, &future.notified);
            future.notified.store(CONSUMED, Ordering::Release);
            return Poll::Ready(());
        }
        if let Some(waiter) = state
            .waiters
            .iter_mut()
            .find(|waiter| waiter.notified.ptr_eq(&Arc::downgrade(&future.notified)))
        {
            if let Some(waker) = waker {
                waiter.waker.clone_from(waker);
            }
        } else {
            state.waiters.push_back(Waiter {
                notified: Arc::downgrade(&future.notified),
                waker: waker.unwrap_or(Waker::noop()).clone(),
            });
        }
        Poll::Pending
    }

    fn cancel(&self, future: &FutureState) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            remove_waiter(&mut state, &future.notified);
            // A selected notify_one permit belongs to the queue until consumed.
            if future.notified.load(Ordering::Acquire) == ONE {
                notify_locked(&mut state, false)
            } else {
                None
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

fn remove_waiter(state: &mut NotifyState, notified: &Arc<AtomicU8>) {
    let weak = Arc::downgrade(notified);
    state
        .waiters
        .retain(|waiter| !waiter.notified.ptr_eq(&weak));
}

fn notify_locked(state: &mut NotifyState, last: bool) -> Option<Waker> {
    while let Some(waiter) = if last {
        state.waiters.pop_back()
    } else {
        state.waiters.pop_front()
    } {
        if let Some(notified) = waiter.notified.upgrade() {
            notified.store(ONE, Ordering::Release);
            return Some(waiter.waker);
        }
    }
    state.permit = true;
    None
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

struct FutureState {
    notified: Arc<AtomicU8>,
    generation: u64,
}

impl FutureState {
    fn new(notify: &Notify) -> Self {
        Self {
            notified: Arc::new(AtomicU8::new(WAITING)),
            generation: notify.state.lock().unwrap().generation,
        }
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct Notified<'a> {
    notify: &'a Notify,
    state: FutureState,
}

impl Future for Notified<'_> {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        this.notify.poll_notified(&mut this.state, Some(cx.waker()))
    }
}

impl Notified<'_> {
    pub fn enable(mut self: Pin<&mut Self>) -> bool {
        let this = &mut *self;
        this.notify.poll_notified(&mut this.state, None).is_ready()
    }
}

impl Drop for Notified<'_> {
    fn drop(&mut self) {
        self.notify.cancel(&self.state);
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct OwnedNotified {
    notify: Arc<Notify>,
    state: FutureState,
}

impl Future for OwnedNotified {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        this.notify.poll_notified(&mut this.state, Some(cx.waker()))
    }
}

impl OwnedNotified {
    pub fn enable(mut self: Pin<&mut Self>) -> bool {
        let this = &mut *self;
        this.notify.poll_notified(&mut this.state, None).is_ready()
    }
}

impl Drop for OwnedNotified {
    fn drop(&mut self) {
        self.notify.cancel(&self.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_one_consumed_by_notified() {
        let notify = Notify::new();
        notify.notify_one();
        crate::Runtime::new().unwrap().block_on(notify.notified());
    }

    #[test]
    fn notify_waiters_wakes_registered_tasks() {
        let notify = Arc::new(Notify::new());
        let waiter = notify.clone().notified_owned();
        let notify_task = notify.clone();
        crate::Runtime::new().unwrap().block_on(async move {
            let handle = crate::spawn(waiter);
            crate::task::yield_now().await;
            notify_task.notify_waiters();
            handle.await.unwrap();
        });
    }
}

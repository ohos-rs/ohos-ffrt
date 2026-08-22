use crate::lock::LazyMutex;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};

struct Waiter {
    notified: Weak<AtomicBool>,
    waker: Waker,
}

struct NotifyState {
    permit: bool,
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

    /// Creates a notification primitive that can be used in a static.
    pub const fn const_new() -> Self {
        Self {
            state: LazyMutex::new(NotifyState {
                permit: false,
                waiters: VecDeque::new(),
            }),
        }
    }

    /// Notifies the oldest live waiter, or stores one permit.
    pub fn notify_one(&self) {
        self.notify(false);
    }

    /// Notifies the newest live waiter, or stores one permit.
    pub fn notify_last(&self) {
        self.notify(true);
    }

    fn notify(&self, last: bool) {
        let mut state = self.state.lock().unwrap();
        loop {
            let waiter = if last {
                state.waiters.pop_back()
            } else {
                state.waiters.pop_front()
            };
            let Some(waiter) = waiter else {
                state.permit = true;
                return;
            };
            if let Some(notified) = waiter.notified.upgrade() {
                notified.store(true, Ordering::Release);
                waiter.waker.wake();
                return;
            }
        }
    }

    /// Notifies every waiter that has already registered.
    pub fn notify_waiters(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.waiters.drain(..).collect::<Vec<_>>()
        };
        for waiter in waiters {
            if let Some(notified) = waiter.notified.upgrade() {
                notified.store(true, Ordering::Release);
                waiter.waker.wake();
            }
        }
    }

    pub fn notified(&self) -> Notified<'_> {
        Notified {
            notify: self,
            notified: Arc::new(AtomicBool::new(false)),
            registered: false,
        }
    }

    pub fn notified_owned(self: Arc<Self>) -> OwnedNotified {
        OwnedNotified {
            notify: self,
            notified: Arc::new(AtomicBool::new(false)),
            registered: false,
        }
    }

    fn poll_notified(
        &self,
        notified: &Arc<AtomicBool>,
        registered: &mut bool,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        if notified.load(Ordering::Acquire) {
            return Poll::Ready(());
        }

        let mut state = self.state.lock().unwrap();
        if state.permit {
            state.permit = false;
            return Poll::Ready(());
        }

        if *registered {
            if let Some(waiter) = state.waiters.iter_mut().find(|waiter| {
                waiter
                    .notified
                    .upgrade()
                    .is_some_and(|queued| Arc::ptr_eq(&queued, notified))
            }) {
                if !waiter.waker.will_wake(cx.waker()) {
                    waiter.waker = cx.waker().clone();
                }
            }
        } else {
            state.waiters.push_back(Waiter {
                notified: Arc::downgrade(notified),
                waker: cx.waker().clone(),
            });
            *registered = true;
        }

        if notified.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct Notified<'a> {
    notify: &'a Notify,
    notified: Arc<AtomicBool>,
    registered: bool,
}

impl Future for Notified<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        this.notify
            .poll_notified(&this.notified, &mut this.registered, cx)
    }
}

impl Notified<'_> {
    /// Registers this future before it is polled, preventing lost notifications in `select!` loops.
    pub fn enable(mut self: Pin<&mut Self>) -> bool {
        let this = &mut *self;
        enable_notified(this.notify, &this.notified, &mut this.registered)
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct OwnedNotified {
    notify: Arc<Notify>,
    notified: Arc<AtomicBool>,
    registered: bool,
}

impl Future for OwnedNotified {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        this.notify
            .poll_notified(&this.notified, &mut this.registered, cx)
    }
}

impl OwnedNotified {
    /// Registers this future before it is polled.
    pub fn enable(mut self: Pin<&mut Self>) -> bool {
        let this = &mut *self;
        enable_notified(&this.notify, &this.notified, &mut this.registered)
    }
}

fn enable_notified(notify: &Notify, notified: &Arc<AtomicBool>, registered: &mut bool) -> bool {
    if notified.load(Ordering::Acquire) {
        return true;
    }
    let mut state = notify.state.lock().unwrap();
    if state.permit {
        state.permit = false;
        notified.store(true, Ordering::Release);
        return true;
    }
    if !*registered {
        state.waiters.push_back(Waiter {
            notified: Arc::downgrade(notified),
            waker: Waker::noop().clone(),
        });
        *registered = true;
    }
    false
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
            let handle = crate::spawn(async move { waiter.await });
            crate::task::yield_now().await;
            notify_task.notify_waiters();
            handle.await.unwrap();
        });
    }
}

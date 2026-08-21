use crate::lock::Mutex;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

struct SemaphoreState {
    permits: usize,
    closed: bool,
    waiters: VecDeque<SemaphoreWaiter>,
}

struct SemaphoreWaiter {
    permits: usize,
    waker: Waker,
}

/// A tokio-style counting semaphore.
pub struct Semaphore {
    state: Mutex<SemaphoreState>,
}

impl Semaphore {
    /// Creates a semaphore with the given initial number of permits.
    pub fn new(permits: usize) -> Self {
        Self {
            state: Mutex::new(SemaphoreState {
                permits,
                closed: false,
                waiters: VecDeque::new(),
            }),
        }
    }

    /// Returns the number of currently available permits.
    pub fn available_permits(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.permits
    }

    /// Adds `n` permits to the semaphore.
    pub fn add_permits(&self, n: usize) {
        let mut state = self.state.lock().unwrap();
        state.permits = state.permits.saturating_add(n);
        Self::wake_waiters(&mut state);
    }

    /// Reduces available permits by up to `n` and returns the number reduced.
    pub fn forget_permits(&self, n: usize) -> usize {
        let mut state = self.state.lock().unwrap();
        let reduced = state.permits.min(n);
        state.permits -= reduced;
        reduced
    }

    /// Closes the semaphore. Waiting acquisitions fail with an error.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        while let Some(waiter) = state.waiters.pop_front() {
            waiter.waker.wake();
        }
    }

    /// Returns `true` if the semaphore is closed.
    pub fn is_closed(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.closed
    }

    /// Acquires one permit.
    pub fn acquire(&self) -> Acquire<'_> {
        self.acquire_many(1)
    }

    /// Acquires `permits` permits.
    pub fn acquire_many(&self, permits: u32) -> Acquire<'_> {
        Acquire {
            semaphore: self,
            permits: permits as usize,
        }
    }

    /// Attempts to acquire one permit without waiting.
    pub fn try_acquire(&self) -> Result<SemaphorePermit<'_>, TryAcquireError> {
        self.try_acquire_many(1)
    }

    /// Attempts to acquire `permits` permits without waiting.
    pub fn try_acquire_many(&self, permits: u32) -> Result<SemaphorePermit<'_>, TryAcquireError> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(TryAcquireError::Closed);
        }
        let permits = permits as usize;
        if state.permits < permits {
            return Err(TryAcquireError::NoPermits);
        }
        state.permits -= permits;
        Ok(SemaphorePermit {
            semaphore: self,
            permits,
            forgotten: false,
        })
    }

    fn wake_waiters(state: &mut SemaphoreState) {
        loop {
            let permits = match state.waiters.front() {
                Some(waiter) => waiter.permits,
                None => return,
            };

            if state.permits < permits {
                return;
            }

            let waiter = state
                .waiters
                .pop_front()
                .expect("front waiter checked above");
            state.permits -= permits;
            waiter.waker.wake();
        }
    }
}

impl Default for Semaphore {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A permit returned from a successful [`Semaphore`] acquisition.
pub struct SemaphorePermit<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
    forgotten: bool,
}

impl SemaphorePermit<'_> {
    /// Forgets the permit without returning it to the semaphore.
    pub fn forget(mut self) {
        self.forgotten = true;
    }
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        if !self.forgotten {
            self.semaphore.add_permits(self.permits);
        }
    }
}

/// Future returned by [`Semaphore::acquire`] and [`Semaphore::acquire_many`].
#[must_use = "futures do nothing unless polled"]
pub struct Acquire<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
}

impl<'a> Future for Acquire<'a> {
    type Output = Result<SemaphorePermit<'a>, AcquireError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.semaphore.state.lock().unwrap();

        if state.closed {
            return Poll::Ready(Err(AcquireError::closed()));
        }

        if state.permits >= this.permits {
            state.permits -= this.permits;
            Semaphore::wake_waiters(&mut state);
            return Poll::Ready(Ok(SemaphorePermit {
                semaphore: this.semaphore,
                permits: this.permits,
                forgotten: false,
            }));
        }

        state.waiters.push_back(SemaphoreWaiter {
            permits: this.permits,
            waker: cx.waker().clone(),
        });
        Poll::Pending
    }
}

/// Error returned when a semaphore acquisition fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquireError(());

impl AcquireError {
    /// Returns `true` if the semaphore was closed.
    pub fn is_closed(&self) -> bool {
        true
    }

    pub(crate) fn closed() -> Self {
        Self(())
    }
}

impl fmt::Display for AcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "semaphore closed")
    }
}

impl std::error::Error for AcquireError {}

/// Error returned by the non-blocking semaphore acquisition methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryAcquireError {
    /// The semaphore is closed.
    Closed,
    /// Not enough permits are currently available.
    NoPermits,
}

impl fmt::Display for TryAcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryAcquireError::Closed => write!(f, "semaphore closed"),
            TryAcquireError::NoPermits => write!(f, "no permits available"),
        }
    }
}

impl std::error::Error for TryAcquireError {}

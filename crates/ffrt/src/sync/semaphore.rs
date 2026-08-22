use crate::lock::Mutex;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

struct SemaphoreState {
    permits: usize,
    closed: bool,
    next_waiter: u64,
    waiters: VecDeque<SemaphoreWaiter>,
}

struct SemaphoreWaiter {
    id: u64,
    permits: usize,
    waker: Waker,
}

/// A fair, tokio-style counting semaphore.
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
                next_waiter: 1,
                waiters: VecDeque::new(),
            }),
        }
    }

    /// Returns the number of currently available permits.
    pub fn available_permits(&self) -> usize {
        self.state.lock().unwrap().permits
    }

    /// Adds `n` permits to the semaphore.
    pub fn add_permits(&self, n: usize) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            state.permits = state.permits.saturating_add(n);
            Self::next_waker(&state)
        };
        if let Some(waker) = waker {
            waker.wake();
        }
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
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            state
                .waiters
                .drain(..)
                .map(|waiter| waiter.waker)
                .collect::<Vec<_>>()
        };
        for waker in waiters {
            waker.wake();
        }
    }

    /// Returns whether the semaphore is closed.
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    /// Acquires one borrowed permit.
    pub fn acquire(&self) -> Acquire<'_> {
        self.acquire_many(1)
    }

    /// Acquires several borrowed permits atomically.
    pub fn acquire_many(&self, permits: u32) -> Acquire<'_> {
        Acquire {
            semaphore: self,
            permits: permits as usize,
            waiter: None,
        }
    }

    /// Acquires one owned permit from an [`Arc`] semaphore.
    pub fn acquire_owned(self: Arc<Self>) -> AcquireOwned {
        self.acquire_many_owned(1)
    }

    /// Acquires several owned permits from an [`Arc`] semaphore.
    pub fn acquire_many_owned(self: Arc<Self>, permits: u32) -> AcquireOwned {
        AcquireOwned {
            semaphore: self,
            permits: permits as usize,
            waiter: None,
        }
    }

    /// Attempts to acquire one borrowed permit without waiting.
    pub fn try_acquire(&self) -> Result<SemaphorePermit<'_>, TryAcquireError> {
        self.try_acquire_many(1)
    }

    /// Attempts to acquire several borrowed permits without waiting.
    pub fn try_acquire_many(&self, permits: u32) -> Result<SemaphorePermit<'_>, TryAcquireError> {
        let permits = permits as usize;
        self.try_take(permits)?;
        Ok(SemaphorePermit {
            semaphore: self,
            permits,
            forgotten: false,
        })
    }

    /// Attempts to acquire one owned permit without waiting.
    pub fn try_acquire_owned(self: Arc<Self>) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        self.try_acquire_many_owned(1)
    }

    /// Attempts to acquire several owned permits without waiting.
    pub fn try_acquire_many_owned(
        self: Arc<Self>,
        permits: u32,
    ) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        let permits = permits as usize;
        self.try_take(permits)?;
        Ok(OwnedSemaphorePermit {
            semaphore: self,
            permits,
            forgotten: false,
        })
    }

    fn try_take(&self, permits: usize) -> Result<(), TryAcquireError> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(TryAcquireError::Closed);
        }
        if state.permits < permits {
            return Err(TryAcquireError::NoPermits);
        }
        state.permits -= permits;
        Ok(())
    }

    fn next_waker(state: &SemaphoreState) -> Option<Waker> {
        state
            .waiters
            .front()
            .filter(|waiter| state.permits >= waiter.permits)
            .map(|waiter| waiter.waker.clone())
    }

    fn poll_acquire(
        &self,
        waiter_id: &mut Option<u64>,
        permits: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), AcquireError>> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            *waiter_id = None;
            return Poll::Ready(Err(AcquireError::closed()));
        }

        if let Some(id) = *waiter_id {
            let position = state.waiters.iter().position(|waiter| waiter.id == id);
            if position == Some(0) && state.permits >= permits {
                state.waiters.pop_front();
                state.permits -= permits;
                *waiter_id = None;
                let next = Self::next_waker(&state);
                drop(state);
                if let Some(waker) = next {
                    waker.wake();
                }
                return Poll::Ready(Ok(()));
            }

            if let Some(position) = position {
                let queued = &mut state.waiters[position];
                if !queued.waker.will_wake(cx.waker()) {
                    queued.waker = cx.waker().clone();
                }
                return Poll::Pending;
            }
            // A close drains the queue, but that case returned above. Treat a
            // missing waiter defensively as a fresh acquisition.
            *waiter_id = None;
        }

        if state.waiters.is_empty() && state.permits >= permits {
            state.permits -= permits;
            return Poll::Ready(Ok(()));
        }

        let id = state.next_waiter;
        state.next_waiter = state.next_waiter.wrapping_add(1).max(1);
        state.waiters.push_back(SemaphoreWaiter {
            id,
            permits,
            waker: cx.waker().clone(),
        });
        *waiter_id = Some(id);
        Poll::Pending
    }

    fn cancel_waiter(&self, waiter_id: &mut Option<u64>) {
        let Some(id) = waiter_id.take() else {
            return;
        };
        let waker = {
            let mut state = self.state.lock().unwrap();
            if let Some(position) = state.waiters.iter().position(|waiter| waiter.id == id) {
                state.waiters.remove(position);
            }
            Self::next_waker(&state)
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl Default for Semaphore {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A borrowed permit returned by [`Semaphore::acquire`].
pub struct SemaphorePermit<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
    forgotten: bool,
}

impl SemaphorePermit<'_> {
    /// Returns the number of represented permits.
    pub fn num_permits(&self) -> usize {
        self.permits
    }

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

/// An owned permit returned by [`Semaphore::acquire_owned`].
pub struct OwnedSemaphorePermit {
    semaphore: Arc<Semaphore>,
    permits: usize,
    forgotten: bool,
}

impl OwnedSemaphorePermit {
    /// Returns the number of represented permits.
    pub fn num_permits(&self) -> usize {
        self.permits
    }

    /// Forgets the permit without returning it to the semaphore.
    pub fn forget(mut self) {
        self.forgotten = true;
    }

    /// Returns the semaphore that issued this permit.
    pub fn semaphore(&self) -> &Arc<Semaphore> {
        &self.semaphore
    }
}

impl Drop for OwnedSemaphorePermit {
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
    waiter: Option<u64>,
}

impl<'a> Future for Acquire<'a> {
    type Output = Result<SemaphorePermit<'a>, AcquireError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this
            .semaphore
            .poll_acquire(&mut this.waiter, this.permits, cx)
        {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(SemaphorePermit {
                semaphore: this.semaphore,
                permits: this.permits,
                forgotten: false,
            })),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Acquire<'_> {
    fn drop(&mut self) {
        self.semaphore.cancel_waiter(&mut self.waiter);
    }
}

/// Future returned by [`Semaphore::acquire_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct AcquireOwned {
    semaphore: Arc<Semaphore>,
    permits: usize,
    waiter: Option<u64>,
}

impl Future for AcquireOwned {
    type Output = Result<OwnedSemaphorePermit, AcquireError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this
            .semaphore
            .poll_acquire(&mut this.waiter, this.permits, cx)
        {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(OwnedSemaphorePermit {
                semaphore: this.semaphore.clone(),
                permits: this.permits,
                forgotten: false,
            })),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AcquireOwned {
    fn drop(&mut self) {
        self.semaphore.cancel_waiter(&mut self.waiter);
    }
}

/// Error returned when a semaphore acquisition fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquireError(());

impl AcquireError {
    /// Returns whether the semaphore was closed.
    pub fn is_closed(&self) -> bool {
        true
    }

    fn closed() -> Self {
        Self(())
    }
}

impl fmt::Display for AcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "semaphore closed")
    }
}

impl std::error::Error for AcquireError {}

/// Error returned by non-blocking semaphore acquisition.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_are_returned_on_drop() {
        let semaphore = Semaphore::new(1);
        let permit = semaphore.try_acquire().unwrap();
        assert_eq!(semaphore.available_permits(), 0);
        drop(permit);
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[test]
    fn owned_permit_returns_to_arc() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.clone().try_acquire_owned().unwrap();
        assert_eq!(semaphore.available_permits(), 0);
        drop(permit);
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[test]
    fn try_acquire_fails_when_empty() {
        let semaphore = Semaphore::new(0);
        assert!(matches!(
            semaphore.try_acquire(),
            Err(TryAcquireError::NoPermits)
        ));
    }

    #[test]
    fn add_permits_and_acquire_async() {
        let semaphore = Semaphore::new(0);
        semaphore.add_permits(2);
        let result = crate::Runtime::new().block_on(async move {
            let _p1 = semaphore.acquire().await.unwrap();
            let _p2 = semaphore.acquire().await.unwrap();
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

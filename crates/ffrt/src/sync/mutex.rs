use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use crate::lock::Mutex as FfrtMutex;

struct MutexState {
    locked: bool,
    waiters: VecDeque<Waker>,
}

/// An asynchronous, tokio-compatible mutex backed by an FFRT mutex.
pub struct Mutex<T> {
    state: FfrtMutex<MutexState>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Creates a new unlocked mutex.
    pub fn new(value: T) -> Self {
        Self {
            state: FfrtMutex::new(MutexState {
                locked: false,
                waiters: VecDeque::new(),
            }),
            value: UnsafeCell::new(value),
        }
    }

    /// Attempts to acquire the lock without waiting.
    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.locked {
            return Err(TryLockError);
        }
        state.locked = true;
        Ok(MutexGuard { mutex: self })
    }

    /// Acquires the lock asynchronously.
    pub fn lock(&self) -> MutexLockFuture<'_, T> {
        MutexLockFuture { mutex: self }
    }

    /// Acquires the lock synchronously.
    pub fn blocking_lock(&self) -> MutexGuard<'_, T> {
        loop {
            match self.try_lock() {
                Ok(guard) => return guard,
                Err(TryLockError) => std::thread::yield_now(),
            }
        }
    }

    /// Consumes the mutex and returns its inner value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> fmt::Debug for Mutex<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mutex").finish_non_exhaustive()
    }
}

/// Error returned by [`Mutex::try_lock`] when the lock is already held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryLockError;

impl fmt::Display for TryLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mutex is currently locked")
    }
}

impl std::error::Error for TryLockError {}

/// Future returned by [`Mutex::lock`].
#[must_use = "futures do nothing unless polled"]
pub struct MutexLockFuture<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<'a, T> Future for MutexLockFuture<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.mutex.state.lock().unwrap();

        if !state.locked {
            state.locked = true;
            return Poll::Ready(MutexGuard { mutex: this.mutex });
        }

        state.waiters.push_back(cx.waker().clone());
        Poll::Pending
    }
}

/// Guard returned by [`Mutex`] acquisition methods.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        let mut state = self.mutex.state.lock().unwrap();
        state.locked = false;
        let waker = state.waiters.pop_front();
        drop(state);

        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

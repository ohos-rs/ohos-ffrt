use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
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

    /// Acquires an owned guard from an [`Arc`] mutex.
    pub fn lock_owned(self: Arc<Self>) -> OwnedMutexLockFuture<T> {
        OwnedMutexLockFuture { mutex: self }
    }

    /// Attempts to acquire an owned guard without waiting.
    pub fn try_lock_owned(self: Arc<Self>) -> Result<OwnedMutexGuard<T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.locked {
            return Err(TryLockError);
        }
        state.locked = true;
        drop(state);
        Ok(OwnedMutexGuard { mutex: self })
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

    fn register_waiter(state: &mut MutexState, waker: &Waker) {
        if let Some(existing) = state
            .waiters
            .iter_mut()
            .find(|existing| existing.will_wake(waker))
        {
            *existing = waker.clone();
        } else {
            state.waiters.push_back(waker.clone());
        }
    }

    fn unlock(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.locked = false;
            state.waiters.drain(..).collect::<Vec<_>>()
        };
        for waker in waiters {
            waker.wake();
        }
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

        Mutex::<T>::register_waiter(&mut state, cx.waker());
        Poll::Pending
    }
}

/// Future returned by [`Mutex::lock_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct OwnedMutexLockFuture<T> {
    mutex: Arc<Mutex<T>>,
}

impl<T> Future for OwnedMutexLockFuture<T> {
    type Output = OwnedMutexGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.mutex.state.lock().unwrap();
        if !state.locked {
            state.locked = true;
            drop(state);
            return Poll::Ready(OwnedMutexGuard {
                mutex: this.mutex.clone(),
            });
        }
        Mutex::<T>::register_waiter(&mut state, cx.waker());
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
        self.mutex.unlock();
    }
}

/// An owned guard returned by [`Mutex::lock_owned`].
pub struct OwnedMutexGuard<T> {
    mutex: Arc<Mutex<T>>,
}

impl<T> Deref for OwnedMutexGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for OwnedMutexGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for OwnedMutexGuard<T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_lock_and_guard() {
        let mutex = Mutex::new(1);
        let result = crate::Runtime::new().block_on(async move {
            let mut guard = mutex.lock().await;
            *guard += 1;
            assert_eq!(*guard, 2);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn try_lock() {
        let mutex = Mutex::new(1);
        let guard = mutex.try_lock().unwrap();
        assert!(mutex.try_lock().is_err());
        drop(guard);
        assert!(mutex.try_lock().is_ok());
    }

    #[test]
    fn owned_guard_outlives_source_binding() {
        let mutex = Arc::new(Mutex::new(1));
        let result = crate::Runtime::new().block_on(async move {
            let mut guard = mutex.lock_owned().await;
            *guard += 1;
            assert_eq!(*guard, 2);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

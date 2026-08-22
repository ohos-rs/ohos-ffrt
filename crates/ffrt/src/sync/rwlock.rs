use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crate::lock::Mutex as FfrtMutex;

use super::mutex::TryLockError;

struct RwLockState {
    readers: usize,
    writer: bool,
    waiters: VecDeque<Waker>,
}

/// An asynchronous, tokio-compatible read-write lock backed by an FFRT mutex.
pub struct RwLock<T> {
    state: FfrtMutex<RwLockState>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Creates a new unlocked read-write lock.
    pub fn new(value: T) -> Self {
        Self {
            state: FfrtMutex::new(RwLockState {
                readers: 0,
                writer: false,
                waiters: VecDeque::new(),
            }),
            value: UnsafeCell::new(value),
        }
    }

    /// Attempts to acquire a read lock without waiting.
    pub fn try_read(&self) -> Result<RwLockReadGuard<'_, T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer {
            return Err(TryLockError);
        }
        state.readers += 1;
        Ok(RwLockReadGuard { lock: self })
    }

    /// Attempts to acquire a write lock without waiting.
    pub fn try_write(&self) -> Result<RwLockWriteGuard<'_, T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer || state.readers != 0 {
            return Err(TryLockError);
        }
        state.writer = true;
        Ok(RwLockWriteGuard { lock: self })
    }

    /// Acquires a read lock asynchronously.
    pub fn read(&self) -> RwLockReadFuture<'_, T> {
        RwLockReadFuture { lock: self }
    }

    /// Acquires a write lock asynchronously.
    pub fn write(&self) -> RwLockWriteFuture<'_, T> {
        RwLockWriteFuture { lock: self }
    }

    /// Acquires an owned read guard from an [`Arc`] lock.
    pub fn read_owned(self: Arc<Self>) -> OwnedRwLockReadFuture<T> {
        OwnedRwLockReadFuture { lock: self }
    }

    /// Acquires an owned write guard from an [`Arc`] lock.
    pub fn write_owned(self: Arc<Self>) -> OwnedRwLockWriteFuture<T> {
        OwnedRwLockWriteFuture { lock: self }
    }

    /// Attempts to acquire an owned read guard without waiting.
    pub fn try_read_owned(self: Arc<Self>) -> Result<OwnedRwLockReadGuard<T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer {
            return Err(TryLockError);
        }
        state.readers += 1;
        drop(state);
        Ok(OwnedRwLockReadGuard { lock: self })
    }

    /// Attempts to acquire an owned write guard without waiting.
    pub fn try_write_owned(self: Arc<Self>) -> Result<OwnedRwLockWriteGuard<T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer || state.readers != 0 {
            return Err(TryLockError);
        }
        state.writer = true;
        drop(state);
        Ok(OwnedRwLockWriteGuard { lock: self })
    }

    /// Acquires a read lock synchronously.
    pub fn blocking_read(&self) -> RwLockReadGuard<'_, T> {
        loop {
            match self.try_read() {
                Ok(guard) => return guard,
                Err(TryLockError) => std::thread::yield_now(),
            }
        }
    }

    /// Acquires a write lock synchronously.
    pub fn blocking_write(&self) -> RwLockWriteGuard<'_, T> {
        loop {
            match self.try_write() {
                Ok(guard) => return guard,
                Err(TryLockError) => std::thread::yield_now(),
            }
        }
    }

    /// Consumes the lock and returns its inner value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    fn register_waiter(state: &mut RwLockState, waker: &Waker) {
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

    fn wake_waiters(state: &mut RwLockState) -> Vec<Waker> {
        state.waiters.drain(..).collect()
    }

    fn release_read(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.readers -= 1;
            if state.readers == 0 {
                Self::wake_waiters(&mut state)
            } else {
                Vec::new()
            }
        };
        for waker in waiters {
            waker.wake();
        }
    }

    fn release_write(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.writer = false;
            Self::wake_waiters(&mut state)
        };
        for waker in waiters {
            waker.wake();
        }
    }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> fmt::Debug for RwLock<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RwLock").finish_non_exhaustive()
    }
}

/// Future returned by [`RwLock::read`].
#[must_use = "futures do nothing unless polled"]
pub struct RwLockReadFuture<'a, T> {
    lock: &'a RwLock<T>,
}

impl<'a, T> Future for RwLockReadFuture<'a, T> {
    type Output = RwLockReadGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.lock.state.lock().unwrap();

        if !state.writer {
            state.readers += 1;
            return Poll::Ready(RwLockReadGuard { lock: this.lock });
        }

        RwLock::<T>::register_waiter(&mut state, cx.waker());
        Poll::Pending
    }
}

/// Future returned by [`RwLock::write`].
#[must_use = "futures do nothing unless polled"]
pub struct RwLockWriteFuture<'a, T> {
    lock: &'a RwLock<T>,
}

impl<'a, T> Future for RwLockWriteFuture<'a, T> {
    type Output = RwLockWriteGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.lock.state.lock().unwrap();

        if !state.writer && state.readers == 0 {
            state.writer = true;
            return Poll::Ready(RwLockWriteGuard { lock: this.lock });
        }

        RwLock::<T>::register_waiter(&mut state, cx.waker());
        Poll::Pending
    }
}

/// Future returned by [`RwLock::read_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct OwnedRwLockReadFuture<T> {
    lock: Arc<RwLock<T>>,
}

impl<T> Future for OwnedRwLockReadFuture<T> {
    type Output = OwnedRwLockReadGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.lock.state.lock().unwrap();
        if !state.writer {
            state.readers += 1;
            drop(state);
            return Poll::Ready(OwnedRwLockReadGuard {
                lock: this.lock.clone(),
            });
        }
        RwLock::<T>::register_waiter(&mut state, cx.waker());
        Poll::Pending
    }
}

/// Future returned by [`RwLock::write_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct OwnedRwLockWriteFuture<T> {
    lock: Arc<RwLock<T>>,
}

impl<T> Future for OwnedRwLockWriteFuture<T> {
    type Output = OwnedRwLockWriteGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.lock.state.lock().unwrap();
        if !state.writer && state.readers == 0 {
            state.writer = true;
            drop(state);
            return Poll::Ready(OwnedRwLockWriteGuard {
                lock: this.lock.clone(),
            });
        }
        RwLock::<T>::register_waiter(&mut state, cx.waker());
        Poll::Pending
    }
}

/// Read guard returned by [`RwLock`].
pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.release_read();
    }
}

/// Write guard returned by [`RwLock`].
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.release_write();
    }
}

/// An owned read guard returned by [`RwLock::read_owned`].
pub struct OwnedRwLockReadGuard<T> {
    lock: Arc<RwLock<T>>,
}

impl<T> Deref for OwnedRwLockReadGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> Drop for OwnedRwLockReadGuard<T> {
    fn drop(&mut self) {
        self.lock.release_read();
    }
}

/// An owned write guard returned by [`RwLock::write_owned`].
pub struct OwnedRwLockWriteGuard<T> {
    lock: Arc<RwLock<T>>,
}

impl<T> Deref for OwnedRwLockWriteGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for OwnedRwLockWriteGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for OwnedRwLockWriteGuard<T> {
    fn drop(&mut self) {
        self.lock.release_write();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_read_write() {
        let lock = RwLock::new(1);
        let result = crate::Runtime::new().block_on(async move {
            let read = lock.read().await;
            assert_eq!(*read, 1);
            drop(read);

            let mut write = lock.write().await;
            *write += 1;
            assert_eq!(*write, 2);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn owned_read_and_write() {
        let lock = Arc::new(RwLock::new(3));
        let result = crate::Runtime::new().block_on(async move {
            let read = lock.clone().read_owned().await;
            assert_eq!(*read, 3);
            drop(read);
            let mut write = lock.write_owned().await;
            *write = 4;
            assert_eq!(*write, 4);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

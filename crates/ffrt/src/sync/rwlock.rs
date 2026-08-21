use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
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

        state.waiters.push_back(cx.waker().clone());
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

        state.waiters.push_back(cx.waker().clone());
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
        let mut state = self.lock.state.lock().unwrap();
        state.readers -= 1;
        let waker = if state.readers == 0 {
            state.waiters.pop_front()
        } else {
            None
        };
        drop(state);

        if let Some(waker) = waker {
            waker.wake();
        }
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
        let mut state = self.lock.state.lock().unwrap();
        state.writer = false;
        let waker = state.waiters.pop_front();
        drop(state);

        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

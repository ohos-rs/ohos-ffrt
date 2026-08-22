use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crate::lock::LazyMutex;

struct MutexState {
    locked: bool,
    next_waiter: u64,
    waiters: VecDeque<MutexWaiter>,
}

struct MutexWaiter {
    id: u64,
    waker: Waker,
}

/// An asynchronous, tokio-compatible mutex backed by an FFRT mutex.
pub struct Mutex<T> {
    state: LazyMutex<MutexState>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Creates a new unlocked mutex.
    pub fn new(value: T) -> Self {
        Self::const_new(value)
    }

    /// Creates a new unlocked mutex that can be used in a static.
    pub const fn const_new(value: T) -> Self {
        Self {
            state: LazyMutex::new(MutexState {
                locked: false,
                next_waiter: 1,
                waiters: VecDeque::new(),
            }),
            value: UnsafeCell::new(value),
        }
    }

    /// Attempts to acquire the lock without waiting.
    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.locked || !state.waiters.is_empty() {
            return Err(TryLockError);
        }
        state.locked = true;
        Ok(MutexGuard { mutex: self })
    }

    /// Acquires the lock asynchronously.
    pub fn lock(&self) -> MutexLockFuture<'_, T> {
        MutexLockFuture {
            mutex: self,
            waiter: None,
        }
    }

    /// Acquires an owned guard from an [`Arc`] mutex.
    pub fn lock_owned(self: Arc<Self>) -> OwnedMutexLockFuture<T> {
        OwnedMutexLockFuture {
            mutex: self,
            waiter: None,
        }
    }

    /// Attempts to acquire an owned guard without waiting.
    pub fn try_lock_owned(self: Arc<Self>) -> Result<OwnedMutexGuard<T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.locked || !state.waiters.is_empty() {
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

    /// Acquires an owned guard synchronously.
    pub fn blocking_lock_owned(self: Arc<Self>) -> OwnedMutexGuard<T> {
        loop {
            match self.clone().try_lock_owned() {
                Ok(guard) => return guard,
                Err(TryLockError) => std::thread::yield_now(),
            }
        }
    }

    /// Consumes the mutex and returns its inner value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    /// Returns a mutable reference without locking when the mutex is uniquely borrowed.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    fn poll_lock(&self, waiter_id: &mut Option<u64>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(id) = *waiter_id {
            let position = state.waiters.iter().position(|waiter| waiter.id == id);
            if position == Some(0) && !state.locked {
                state.waiters.pop_front();
                state.locked = true;
                *waiter_id = None;
                return Poll::Ready(());
            }
            if let Some(position) = position {
                let waiter = &mut state.waiters[position];
                if !waiter.waker.will_wake(cx.waker()) {
                    waiter.waker = cx.waker().clone();
                }
                return Poll::Pending;
            }
            *waiter_id = None;
        }

        if !state.locked && state.waiters.is_empty() {
            state.locked = true;
            return Poll::Ready(());
        }

        let id = state.next_waiter;
        state.next_waiter = state.next_waiter.wrapping_add(1).max(1);
        state.waiters.push_back(MutexWaiter {
            id,
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
            let was_front = state.waiters.front().is_some_and(|waiter| waiter.id == id);
            if let Some(position) = state.waiters.iter().position(|waiter| waiter.id == id) {
                state.waiters.remove(position);
            }
            (was_front && !state.locked)
                .then(|| state.waiters.front().map(|waiter| waiter.waker.clone()))
                .flatten()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn unlock(&self) {
        let waiter = {
            let mut state = self.state.lock().unwrap();
            state.locked = false;
            state.waiters.front().map(|waiter| waiter.waker.clone())
        };
        if let Some(waker) = waiter {
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
    waiter: Option<u64>,
}

impl<'a, T> Future for MutexLockFuture<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.mutex
            .poll_lock(&mut this.waiter, cx)
            .map(|()| MutexGuard { mutex: this.mutex })
    }
}

impl<T> Drop for MutexLockFuture<'_, T> {
    fn drop(&mut self) {
        self.mutex.cancel_waiter(&mut self.waiter);
    }
}

/// Future returned by [`Mutex::lock_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct OwnedMutexLockFuture<T> {
    mutex: Arc<Mutex<T>>,
    waiter: Option<u64>,
}

impl<T> Future for OwnedMutexLockFuture<T> {
    type Output = OwnedMutexGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.mutex
            .poll_lock(&mut this.waiter, cx)
            .map(|()| OwnedMutexGuard {
                mutex: this.mutex.clone(),
            })
    }
}

impl<T> Drop for OwnedMutexLockFuture<T> {
    fn drop(&mut self) {
        self.mutex.cancel_waiter(&mut self.waiter);
    }
}

/// Guard returned by [`Mutex`] acquisition methods.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<'a, T> MutexGuard<'a, T> {
    /// Maps this guard to a mutable subfield of the protected value.
    pub fn map<U, F>(this: Self, f: F) -> MappedMutexGuard<'a, U>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        let this = ManuallyDrop::new(this);
        let value = f(unsafe { &mut *this.mutex.value.get() });
        MappedMutexGuard {
            mutex: (this.mutex as *const Mutex<T>).cast(),
            value,
            unlock: unlock_mutex::<T>,
            _lifetime: PhantomData,
        }
    }

    /// Attempts to map this guard, returning it unchanged on failure.
    pub fn try_map<U, F>(this: Self, f: F) -> Result<MappedMutexGuard<'a, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.mutex.value.get() }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        Ok(MappedMutexGuard {
            mutex: (this.mutex as *const Mutex<T>).cast(),
            value,
            unlock: unlock_mutex::<T>,
            _lifetime: PhantomData,
        })
    }
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

impl<T> OwnedMutexGuard<T> {
    /// Returns the original mutex.
    pub fn mutex(this: &Self) -> &Arc<Mutex<T>> {
        &this.mutex
    }

    /// Maps this owned guard to a mutable subfield of the protected value.
    pub fn map<U, F>(this: Self, f: F) -> OwnedMappedMutexGuard<T, U>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        let this = ManuallyDrop::new(this);
        let value = f(unsafe { &mut *this.mutex.value.get() });
        let mutex = unsafe { std::ptr::read(&this.mutex) };
        OwnedMappedMutexGuard { mutex, value }
    }

    /// Attempts to map this owned guard, returning it unchanged on failure.
    pub fn try_map<U, F>(this: Self, f: F) -> Result<OwnedMappedMutexGuard<T, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.mutex.value.get() }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        let mutex = unsafe { std::ptr::read(&this.mutex) };
        Ok(OwnedMappedMutexGuard { mutex, value })
    }
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

unsafe fn unlock_mutex<T>(mutex: *const ()) {
    unsafe { (&*mutex.cast::<Mutex<T>>()).unlock() };
}

/// A borrowed mutex guard mapped to a subfield.
pub struct MappedMutexGuard<'a, T: ?Sized> {
    mutex: *const (),
    value: *mut T,
    unlock: unsafe fn(*const ()),
    _lifetime: PhantomData<&'a mut T>,
}

unsafe impl<T: ?Sized + Send> Send for MappedMutexGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for MappedMutexGuard<'_, T> {}

impl<'a, T: ?Sized> MappedMutexGuard<'a, T> {
    /// Maps this guard to a deeper mutable subfield.
    pub fn map<U, F>(this: Self, f: F) -> MappedMutexGuard<'a, U>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        let mut this = ManuallyDrop::new(this);
        let value = f(unsafe { &mut *this.value });
        MappedMutexGuard {
            mutex: this.mutex,
            value,
            unlock: this.unlock,
            _lifetime: PhantomData,
        }
    }

    /// Attempts to map this guard, returning it unchanged on failure.
    pub fn try_map<U, F>(this: Self, f: F) -> Result<MappedMutexGuard<'a, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let mut this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.value }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        Ok(MappedMutexGuard {
            mutex: this.mutex,
            value,
            unlock: this.unlock,
            _lifetime: PhantomData,
        })
    }
}

impl<T: ?Sized> Deref for MappedMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<T: ?Sized> DerefMut for MappedMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.value }
    }
}

impl<T: ?Sized> Drop for MappedMutexGuard<'_, T> {
    fn drop(&mut self) {
        unsafe { (self.unlock)(self.mutex) };
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MappedMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for MappedMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

/// An owned mutex guard mapped to a subfield.
pub struct OwnedMappedMutexGuard<T, U: ?Sized = T> {
    mutex: Arc<Mutex<T>>,
    value: *mut U,
}

unsafe impl<T: Send, U: ?Sized + Send> Send for OwnedMappedMutexGuard<T, U> {}
unsafe impl<T: Send + Sync, U: ?Sized + Sync> Sync for OwnedMappedMutexGuard<T, U> {}

impl<T, U: ?Sized> OwnedMappedMutexGuard<T, U> {
    /// Returns the original mutex.
    pub fn mutex(this: &Self) -> &Arc<Mutex<T>> {
        &this.mutex
    }

    /// Maps this guard to a deeper mutable subfield.
    pub fn map<V, F>(this: Self, f: F) -> OwnedMappedMutexGuard<T, V>
    where
        V: ?Sized,
        F: FnOnce(&mut U) -> &mut V,
    {
        let mut this = ManuallyDrop::new(this);
        let value = f(unsafe { &mut *this.value });
        let mutex = unsafe { std::ptr::read(&this.mutex) };
        OwnedMappedMutexGuard { mutex, value }
    }

    /// Attempts to map this guard, returning it unchanged on failure.
    pub fn try_map<V, F>(this: Self, f: F) -> Result<OwnedMappedMutexGuard<T, V>, Self>
    where
        V: ?Sized,
        F: FnOnce(&mut U) -> Option<&mut V>,
    {
        let mut this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.value }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        let mutex = unsafe { std::ptr::read(&this.mutex) };
        Ok(OwnedMappedMutexGuard { mutex, value })
    }
}

impl<T, U: ?Sized> Deref for OwnedMappedMutexGuard<T, U> {
    type Target = U;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<T, U: ?Sized> DerefMut for OwnedMappedMutexGuard<T, U> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.value }
    }
}

impl<T, U: ?Sized> Drop for OwnedMappedMutexGuard<T, U> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl<T, U: ?Sized + fmt::Debug> fmt::Debug for OwnedMappedMutexGuard<T, U> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T, U: ?Sized + fmt::Display> fmt::Display for OwnedMappedMutexGuard<T, U> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_lock_and_guard() {
        let mutex = Mutex::new(1);
        let result = crate::Runtime::new().unwrap().block_on(async move {
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
        let result = crate::Runtime::new().unwrap().block_on(async move {
            let mut guard = mutex.lock_owned().await;
            *guard += 1;
            assert_eq!(*guard, 2);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

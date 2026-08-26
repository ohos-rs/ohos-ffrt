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

use super::mutex::TryLockError;

struct RwLockState {
    readers: usize,
    max_readers: usize,
    writer: bool,
    next_waiter: u64,
    waiters: VecDeque<RwLockWaiter>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RwLockWaiterKind {
    Read,
    Write,
}

struct RwLockWaiter {
    id: u64,
    kind: RwLockWaiterKind,
    waker: Waker,
}

/// An asynchronous, tokio-compatible read-write lock backed by an FFRT mutex.
pub struct RwLock<T> {
    state: LazyMutex<RwLockState>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    const MAX_READERS: u32 = u32::MAX >> 3;

    /// Creates a new unlocked read-write lock.
    pub fn new(value: T) -> Self {
        Self::const_new(value)
    }

    /// Creates a read-write lock with a limit on concurrent readers.
    pub fn with_max_readers(value: T, max_reads: u32) -> Self {
        Self::const_with_max_readers(value, max_reads)
    }

    /// Creates a read-write lock that can be used in a static.
    pub const fn const_new(value: T) -> Self {
        Self::const_with_max_readers(value, Self::MAX_READERS)
    }

    /// Creates a statically usable lock with a concurrent-reader limit.
    pub const fn const_with_max_readers(value: T, max_reads: u32) -> Self {
        assert!(max_reads > 0, "max_reads must be greater than zero");
        assert!(
            max_reads <= Self::MAX_READERS,
            "max_reads exceeds the supported limit"
        );
        Self {
            state: LazyMutex::new(RwLockState {
                readers: 0,
                max_readers: max_reads as usize,
                writer: false,
                next_waiter: 1,
                waiters: VecDeque::new(),
            }),
            value: UnsafeCell::new(value),
        }
    }

    /// Attempts to acquire a read lock without waiting.
    pub fn try_read(&self) -> Result<RwLockReadGuard<'_, T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer || !state.waiters.is_empty() {
            return Err(TryLockError);
        }
        state.readers += 1;
        Ok(RwLockReadGuard::new(self))
    }

    /// Attempts to acquire a write lock without waiting.
    pub fn try_write(&self) -> Result<RwLockWriteGuard<'_, T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer || state.readers != 0 || !state.waiters.is_empty() {
            return Err(TryLockError);
        }
        state.writer = true;
        Ok(RwLockWriteGuard { lock: self })
    }

    /// Acquires a read lock asynchronously.
    pub fn read(&self) -> RwLockReadFuture<'_, T> {
        RwLockReadFuture {
            lock: self,
            waiter: None,
        }
    }

    /// Acquires a write lock asynchronously.
    pub fn write(&self) -> RwLockWriteFuture<'_, T> {
        RwLockWriteFuture {
            lock: self,
            waiter: None,
        }
    }

    /// Acquires an owned read guard from an [`Arc`] lock.
    pub fn read_owned(self: Arc<Self>) -> OwnedRwLockReadFuture<T> {
        OwnedRwLockReadFuture {
            lock: self,
            waiter: None,
        }
    }

    /// Acquires an owned write guard from an [`Arc`] lock.
    pub fn write_owned(self: Arc<Self>) -> OwnedRwLockWriteFuture<T> {
        OwnedRwLockWriteFuture {
            lock: self,
            waiter: None,
        }
    }

    /// Attempts to acquire an owned read guard without waiting.
    pub fn try_read_owned(self: Arc<Self>) -> Result<OwnedRwLockReadGuard<T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer || !state.waiters.is_empty() {
            return Err(TryLockError);
        }
        state.readers += 1;
        drop(state);
        Ok(OwnedRwLockReadGuard::new(self))
    }

    /// Attempts to acquire an owned write guard without waiting.
    pub fn try_write_owned(self: Arc<Self>) -> Result<OwnedRwLockWriteGuard<T>, TryLockError> {
        let mut state = self.state.lock().map_err(|_| TryLockError)?;
        if state.writer || state.readers != 0 || !state.waiters.is_empty() {
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

    /// Returns a mutable reference without locking when the lock is uniquely borrowed.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    fn can_acquire(state: &RwLockState, kind: RwLockWaiterKind) -> bool {
        match kind {
            RwLockWaiterKind::Read => !state.writer && state.readers < state.max_readers,
            RwLockWaiterKind::Write => !state.writer && state.readers == 0,
        }
    }

    fn next_waker(state: &RwLockState) -> Option<Waker> {
        state
            .waiters
            .front()
            .filter(|waiter| Self::can_acquire(state, waiter.kind))
            .map(|waiter| waiter.waker.clone())
    }

    fn poll_lock(
        &self,
        waiter_id: &mut Option<u64>,
        kind: RwLockWaiterKind,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(id) = *waiter_id {
            let position = state.waiters.iter().position(|waiter| waiter.id == id);
            if position == Some(0) && Self::can_acquire(&state, kind) {
                state.waiters.pop_front();
                match kind {
                    RwLockWaiterKind::Read => state.readers += 1,
                    RwLockWaiterKind::Write => state.writer = true,
                }
                *waiter_id = None;
                let next = (kind == RwLockWaiterKind::Read)
                    .then(|| Self::next_waker(&state))
                    .flatten();
                drop(state);
                if let Some(waker) = next {
                    waker.wake();
                }
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

        if state.waiters.is_empty() && Self::can_acquire(&state, kind) {
            match kind {
                RwLockWaiterKind::Read => state.readers += 1,
                RwLockWaiterKind::Write => state.writer = true,
            }
            return Poll::Ready(());
        }

        let id = state.next_waiter;
        state.next_waiter = state.next_waiter.wrapping_add(1).max(1);
        state.waiters.push_back(RwLockWaiter {
            id,
            kind,
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

    fn release_read(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.readers -= 1;
            if state.readers == 0 {
                Self::next_waker(&state)
            } else {
                None
            }
        };
        if let Some(waker) = waiters {
            waker.wake();
        }
    }

    fn release_write(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            state.writer = false;
            Self::next_waker(&state)
        };
        if let Some(waker) = waiters {
            waker.wake();
        }
    }

    fn downgrade_write(&self) {
        let waiters = {
            let mut state = self.state.lock().unwrap();
            debug_assert!(state.writer);
            state.writer = false;
            state.readers += 1;
            Self::next_waker(&state)
        };
        if let Some(waker) = waiters {
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
    waiter: Option<u64>,
}

impl<'a, T> Future for RwLockReadFuture<'a, T> {
    type Output = RwLockReadGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.lock
            .poll_lock(&mut this.waiter, RwLockWaiterKind::Read, cx)
            .map(|()| RwLockReadGuard::new(this.lock))
    }
}

impl<T> Drop for RwLockReadFuture<'_, T> {
    fn drop(&mut self) {
        self.lock.cancel_waiter(&mut self.waiter);
    }
}

/// Future returned by [`RwLock::write`].
#[must_use = "futures do nothing unless polled"]
pub struct RwLockWriteFuture<'a, T> {
    lock: &'a RwLock<T>,
    waiter: Option<u64>,
}

impl<'a, T> Future for RwLockWriteFuture<'a, T> {
    type Output = RwLockWriteGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.lock
            .poll_lock(&mut this.waiter, RwLockWaiterKind::Write, cx)
            .map(|()| RwLockWriteGuard { lock: this.lock })
    }
}

impl<T> Drop for RwLockWriteFuture<'_, T> {
    fn drop(&mut self) {
        self.lock.cancel_waiter(&mut self.waiter);
    }
}

/// Future returned by [`RwLock::read_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct OwnedRwLockReadFuture<T> {
    lock: Arc<RwLock<T>>,
    waiter: Option<u64>,
}

impl<T> Future for OwnedRwLockReadFuture<T> {
    type Output = OwnedRwLockReadGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.lock
            .poll_lock(&mut this.waiter, RwLockWaiterKind::Read, cx)
            .map(|()| OwnedRwLockReadGuard::new(this.lock.clone()))
    }
}

impl<T> Drop for OwnedRwLockReadFuture<T> {
    fn drop(&mut self) {
        self.lock.cancel_waiter(&mut self.waiter);
    }
}

/// Future returned by [`RwLock::write_owned`].
#[must_use = "futures do nothing unless polled"]
pub struct OwnedRwLockWriteFuture<T> {
    lock: Arc<RwLock<T>>,
    waiter: Option<u64>,
}

impl<T> Future for OwnedRwLockWriteFuture<T> {
    type Output = OwnedRwLockWriteGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.lock
            .poll_lock(&mut this.waiter, RwLockWaiterKind::Write, cx)
            .map(|()| OwnedRwLockWriteGuard {
                lock: this.lock.clone(),
            })
    }
}

impl<T> Drop for OwnedRwLockWriteFuture<T> {
    fn drop(&mut self) {
        self.lock.cancel_waiter(&mut self.waiter);
    }
}

unsafe fn release_read_erased<T>(lock: *const ()) {
    unsafe { (&*lock.cast::<RwLock<T>>()).release_read() };
}

unsafe fn release_write_erased<T>(lock: *const ()) {
    unsafe { (&*lock.cast::<RwLock<T>>()).release_write() };
}

/// Read guard returned by [`RwLock`].
pub struct RwLockReadGuard<'a, T: ?Sized> {
    lock: *const (),
    value: *const T,
    release: unsafe fn(*const ()),
    _lifetime: PhantomData<&'a T>,
}

unsafe impl<T: ?Sized + Sync> Send for RwLockReadGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for RwLockReadGuard<'_, T> {}

impl<'a, T> RwLockReadGuard<'a, T> {
    fn new(lock: &'a RwLock<T>) -> Self {
        Self {
            lock: (lock as *const RwLock<T>).cast(),
            value: lock.value.get(),
            release: release_read_erased::<T>,
            _lifetime: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> RwLockReadGuard<'a, T> {
    /// Maps this read guard to a subfield.
    pub fn map<U, F>(this: Self, f: F) -> RwLockReadGuard<'a, U>
    where
        U: ?Sized,
        F: FnOnce(&T) -> &U,
    {
        let this = ManuallyDrop::new(this);
        RwLockReadGuard {
            lock: this.lock,
            value: f(unsafe { &*this.value }),
            release: this.release,
            _lifetime: PhantomData,
        }
    }

    /// Attempts to map this read guard, returning it unchanged on failure.
    pub fn try_map<U, F>(this: Self, f: F) -> Result<RwLockReadGuard<'a, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&T) -> Option<&U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &*this.value }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        Ok(RwLockReadGuard {
            lock: this.lock,
            value,
            release: this.release,
            _lifetime: PhantomData,
        })
    }
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        unsafe { (self.release)(self.lock) };
    }
}

/// Write guard returned by [`RwLock`].
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<'a, T> RwLockWriteGuard<'a, T> {
    /// Maps this write guard to a mutable subfield.
    pub fn map<U, F>(this: Self, f: F) -> RwLockMappedWriteGuard<'a, U>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        let this = ManuallyDrop::new(this);
        RwLockMappedWriteGuard {
            lock: (this.lock as *const RwLock<T>).cast(),
            value: f(unsafe { &mut *this.lock.value.get() }),
            release: release_write_erased::<T>,
            _lifetime: PhantomData,
        }
    }

    /// Attempts to map this guard, returning it unchanged on failure.
    pub fn try_map<U, F>(this: Self, f: F) -> Result<RwLockMappedWriteGuard<'a, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.lock.value.get() }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        Ok(RwLockMappedWriteGuard {
            lock: (this.lock as *const RwLock<T>).cast(),
            value,
            release: release_write_erased::<T>,
            _lifetime: PhantomData,
        })
    }

    /// Converts this guard into the mapped-guard representation.
    pub fn into_mapped(this: Self) -> RwLockMappedWriteGuard<'a, T> {
        Self::map(this, |value| value)
    }

    /// Atomically downgrades this write guard into a read guard.
    pub fn downgrade(self) -> RwLockReadGuard<'a, T> {
        Self::downgrade_map(self, |value| value)
    }

    /// Atomically downgrades and maps this guard to a shared subfield.
    pub fn downgrade_map<U, F>(this: Self, f: F) -> RwLockReadGuard<'a, U>
    where
        U: ?Sized,
        F: FnOnce(&T) -> &U,
    {
        let this = ManuallyDrop::new(this);
        let value = f(unsafe { &*this.lock.value.get() });
        this.lock.downgrade_write();
        RwLockReadGuard {
            lock: (this.lock as *const RwLock<T>).cast(),
            value,
            release: release_read_erased::<T>,
            _lifetime: PhantomData,
        }
    }

    /// Attempts an atomic downgrade and map operation.
    pub fn try_downgrade_map<U, F>(this: Self, f: F) -> Result<RwLockReadGuard<'a, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&T) -> Option<&U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &*this.lock.value.get() }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        this.lock.downgrade_write();
        Ok(RwLockReadGuard {
            lock: (this.lock as *const RwLock<T>).cast(),
            value,
            release: release_read_erased::<T>,
            _lifetime: PhantomData,
        })
    }
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
pub struct OwnedRwLockReadGuard<T, U: ?Sized = T> {
    lock: Arc<RwLock<T>>,
    value: *const U,
}

unsafe impl<T: Send + Sync, U: ?Sized + Sync> Send for OwnedRwLockReadGuard<T, U> {}
unsafe impl<T: Send + Sync, U: ?Sized + Sync> Sync for OwnedRwLockReadGuard<T, U> {}

impl<T> OwnedRwLockReadGuard<T> {
    fn new(lock: Arc<RwLock<T>>) -> Self {
        let value = lock.value.get();
        Self { lock, value }
    }
}

impl<T, U: ?Sized> OwnedRwLockReadGuard<T, U> {
    /// Maps this owned read guard to a subfield.
    pub fn map<V, F>(this: Self, f: F) -> OwnedRwLockReadGuard<T, V>
    where
        V: ?Sized,
        F: FnOnce(&U) -> &V,
    {
        let this = ManuallyDrop::new(this);
        let value = f(unsafe { &*this.value });
        let lock = unsafe { std::ptr::read(&this.lock) };
        OwnedRwLockReadGuard { lock, value }
    }

    /// Attempts to map this guard, returning it unchanged on failure.
    pub fn try_map<V, F>(this: Self, f: F) -> Result<OwnedRwLockReadGuard<T, V>, Self>
    where
        V: ?Sized,
        F: FnOnce(&U) -> Option<&V>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &*this.value }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        let lock = unsafe { std::ptr::read(&this.lock) };
        Ok(OwnedRwLockReadGuard { lock, value })
    }

    /// Returns the original lock.
    pub fn rwlock(this: &Self) -> &Arc<RwLock<T>> {
        &this.lock
    }
}

impl<T, U: ?Sized> Deref for OwnedRwLockReadGuard<T, U> {
    type Target = U;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<T, U: ?Sized> Drop for OwnedRwLockReadGuard<T, U> {
    fn drop(&mut self) {
        self.lock.release_read();
    }
}

/// An owned write guard returned by [`RwLock::write_owned`].
pub struct OwnedRwLockWriteGuard<T> {
    lock: Arc<RwLock<T>>,
}

impl<T> OwnedRwLockWriteGuard<T> {
    /// Maps this owned write guard to a mutable subfield.
    pub fn map<U, F>(this: Self, f: F) -> OwnedRwLockMappedWriteGuard<T, U>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        let this = ManuallyDrop::new(this);
        let value = f(unsafe { &mut *this.lock.value.get() });
        let lock = unsafe { std::ptr::read(&this.lock) };
        OwnedRwLockMappedWriteGuard { lock, value }
    }

    /// Attempts to map this guard, returning it unchanged on failure.
    pub fn try_map<U, F>(this: Self, f: F) -> Result<OwnedRwLockMappedWriteGuard<T, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.lock.value.get() }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        let lock = unsafe { std::ptr::read(&this.lock) };
        Ok(OwnedRwLockMappedWriteGuard { lock, value })
    }

    /// Converts this guard into the mapped-guard representation.
    pub fn into_mapped(this: Self) -> OwnedRwLockMappedWriteGuard<T> {
        Self::map(this, |value| value)
    }

    /// Atomically downgrades this write guard into an owned read guard.
    pub fn downgrade(self) -> OwnedRwLockReadGuard<T> {
        Self::downgrade_map(self, |value| value)
    }

    /// Atomically downgrades and maps this guard to a shared subfield.
    pub fn downgrade_map<U, F>(this: Self, f: F) -> OwnedRwLockReadGuard<T, U>
    where
        U: ?Sized,
        F: FnOnce(&T) -> &U,
    {
        let this = ManuallyDrop::new(this);
        let value = f(unsafe { &*this.lock.value.get() });
        this.lock.downgrade_write();
        let lock = unsafe { std::ptr::read(&this.lock) };
        OwnedRwLockReadGuard { lock, value }
    }

    /// Attempts an atomic downgrade and map operation.
    pub fn try_downgrade_map<U, F>(this: Self, f: F) -> Result<OwnedRwLockReadGuard<T, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&T) -> Option<&U>,
    {
        let this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &*this.lock.value.get() }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        this.lock.downgrade_write();
        let lock = unsafe { std::ptr::read(&this.lock) };
        Ok(OwnedRwLockReadGuard { lock, value })
    }

    /// Returns the original lock.
    pub fn rwlock(this: &Self) -> &Arc<RwLock<T>> {
        &this.lock
    }
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

/// A borrowed write guard mapped to a subfield.
pub struct RwLockMappedWriteGuard<'a, T: ?Sized> {
    lock: *const (),
    value: *mut T,
    release: unsafe fn(*const ()),
    _lifetime: PhantomData<&'a mut T>,
}

unsafe impl<T: ?Sized + Send> Send for RwLockMappedWriteGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for RwLockMappedWriteGuard<'_, T> {}

impl<'a, T: ?Sized> RwLockMappedWriteGuard<'a, T> {
    pub fn map<U, F>(this: Self, f: F) -> RwLockMappedWriteGuard<'a, U>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        let mut this = ManuallyDrop::new(this);
        RwLockMappedWriteGuard {
            lock: this.lock,
            value: f(unsafe { &mut *this.value }),
            release: this.release,
            _lifetime: PhantomData,
        }
    }

    pub fn try_map<U, F>(this: Self, f: F) -> Result<RwLockMappedWriteGuard<'a, U>, Self>
    where
        U: ?Sized,
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        let mut this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.value }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        Ok(RwLockMappedWriteGuard {
            lock: this.lock,
            value,
            release: this.release,
            _lifetime: PhantomData,
        })
    }
}

impl<T: ?Sized> Deref for RwLockMappedWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<T: ?Sized> DerefMut for RwLockMappedWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.value }
    }
}

impl<T: ?Sized> Drop for RwLockMappedWriteGuard<'_, T> {
    fn drop(&mut self) {
        unsafe { (self.release)(self.lock) };
    }
}

/// An owned write guard mapped to a subfield.
pub struct OwnedRwLockMappedWriteGuard<T, U: ?Sized = T> {
    lock: Arc<RwLock<T>>,
    value: *mut U,
}

unsafe impl<T: Send + Sync, U: ?Sized + Send> Send for OwnedRwLockMappedWriteGuard<T, U> {}
unsafe impl<T: Send + Sync, U: ?Sized + Sync> Sync for OwnedRwLockMappedWriteGuard<T, U> {}

impl<T, U: ?Sized> OwnedRwLockMappedWriteGuard<T, U> {
    /// Returns the original lock.
    pub fn rwlock(this: &Self) -> &Arc<RwLock<T>> {
        &this.lock
    }

    pub fn map<V, F>(this: Self, f: F) -> OwnedRwLockMappedWriteGuard<T, V>
    where
        V: ?Sized,
        F: FnOnce(&mut U) -> &mut V,
    {
        let mut this = ManuallyDrop::new(this);
        let value = f(unsafe { &mut *this.value });
        let lock = unsafe { std::ptr::read(&this.lock) };
        OwnedRwLockMappedWriteGuard { lock, value }
    }

    pub fn try_map<V, F>(this: Self, f: F) -> Result<OwnedRwLockMappedWriteGuard<T, V>, Self>
    where
        V: ?Sized,
        F: FnOnce(&mut U) -> Option<&mut V>,
    {
        let mut this = ManuallyDrop::new(this);
        let Some(value) = f(unsafe { &mut *this.value }) else {
            return Err(ManuallyDrop::into_inner(this));
        };
        let lock = unsafe { std::ptr::read(&this.lock) };
        Ok(OwnedRwLockMappedWriteGuard { lock, value })
    }
}

impl<T, U: ?Sized> Deref for OwnedRwLockMappedWriteGuard<T, U> {
    type Target = U;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<T, U: ?Sized> DerefMut for OwnedRwLockMappedWriteGuard<T, U> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.value }
    }
}

impl<T, U: ?Sized> Drop for OwnedRwLockMappedWriteGuard<T, U> {
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
        let result = crate::Runtime::new().unwrap().block_on(async move {
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
        let result = crate::Runtime::new().unwrap().block_on(async move {
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

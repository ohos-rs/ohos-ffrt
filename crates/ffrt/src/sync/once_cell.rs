use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Poll, Waker};

use crate::lock::LazyMutex;

struct OnceState {
    initializing: bool,
    waiters: VecDeque<Waker>,
}

/// A Tokio-compatible cell initialized at most once.
pub struct OnceCell<T> {
    state: LazyMutex<OnceState>,
    initialized: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for OnceCell<T> {}
unsafe impl<T: Send + Sync> Sync for OnceCell<T> {}

impl<T> OnceCell<T> {
    /// Creates an empty cell.
    pub fn new() -> Self {
        Self::const_new()
    }

    /// Creates an empty cell that can be used in a static.
    pub const fn const_new() -> Self {
        Self {
            state: LazyMutex::new(OnceState {
                initializing: false,
                waiters: VecDeque::new(),
            }),
            initialized: AtomicBool::new(false),
            value: UnsafeCell::new(None),
        }
    }

    /// Creates a cell containing an optional initial value.
    pub fn new_with(value: Option<T>) -> Self {
        let initialized = value.is_some();
        Self {
            state: LazyMutex::new(OnceState {
                initializing: false,
                waiters: VecDeque::new(),
            }),
            initialized: AtomicBool::new(initialized),
            value: UnsafeCell::new(value),
        }
    }

    /// Creates an initialized cell that can be used in a static.
    pub const fn const_new_with(value: T) -> Self {
        Self {
            state: LazyMutex::new(OnceState {
                initializing: false,
                waiters: VecDeque::new(),
            }),
            initialized: AtomicBool::new(true),
            value: UnsafeCell::new(Some(value)),
        }
    }

    /// Returns whether the cell has been initialized.
    pub fn initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Returns the initialized value, if any.
    pub fn get(&self) -> Option<&T> {
        if !self.initialized() {
            return None;
        }
        // SAFETY: the release store to `initialized` happens after the value
        // is written, and a value cannot be removed through a shared borrow.
        unsafe { &*self.value.get() }.as_ref()
    }

    /// Returns a mutable reference to the initialized value, if any.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.get_mut().as_mut()
    }

    /// Sets the cell if it is neither initialized nor currently initializing.
    pub fn set(&self, value: T) -> Result<(), SetError<T>> {
        let mut state = self.state.lock().unwrap();
        if self.initialized() {
            return Err(SetError::AlreadyInitializedError(value));
        }
        if state.initializing {
            return Err(SetError::InitializingError(value));
        }

        unsafe { *self.value.get() = Some(value) };
        self.initialized.store(true, Ordering::Release);
        wake_all(&mut state);
        Ok(())
    }

    /// Returns the value, asynchronously initializing it when empty.
    pub async fn get_or_init<F, Fut>(&self, init: F) -> &T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if let Some(value) = self.get() {
            return value;
        }

        self.acquire_initializer().await;
        if let Some(value) = self.get() {
            return value;
        }

        let mut guard = InitializerGuard::new(self);
        let value = init().await;
        guard.complete(value)
    }

    /// Returns the value, asynchronously attempting to initialize it when empty.
    pub async fn get_or_try_init<E, F, Fut>(&self, init: F) -> Result<&T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if let Some(value) = self.get() {
            return Ok(value);
        }

        self.acquire_initializer().await;
        if let Some(value) = self.get() {
            return Ok(value);
        }

        let mut guard = InitializerGuard::new(self);
        match init().await {
            Ok(value) => Ok(guard.complete(value)),
            Err(error) => Err(error),
        }
    }

    async fn acquire_initializer(&self) {
        poll_fn(|cx| {
            if self.initialized() {
                return Poll::Ready(());
            }

            let mut state = self.state.lock().unwrap();
            if self.initialized() {
                return Poll::Ready(());
            }
            if !state.initializing {
                state.initializing = true;
                return Poll::Ready(());
            }

            if let Some(waiter) = state
                .waiters
                .iter_mut()
                .find(|waiter| waiter.will_wake(cx.waker()))
            {
                *waiter = cx.waker().clone();
            } else {
                state.waiters.push_back(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }

    /// Consumes the cell and returns its value.
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }

    /// Takes the current value, leaving the cell empty.
    pub fn take(&mut self) -> Option<T> {
        self.initialized.store(false, Ordering::Release);
        self.value.get_mut().take()
    }
}

impl<T> Default for OnceCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

struct InitializerGuard<'a, T> {
    cell: &'a OnceCell<T>,
    active: bool,
}

impl<'a, T> InitializerGuard<'a, T> {
    fn new(cell: &'a OnceCell<T>) -> Self {
        Self { cell, active: true }
    }

    fn complete(&mut self, value: T) -> &'a T {
        unsafe { *self.cell.value.get() = Some(value) };
        self.cell.initialized.store(true, Ordering::Release);
        let mut state = self.cell.state.lock().unwrap();
        state.initializing = false;
        wake_all(&mut state);
        self.active = false;
        self.cell.get().expect("OnceCell was just initialized")
    }
}

impl<T> Drop for InitializerGuard<'_, T> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.cell.state.lock().unwrap();
        state.initializing = false;
        wake_all(&mut state);
    }
}

fn wake_all(state: &mut OnceState) {
    for waker in state.waiters.drain(..) {
        waker.wake();
    }
}

/// Errors returned by [`OnceCell::set`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetError<T> {
    AlreadyInitializedError(T),
    InitializingError(T),
}

impl<T> SetError<T> {
    pub fn is_already_init_err(&self) -> bool {
        matches!(self, Self::AlreadyInitializedError(_))
    }

    pub fn is_initializing_err(&self) -> bool {
        matches!(self, Self::InitializingError(_))
    }
}

impl<T> fmt::Display for SetError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitializedError(_) => f.write_str("AlreadyInitializedError"),
            Self::InitializingError(_) => f.write_str("InitializingError"),
        }
    }
}

impl<T: fmt::Debug> std::error::Error for SetError<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_init_sets_value() {
        let cell = OnceCell::new();
        let result = crate::Runtime::new().unwrap().block_on(async move {
            let value = cell.get_or_init(|| async { 42 }).await;
            assert_eq!(*value, 42);
            let again = cell.get_or_init(|| async { 0 }).await;
            assert_eq!(*again, 42);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn set_rejects_duplicate() {
        let cell = OnceCell::new();
        assert!(cell.set(1).is_ok());
        assert!(cell.set(2).unwrap_err().is_already_init_err());
    }
}

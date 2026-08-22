//! Task-local values whose scope follows a future across worker threads.

use std::cell::RefCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Per-thread portion of a task-local key.
///
/// This type is public only so [`crate::task_local!`] can expand in downstream
/// crates; it is not intended to be constructed directly.
#[doc(hidden)]
pub struct LocalKeyInner<T> {
    stack: RefCell<Vec<*const T>>,
}

impl<T> LocalKeyInner<T> {
    /// Creates an empty task-local stack.
    #[doc(hidden)]
    pub const fn new() -> Self {
        Self {
            stack: RefCell::new(Vec::new()),
        }
    }
}

/// A key declared by [`crate::task_local!`].
pub struct LocalKey<T: 'static> {
    get: fn() -> *const LocalKeyInner<T>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> LocalKey<T> {
    /// Creates a key from its macro-generated thread-local accessor.
    #[doc(hidden)]
    pub const fn new(get: fn() -> *const LocalKeyInner<T>) -> Self {
        Self {
            get,
            _marker: PhantomData,
        }
    }

    fn inner(&self) -> &LocalKeyInner<T> {
        unsafe { &*(self.get)() }
    }

    /// Runs a future with `value` installed for this key.
    pub fn scope<F>(&'static self, value: T, future: F) -> TaskLocalFuture<T, F>
    where
        F: Future,
    {
        TaskLocalFuture {
            key: self,
            value,
            future,
        }
    }

    /// Runs a synchronous closure with `value` installed for this key.
    pub fn sync_scope<F, R>(&'static self, value: T, function: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _guard = self.enter(&value);
        function()
    }

    /// Borrows the current value or panics when called outside a scope.
    pub fn with<F, R>(&'static self, function: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.try_with(function)
            .expect("task-local value accessed outside its scope")
    }

    /// Borrows the current value, returning an error outside a scope.
    pub fn try_with<F, R>(&'static self, function: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let inner = self.inner();
        let stack = inner.stack.borrow();
        let value = stack.last().copied().ok_or(AccessError(()))?;
        Ok(function(unsafe { &*value }))
    }

    /// Copies the current value.
    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(|value| *value)
    }

    fn enter(&'static self, value: &T) -> EnterGuard<T> {
        let inner = self.inner();
        inner.stack.borrow_mut().push(value as *const T);
        EnterGuard {
            inner: inner as *const LocalKeyInner<T>,
        }
    }
}

/// Error returned when a task-local value is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessError(());

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "task-local value is not set")
    }
}

impl std::error::Error for AccessError {}

struct EnterGuard<T: 'static> {
    inner: *const LocalKeyInner<T>,
}

impl<T> Drop for EnterGuard<T> {
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner };
        inner
            .stack
            .borrow_mut()
            .pop()
            .expect("task-local scope stack corrupted");
    }
}

/// Future returned by [`LocalKey::scope`].
#[must_use = "futures do nothing unless polled"]
pub struct TaskLocalFuture<T: 'static, F> {
    key: &'static LocalKey<T>,
    value: T,
    future: F,
}

impl<T, F> Future for TaskLocalFuture<T, F>
where
    T: 'static,
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Neither `value` nor `future` is moved after the wrapper is pinned.
        let this = unsafe { self.get_unchecked_mut() };
        let _guard = this.key.enter(&this.value);
        unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    crate::task_local! {
        static REQUEST_ID: u64;
    }

    #[test]
    fn nested_scopes_restore_values() {
        let result = crate::Runtime::new().block_on(REQUEST_ID.scope(7, async {
            assert_eq!(REQUEST_ID.get(), 7);
            REQUEST_ID
                .scope(9, async {
                    crate::task::yield_now().await;
                    assert_eq!(REQUEST_ID.get(), 9);
                })
                .await;
            assert_eq!(REQUEST_ID.get(), 7);
            Ok::<(), crate::RuntimeError>(())
        }));
        assert!(result.is_ok());
    }
}

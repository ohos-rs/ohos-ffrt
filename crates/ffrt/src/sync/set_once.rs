use std::cell::UnsafeCell;
use std::fmt;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Poll, Waker};

/// A thread-safe cell that can be written once and awaited asynchronously.
pub struct SetOnce<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    state: AtomicU8,
    waiters: StdMutex<Vec<Waker>>,
}

// State 0 is empty, 1 is being initialized, and 2 is initialized.
const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;

unsafe impl<T: Send> Send for SetOnce<T> {}
unsafe impl<T: Send + Sync> Sync for SetOnce<T> {}

impl<T> SetOnce<T> {
    /// Creates an empty cell.
    pub fn new() -> Self {
        Self::const_new()
    }

    /// Creates an empty cell in a const context.
    pub const fn const_new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(EMPTY),
            waiters: StdMutex::new(Vec::new()),
        }
    }

    /// Creates a cell from an optional value.
    pub fn new_with(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::const_new_with(value),
            None => Self::new(),
        }
    }

    /// Creates an initialized cell in a const context.
    pub const fn const_new_with(value: T) -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::new(value)),
            state: AtomicU8::new(READY),
            waiters: StdMutex::new(Vec::new()),
        }
    }

    /// Returns the stored value, if initialized.
    pub fn get(&self) -> Option<&T> {
        (self.state.load(Ordering::Acquire) == READY)
            .then(|| unsafe { (&*self.value.get()).assume_init_ref() })
    }

    /// Returns whether the cell has been initialized.
    pub fn initialized(&self) -> bool {
        self.state.load(Ordering::Acquire) == READY
    }

    /// Sets the value and wakes every waiter.
    pub fn set(&self, value: T) -> Result<(), SetOnceError<T>> {
        if self
            .state
            .compare_exchange(EMPTY, WRITING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SetOnceError(value));
        }
        unsafe { (*self.value.get()).write(value) };
        self.state.store(READY, Ordering::Release);
        let waiters = std::mem::take(&mut *self.waiters.lock().unwrap());
        for waker in waiters {
            waker.wake();
        }
        Ok(())
    }

    /// Waits asynchronously until the value is set.
    pub async fn wait(&self) -> &T {
        std::future::poll_fn(|cx| {
            if let Some(value) = self.get() {
                return Poll::Ready(value);
            }

            let mut waiters = self.waiters.lock().unwrap();
            if let Some(value) = self.get() {
                return Poll::Ready(value);
            }
            if let Some(existing) = waiters
                .iter_mut()
                .find(|existing| existing.will_wake(cx.waker()))
            {
                *existing = cx.waker().clone();
            } else {
                waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }

    /// Consumes the cell and returns its value.
    pub fn into_inner(self) -> Option<T> {
        let this = ManuallyDrop::new(self);
        (this.state.load(Ordering::Acquire) == READY)
            .then(|| unsafe { (*this.value.get()).assume_init_read() })
    }
}

impl<T> Drop for SetOnce<T> {
    fn drop(&mut self) {
        if *self.state.get_mut() == READY {
            unsafe { self.value.get_mut().assume_init_drop() };
        }
    }
}

impl<T> Default for SetOnce<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<T> for SetOnce<T> {
    fn from(value: T) -> Self {
        Self::new_with(Some(value))
    }
}

impl<T: Clone> Clone for SetOnce<T> {
    fn clone(&self) -> Self {
        Self::new_with(self.get().cloned())
    }
}

impl<T: fmt::Debug> fmt::Debug for SetOnce<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SetOnce").field(&self.get()).finish()
    }
}

impl<T: PartialEq> PartialEq for SetOnce<T> {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl<T: Eq> Eq for SetOnce<T> {}

/// Error returned when a [`SetOnce`] is already initialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetOnceError<T>(pub T);

impl<T> fmt::Display for SetOnceError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SetOnce already initialized")
    }
}

impl<T: fmt::Debug> std::error::Error for SetOnceError<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn wait_observes_set_value() {
        let cell = Arc::new(SetOnce::new());
        let setter = cell.clone();
        crate::Runtime::new().unwrap().block_on(async move {
            let task = crate::spawn(async move { setter.set(42).unwrap() });
            assert_eq!(*cell.wait().await, 42);
            task.await.unwrap();
        });
    }
}

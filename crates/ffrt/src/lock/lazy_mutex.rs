use std::cell::UnsafeCell;
use std::sync::OnceLock;

use super::{LockError, Mutex, MutexGuard};

/// A const-constructible holder that initializes its FFRT mutex on first use.
pub(crate) struct LazyMutex<T> {
    mutex: OnceLock<Mutex<T>>,
    initial: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for LazyMutex<T> {}
unsafe impl<T: Send> Sync for LazyMutex<T> {}

impl<T> LazyMutex<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self {
            mutex: OnceLock::new(),
            initial: UnsafeCell::new(Some(value)),
        }
    }

    fn get(&self) -> &Mutex<T> {
        self.mutex.get_or_init(|| {
            // SAFETY: OnceLock serializes initialization, and `initial` is no
            // longer accessed after the FFRT mutex has been published.
            let value = unsafe { &mut *self.initial.get() }
                .take()
                .expect("lazy FFRT mutex initialized twice");
            Mutex::new(value)
        })
    }

    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, T>, LockError> {
        self.get().lock()
    }
}

use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

pub struct RwLock<T> {
    inner: ffrt_rwlock_t,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    pub fn new(value: T) -> Self {
        let mut inner: *mut ffrt_rwlock_t = ptr::null_mut();
        let result = unsafe { ffrt_rwlock_init(&mut inner, ptr::null()) };

        #[cfg(debug_assertions)]
        assert!(
            result == ffrt_error_t_ffrt_success,
            "Failed to initialize rwlock"
        );

        Self {
            inner,
            data: UnsafeCell::new(value),
        }
    }

    pub fn read(&self) -> Result<RwLockReadGuard<'_, T>, LockError> {
        let result = unsafe { ffrt_rwlock_rdlock(&self.inner as *const _ as *mut _) };

        match result {
            ffrt_error_t_ffrt_success => Ok(RwLockReadGuard { lock: self }),
            _ => Err(LockError::InnerError(format!(
                "Failed to read rwlock: {}",
                result
            ))),
        }
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, T>, LockError> {
        let result = unsafe { ffrt_rwlock_wrlock(&self.inner as *const _ as *mut _) };

        match result {
            ffrt_error_t_ffrt_success => Ok(RwLockWriteGuard { lock: self }),
            _ => Err(LockError::InnerError(format!(
                "Failed to write rwlock: {}",
                result
            ))),
        }
    }

    pub fn try_read(&self) -> Result<RwLockReadGuard<'_, T>, LockError> {
        let result = unsafe { ffrt_rwlock_tryrdlock(&self.inner as *const _ as *mut _) };

        match result {
            ffrt_error_t_ffrt_success => Ok(RwLockReadGuard { lock: self }),
            _ => Err(LockError::InnerError(format!(
                "Failed to try read rwlock: {}",
                result
            ))),
        }
    }

    pub fn try_write(&self) -> Result<RwLockWriteGuard<'_, T>, LockError> {
        let result = unsafe { ffrt_rwlock_trywrlock(&self.inner as *const _ as *mut _) };

        match result {
            ffrt_error_t_ffrt_success => Ok(RwLockWriteGuard { lock: self }),
            _ => Err(LockError::InnerError(format!(
                "Failed to try write rwlock: {}",
                result
            ))),
        }
    }
}

impl<T> Drop for RwLock<T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_rwlock_destroy(&mut self.inner);
        }
    }
}

pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_rwlock_unlock(&self.lock.inner as *const _ as *mut _);
        }
    }
}

pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_rwlock_unlock(&self.lock.inner as *const _ as *mut _);
        }
    }
}

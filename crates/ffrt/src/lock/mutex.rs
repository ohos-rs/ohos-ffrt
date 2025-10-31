use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};

use crate::lock::LockError;

pub struct Mutex<T> {
    inner: NonNull<ffrt_mutex_t>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// 创建新的互斥锁
    pub fn new(value: T) -> Self {
        use std::mem::MaybeUninit;

        let mut uninit = Box::new(MaybeUninit::<ffrt_mutex_t>::uninit());

        let result = unsafe { ffrt_mutex_init(uninit.as_mut_ptr(), ptr::null()) };

        #[cfg(debug_assertions)]
        assert!(
            result == ffrt_error_t_ffrt_success,
            "Failed to initialize mutex"
        );

        let inner = unsafe { uninit.assume_init() };
        let ptr = Box::into_raw(inner);

        Self {
            inner: unsafe { NonNull::new_unchecked(ptr) },
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> Result<MutexGuard<'_, T>, LockError> {
        let result = unsafe { ffrt_mutex_lock(&self.inner as *const _ as *mut _) };

        match result {
            ffrt_error_t_ffrt_success => Ok(MutexGuard { mutex: self }),
            _ => Err(LockError::InnerError(format!(
                "Failed to lock mutex: {}",
                result
            ))),
        }
    }

    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, LockError> {
        let result = unsafe { ffrt_mutex_trylock(&self.inner as *const _ as *mut _) };

        match result {
            ffrt_error_t_ffrt_success => Ok(MutexGuard { mutex: self }),
            _ => Err(LockError::InnerError(format!(
                "Failed to try lock mutex: {}",
                result
            ))),
        }
    }
}

impl<T> Drop for Mutex<T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_mutex_destroy(self.inner.as_ptr());
        }
    }
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_mutex_unlock(&self.mutex.inner as *const _ as *mut _);
        }
    }
}

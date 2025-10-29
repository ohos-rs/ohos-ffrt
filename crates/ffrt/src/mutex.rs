use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

/// 异步互斥锁
pub struct Mutex<T> {
    inner: ffrt_mutex_t,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// 创建新的互斥锁
    pub fn new(value: T) -> Self {
        let mut inner = unsafe { std::mem::zeroed() };
        unsafe {
            ffrt_mutex_init(&mut inner, std::ptr::null());
        }

        Self {
            inner,
            data: UnsafeCell::new(value),
        }
    }

    /// 锁定互斥锁
    pub async fn lock(&self) -> MutexGuard<'_, T> {
        unsafe {
            ffrt_mutex_lock(&self.inner as *const _ as *mut _);
        }
        MutexGuard { mutex: self }
    }

    /// 尝试锁定互斥锁
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let result = unsafe { ffrt_mutex_trylock(&self.inner as *const _ as *mut _) };

        if result == 0 {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }
}

impl<T> Drop for Mutex<T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_mutex_destroy(&mut self.inner);
        }
    }
}

/// 互斥锁守卫
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

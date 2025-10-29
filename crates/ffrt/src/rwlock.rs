use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

/// 异步读写锁
pub struct RwLock<T> {
    inner: ffrt_rwlock_t,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// 创建新的读写锁
    pub fn new(value: T) -> Self {
        let mut inner = unsafe { std::mem::zeroed() };
        unsafe {
            ffrt_rwlock_init(&mut inner, std::ptr::null());
        }

        Self {
            inner,
            data: UnsafeCell::new(value),
        }
    }

    /// 获取读锁
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        unsafe {
            ffrt_rwlock_rdlock(&self.inner as *const _ as *mut _);
        }
        RwLockReadGuard { lock: self }
    }

    /// 获取写锁
    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        unsafe {
            ffrt_rwlock_wrlock(&self.inner as *const _ as *mut _);
        }
        RwLockWriteGuard { lock: self }
    }

    /// 尝试获取读锁
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let result = unsafe { ffrt_rwlock_tryrdlock(&self.inner as *const _ as *mut _) };

        if result == 0 {
            Some(RwLockReadGuard { lock: self })
        } else {
            None
        }
    }

    /// 尝试获取写锁
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        let result = unsafe { ffrt_rwlock_trywrlock(&self.inner as *const _ as *mut _) };

        if result == 0 {
            Some(RwLockWriteGuard { lock: self })
        } else {
            None
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

/// 读锁守卫
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

/// 写锁守卫
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

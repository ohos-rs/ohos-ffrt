use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

/// FFRT读写锁 - 基于FFRT原生rwlock实现
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

    /// 获取读锁（同步阻塞）
    /// FFRT的rwlock_rdlock本身就是协程感知的，会自动让出执行权
    pub fn read(&self) -> Result<RwLockReadGuard<'_, T>, ()> {
        let result = unsafe { ffrt_rwlock_rdlock(&self.inner as *const _ as *mut _) };
        
        if result == 0 {
            Ok(RwLockReadGuard { lock: self })
        } else {
            Err(())
        }
    }

    /// 获取写锁（同步阻塞）
    /// FFRT的rwlock_wrlock本身就是协程感知的，会自动让出执行权
    pub fn write(&self) -> Result<RwLockWriteGuard<'_, T>, ()> {
        let result = unsafe { ffrt_rwlock_wrlock(&self.inner as *const _ as *mut _) };
        
        if result == 0 {
            Ok(RwLockWriteGuard { lock: self })
        } else {
            Err(())
        }
    }

    /// 尝试获取读锁（非阻塞）
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let result = unsafe { ffrt_rwlock_tryrdlock(&self.inner as *const _ as *mut _) };

        if result == 0 {
            Some(RwLockReadGuard { lock: self })
        } else {
            None
        }
    }

    /// 尝试获取写锁（非阻塞）
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

//! 异步Channel

use ohos_ffrt_sys::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::cell::UnsafeCell;

/// 创建有界channel
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        mutex: {
            let mut m = unsafe { std::mem::zeroed() };
            unsafe { ffrt_mutex_init(&mut m, std::ptr::null()) };
            m
        },
        cond_send: {
            let mut c = unsafe { std::mem::zeroed() };
            unsafe { ffrt_cond_init(&mut c, std::ptr::null()) };
            c
        },
        cond_recv: {
            let mut c = unsafe { std::mem::zeroed() };
            unsafe { ffrt_cond_init(&mut c, std::ptr::null()) };
            c
        },
        queue: UnsafeCell::new(VecDeque::new()),
        capacity,
        closed: UnsafeCell::new(false),
    });
    
    (
        Sender { shared: shared.clone() },
        Receiver { shared },
    )
}

struct Shared<T> {
    mutex: FfrtMutex,
    cond_send: FfrtCond,
    cond_recv: FfrtCond,
    queue: UnsafeCell<VecDeque<T>>,
    capacity: usize,
    closed: UnsafeCell<bool>,
}

unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_mutex_destroy(&mut self.mutex);
            ffrt_cond_destroy(&mut self.cond_send);
            ffrt_cond_destroy(&mut self.cond_recv);
        }
    }
}

/// 发送端
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Sender<T> {
    /// 发送值
    pub async fn send(&self, value: T) -> Result<(), T> {
        unsafe {
            ffrt_mutex_lock(&self.shared.mutex as *const _ as *mut _);
            
            // 等待队列有空间
            while (*self.shared.queue.get()).len() >= self.shared.capacity 
                && !*self.shared.closed.get() {
                ffrt_cond_wait(
                    &self.shared.cond_send as *const _ as *mut _,
                    &self.shared.mutex as *const _ as *mut _,
                );
            }
            
            if *self.shared.closed.get() {
                ffrt_mutex_unlock(&self.shared.mutex as *const _ as *mut _);
                return Err(value);
            }
            
            (*self.shared.queue.get()).push_back(value);
            ffrt_cond_signal(&self.shared.cond_recv as *const _ as *mut _);
            ffrt_mutex_unlock(&self.shared.mutex as *const _ as *mut _);
        }
        
        Ok(())
    }
    
    /// 关闭channel
    pub fn close(&self) {
        unsafe {
            ffrt_mutex_lock(&self.shared.mutex as *const _ as *mut _);
            *self.shared.closed.get() = true;
            ffrt_cond_broadcast(&self.shared.cond_recv as *const _ as *mut _);
            ffrt_cond_broadcast(&self.shared.cond_send as *const _ as *mut _);
            ffrt_mutex_unlock(&self.shared.mutex as *const _ as *mut _);
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

/// 接收端
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Receiver<T> {
    /// 接收值
    pub async fn recv(&self) -> Option<T> {
        unsafe {
            ffrt_mutex_lock(&self.shared.mutex as *const _ as *mut _);
            
            // 等待队列有数据
            while (*self.shared.queue.get()).is_empty() && !*self.shared.closed.get() {
                ffrt_cond_wait(
                    &self.shared.cond_recv as *const _ as *mut _,
                    &self.shared.mutex as *const _ as *mut _,
                );
            }
            
            let value = (*self.shared.queue.get()).pop_front();
            
            if value.is_some() {
                ffrt_cond_signal(&self.shared.cond_send as *const _ as *mut _);
            }
            
            ffrt_mutex_unlock(&self.shared.mutex as *const _ as *mut _);
            value
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}
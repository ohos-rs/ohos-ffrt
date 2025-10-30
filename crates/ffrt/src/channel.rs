use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// oneshot 错误类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    /// 发送方已关闭或丢弃
    Closed,
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecvError::Closed => write!(f, "channel closed"),
        }
    }
}

impl std::error::Error for RecvError {}

/// oneshot 发送错误
pub struct SendError<T>(pub T);

impl<T: std::fmt::Debug> std::fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SendError").field(&self.0).finish()
    }
}

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "receiver has been dropped")
    }
}

impl<T: std::fmt::Debug> std::error::Error for SendError<T> {}

/// 创建 oneshot channel - 只能发送一条消息
/// 使用FFRT的mutex和condition variable实现
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared::new());

    (
        Sender {
            shared: shared.clone(),
        },
        Receiver { shared },
    )
}

struct Inner<T> {
    value: Option<T>,
    sender_alive: bool,
    receiver_alive: bool,
    waker: Option<Waker>,
}

/// 基于FFRT同步原语的共享状态
struct Shared<T> {
    mutex: ffrt_mutex_t,
    cond: ffrt_cond_t,
    data: UnsafeCell<Inner<T>>,
}

impl<T> Shared<T> {
    fn new() -> Self {
        let mut mutex = unsafe { std::mem::zeroed() };
        let mut cond = unsafe { std::mem::zeroed() };

        unsafe {
            ffrt_mutex_init(&mut mutex, std::ptr::null());
            ffrt_cond_init(&mut cond, std::ptr::null());
        }

        Self {
            mutex,
            cond,
            data: UnsafeCell::new(Inner {
                value: None,
                sender_alive: true,
                receiver_alive: true,
                waker: None,
            }),
        }
    }

    fn lock(&self) -> SharedGuard<'_, T> {
        unsafe {
            ffrt_mutex_lock(&self.mutex as *const _ as *mut _);
        }
        SharedGuard { shared: self }
    }
}

struct SharedGuard<'a, T> {
    shared: &'a Shared<T>,
}

impl<'a, T> SharedGuard<'a, T> {
    fn inner(&self) -> &Inner<T> {
        unsafe { &*self.shared.data.get() }
    }

    fn inner_mut(&mut self) -> &mut Inner<T> {
        unsafe { &mut *self.shared.data.get() }
    }

    #[allow(dead_code)]
    fn signal(&self) {
        unsafe {
            ffrt_cond_signal(&self.shared.cond as *const _ as *mut _);
        }
    }

    fn broadcast(&self) {
        unsafe {
            ffrt_cond_broadcast(&self.shared.cond as *const _ as *mut _);
        }
    }
}

impl<'a, T> Drop for SharedGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_mutex_unlock(&self.shared.mutex as *const _ as *mut _);
        }
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_cond_destroy(&mut self.cond);
            ffrt_mutex_destroy(&mut self.mutex);
        }
    }
}

unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

/// oneshot 发送端 - 不可克隆
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Sender<T> {
    /// 发送值到channel
    /// 如果接收方已被drop，返回SendError
    pub fn send(self, value: T) -> Result<(), SendError<T>> {
        let mut guard = self.shared.lock();

        // 检查接收方是否已关闭
        if !guard.inner().receiver_alive {
            return Err(SendError(value));
        }

        // 设置值
        guard.inner_mut().value = Some(value);

        // 唤醒等待的接收方（使用condition variable）
        guard.broadcast();

        // 如果有waker也唤醒它
        if let Some(waker) = guard.inner_mut().waker.take() {
            waker.wake();
        }

        Ok(())
    }

    /// 检查接收方是否已关闭
    pub fn is_closed(&self) -> bool {
        let guard = self.shared.lock();
        !guard.inner().receiver_alive
    }

    /// 异步等待接收方关闭
    pub async fn closed(&self) {
        ClosedFuture {
            shared: self.shared.clone(),
        }
        .await
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().sender_alive = false;

        // 广播通知等待的接收方
        guard.broadcast();

        // 如果有waker也唤醒它
        if let Some(waker) = guard.inner_mut().waker.take() {
            waker.wake();
        }
    }
}

/// 等待发送方关闭的Future
struct ClosedFuture<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Future for ClosedFuture<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut guard = self.shared.lock();

        if !guard.inner().receiver_alive {
            Poll::Ready(())
        } else {
            // 保存waker以便在接收方关闭时被唤醒
            guard.inner_mut().waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// oneshot 接收端 - 不可克隆
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Receiver<T> {
    /// 尝试同步接收值（非阻塞）
    pub fn try_recv(&mut self) -> Result<T, RecvError> {
        let mut guard = self.shared.lock();

        if let Some(value) = guard.inner_mut().value.take() {
            Ok(value)
        } else if !guard.inner().sender_alive {
            Err(RecvError::Closed)
        } else {
            Err(RecvError::Closed)
        }
    }

    /// 关闭接收端
    pub fn close(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        // 通知等待的发送方
        guard.broadcast();
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut guard = self.shared.lock();

        // 检查是否有值
        if let Some(value) = guard.inner_mut().value.take() {
            return Poll::Ready(Ok(value));
        }

        // 检查发送方是否已关闭
        if !guard.inner().sender_alive {
            return Poll::Ready(Err(RecvError::Closed));
        }

        // 保存waker以便在值到达时被唤醒
        guard.inner_mut().waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;

        // 广播通知等待的发送方
        guard.broadcast();

        // 如果有等待的发送方，唤醒它
        if let Some(waker) = guard.inner_mut().waker.take() {
            waker.wake();
        }
    }
}

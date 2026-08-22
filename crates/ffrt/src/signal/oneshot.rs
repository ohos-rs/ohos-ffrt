use ffrt_sys::*;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// oneshot 接收错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "channel closed")
    }
}

impl std::error::Error for RecvError {}

/// oneshot 发送错误
#[derive(Debug, PartialEq, Eq)]
pub struct SendError<T>(pub T);

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "receiver dropped")
    }
}

impl<T: std::fmt::Debug> std::error::Error for SendError<T> {}

/// Error returned by [`Receiver::try_recv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    Empty,
    Closed,
}

impl std::fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("channel empty"),
            Self::Closed => f.write_str("channel closed"),
        }
    }
}

impl std::error::Error for TryRecvError {}

/// Tokio-compatible error namespace.
pub mod error {
    pub use super::{RecvError, TryRecvError};
}

/// 创建一个新的 oneshot channel
///
/// 返回一个 (Sender, Receiver) 对，只能发送一个值
///
/// # Examples
///
/// ```no_run
/// use ffrt::signal::oneshot;
///
/// let (tx, rx) = oneshot::channel();
///
/// ffrt::spawn(async move {
///     if let Err(_) = tx.send(42) {
///         println!("Receiver dropped");
///     }
/// });
///
/// match rx.await {
///     Ok(value) => println!("Got: {}", value),
///     Err(_) => println!("Sender dropped"),
/// }
/// ```
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared::new());
    (
        Sender {
            shared: Some(shared.clone()),
        },
        Receiver {
            shared: Some(shared),
        },
    )
}

struct Inner<T> {
    value: Option<T>,
    sender_alive: bool,
    receiver_alive: bool,
    recv_waker: Option<Waker>,
    close_waker: Option<Waker>,
}

/// 基于 FFRT 同步原语的共享状态
struct Shared<T> {
    mutex: NonNull<ffrt_mutex_t>,
    cond: NonNull<ffrt_cond_t>,
    data: UnsafeCell<Inner<T>>,
}

impl<T> Shared<T> {
    fn new() -> Self {
        use std::mem::MaybeUninit;

        let mut uninit_mutex = Box::new(MaybeUninit::<ffrt_mutex_t>::uninit());
        let mut uninit_cond = Box::new(MaybeUninit::<ffrt_cond_t>::uninit());

        unsafe {
            ffrt_mutex_init(uninit_mutex.as_mut_ptr(), std::ptr::null());
            ffrt_cond_init(uninit_cond.as_mut_ptr(), std::ptr::null());
        }

        let mutex = unsafe { uninit_mutex.assume_init() };
        let cond = unsafe { uninit_cond.assume_init() };

        Self {
            mutex: unsafe { NonNull::new_unchecked(Box::into_raw(mutex)) },
            cond: unsafe { NonNull::new_unchecked(Box::into_raw(cond)) },
            data: UnsafeCell::new(Inner {
                value: None,
                sender_alive: true,
                receiver_alive: true,
                recv_waker: None,
                close_waker: None,
            }),
        }
    }

    fn lock(&self) -> SharedGuard<'_, T> {
        unsafe {
            ffrt_mutex_lock(self.mutex.as_ptr());
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

    fn broadcast(&self) {
        unsafe {
            ffrt_cond_broadcast(self.shared.cond.as_ptr());
        }
    }
}

impl<'a, T> Drop for SharedGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_mutex_unlock(self.shared.mutex.as_ptr());
        }
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        unsafe {
            ffrt_cond_destroy(self.cond.as_ptr());
            ffrt_mutex_destroy(self.mutex.as_ptr());
            let _ = Box::from_raw(self.mutex.as_ptr());
            let _ = Box::from_raw(self.cond.as_ptr());
        }
    }
}

unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

/// oneshot channel 的发送端
///
/// 通过此类型发送单个值到对应的 Receiver
pub struct Sender<T> {
    shared: Option<Arc<Shared<T>>>,
}

impl<T> Sender<T> {
    /// 发送值到 channel
    ///
    /// 如果接收端已经被丢弃，则返回错误
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ffrt::signal::oneshot;
    ///
    /// let (tx, rx) = oneshot::channel();
    ///
    /// tx.send(42).unwrap();
    /// ```
    pub fn send(mut self, value: T) -> Result<(), T> {
        if let Some(shared) = self.shared.take() {
            let mut guard = shared.lock();

            if !guard.inner().receiver_alive {
                return Err(value);
            }

            guard.inner_mut().value = Some(value);
            guard.inner_mut().sender_alive = false;

            // Wake the receiver without disturbing a concurrent `closed()` waiter.
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }

            guard.broadcast();
            Ok(())
        } else {
            Err(value)
        }
    }

    /// 检查接收端是否已关闭
    pub fn is_closed(&self) -> bool {
        if let Some(shared) = &self.shared {
            let guard = shared.lock();
            !guard.inner().receiver_alive
        } else {
            true
        }
    }

    /// 异步等待接收端关闭
    pub async fn closed(&mut self) {
        if let Some(shared) = &self.shared {
            ClosedFuture {
                shared: shared.clone(),
            }
            .await
        }
    }

    /// Polls until the receiver has closed.
    pub fn poll_closed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(shared) = &self.shared else {
            return Poll::Ready(());
        };
        let mut guard = shared.lock();
        if !guard.inner().receiver_alive {
            Poll::Ready(())
        } else {
            guard.inner_mut().close_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            let mut guard = shared.lock();
            guard.inner_mut().sender_alive = false;

            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }

            guard.broadcast();
        }
    }
}

/// 等待接收端关闭的 Future
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
            guard.inner_mut().close_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// oneshot channel 的接收端
///
/// 通过 await 或 blocking_recv 接收单个值
pub struct Receiver<T> {
    shared: Option<Arc<Shared<T>>>,
}

impl<T> Receiver<T> {
    /// 尝试非阻塞地接收值
    ///
    /// 如果值还没准备好或发送端已关闭，返回错误
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ffrt::signal::oneshot;
    ///
    /// let (tx, mut rx) = oneshot::channel();
    ///
    /// // 没有值可接收
    /// assert!(rx.try_recv().is_err());
    ///
    /// tx.send(42).unwrap();
    ///
    /// // 现在可以接收了
    /// assert_eq!(rx.try_recv().unwrap(), 42);
    /// ```
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        if let Some(shared) = self.shared.clone() {
            let mut guard = shared.lock();

            if let Some(value) = guard.inner_mut().value.take() {
                drop(guard);
                self.shared.take();
                Ok(value)
            } else if guard.inner().sender_alive {
                Err(TryRecvError::Empty)
            } else {
                drop(guard);
                self.shared.take();
                Err(TryRecvError::Closed)
            }
        } else {
            Err(TryRecvError::Closed)
        }
    }

    /// 阻塞式接收值
    ///
    /// 会阻塞当前线程直到接收到值或发送端关闭
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ffrt::signal::oneshot;
    /// use std::thread;
    ///
    /// let (tx, rx) = oneshot::channel();
    ///
    /// thread::spawn(move || {
    ///     tx.send(42).unwrap();
    /// });
    ///
    /// assert_eq!(rx.blocking_recv().unwrap(), 42);
    /// ```
    pub fn blocking_recv(mut self) -> Result<T, RecvError> {
        if let Some(shared) = self.shared.take() {
            let mut guard = shared.lock();

            loop {
                if let Some(value) = guard.inner_mut().value.take() {
                    return Ok(value);
                }

                if !guard.inner().sender_alive {
                    return Err(RecvError);
                }

                // 等待条件变量
                unsafe {
                    ffrt_cond_wait(shared.cond.as_ptr(), shared.mutex.as_ptr());
                }
            }
        } else {
            Err(RecvError)
        }
    }

    /// Check whether the value has been sent or the channel has been closed.
    pub fn is_finished(&self) -> bool {
        if let Some(shared) = &self.shared {
            let guard = shared.lock();
            guard.inner().value.is_some() || !guard.inner().sender_alive
        } else {
            true
        }
    }

    /// Returns whether this receiver has already yielded its final result.
    pub fn is_terminated(&self) -> bool {
        self.shared.is_none()
    }

    /// Returns whether no value is currently buffered.
    pub fn is_empty(&self) -> bool {
        self.shared.as_ref().is_none_or(|shared| {
            let guard = shared.lock();
            guard.inner().value.is_none()
        })
    }

    /// Wake any task currently waiting on this receiver.
    pub fn wake_waiter(&self) {
        if let Some(shared) = &self.shared {
            let mut guard = shared.lock();
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }
            guard.broadcast();
        }
    }

    /// 关闭接收端
    pub fn close(&mut self) {
        if let Some(shared) = &self.shared {
            let mut guard = shared.lock();
            guard.inner_mut().receiver_alive = false;
            if let Some(waker) = guard.inner_mut().close_waker.take() {
                waker.wake();
            }
            guard.broadcast();
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(shared) = this.shared.clone() {
            let mut guard = shared.lock();

            if let Some(value) = guard.inner_mut().value.take() {
                drop(guard);
                this.shared.take();
                return Poll::Ready(Ok(value));
            }

            if !guard.inner().sender_alive {
                drop(guard);
                this.shared.take();
                return Poll::Ready(Err(RecvError));
            }

            guard.inner_mut().recv_waker = Some(cx.waker().clone());
            Poll::Pending
        } else {
            Poll::Ready(Err(RecvError))
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            let mut guard = shared.lock();
            guard.inner_mut().receiver_alive = false;

            if let Some(waker) = guard.inner_mut().close_waker.take() {
                waker.wake();
            }

            guard.broadcast();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_recv() {
        let (tx, rx) = channel();
        tx.send(42).unwrap();
        assert_eq!(rx.blocking_recv().unwrap(), 42);
    }

    #[test]
    fn test_sender_dropped() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert!(rx.blocking_recv().is_err());
    }

    #[test]
    fn test_receiver_dropped() {
        let (tx, _rx) = channel();
        assert!(tx.send(42).is_err());
    }
}

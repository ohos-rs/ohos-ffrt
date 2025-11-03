use ohos_ffrt_sys::*;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// 创建一个无界的 mpsc channel
///
/// 返回一个 (Sender, Receiver) 对，Sender 可以克隆
///
/// # Examples
///
/// ```no_run
/// use ohos_ffrt::signal::mpsc;
///
/// let (tx, mut rx) = mpsc::unbounded_channel();
///
/// ohos_ffrt::spawn(async move {
///     tx.send(42).unwrap();
/// });
///
/// if let Some(value) = rx.recv().await {
///     println!("Got: {}", value);
/// }
/// ```
pub fn unbounded_channel<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let shared = Arc::new(Shared::new(None));
    (
        UnboundedSender {
            shared: shared.clone(),
        },
        UnboundedReceiver { shared },
    )
}

/// 创建一个有界的 mpsc channel
///
/// capacity 参数指定队列的最大容量
///
/// # Examples
///
/// ```no_run
/// use ohos_ffrt::signal::mpsc;
///
/// let (tx, mut rx) = mpsc::channel(10);
///
/// ohos_ffrt::spawn(async move {
///     for i in 0..5 {
///         tx.send(i).await.unwrap();
///     }
/// });
///
/// while let Some(value) = rx.recv().await {
///     println!("Got: {}", value);
/// }
/// ```
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared::new(Some(capacity)));
    (
        Sender {
            shared: shared.clone(),
        },
        Receiver { shared },
    )
}

/// mpsc 发送错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendError<T>(pub T);

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "receiver dropped")
    }
}

impl<T: std::fmt::Debug> std::error::Error for SendError<T> {}

/// mpsc 接收错误  
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "all senders dropped")
    }
}

impl std::error::Error for RecvError {}

/// mpsc 超时发送错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrySendError<T> {
    /// 队列已满
    Full(T),
    /// 接收端已关闭
    Disconnected(T),
}

impl<T> std::fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrySendError::Full(_) => write!(f, "channel full"),
            TrySendError::Disconnected(_) => write!(f, "receiver dropped"),
        }
    }
}

impl<T: std::fmt::Debug> std::error::Error for TrySendError<T> {}

/// mpsc 超时接收错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    /// 队列为空
    Empty,
    /// 所有发送端已关闭
    Disconnected,
}

impl std::fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryRecvError::Empty => write!(f, "channel empty"),
            TryRecvError::Disconnected => write!(f, "all senders dropped"),
        }
    }
}

impl std::error::Error for TryRecvError {}

struct Inner<T> {
    queue: VecDeque<T>,
    capacity: Option<usize>,
    sender_count: usize,
    receiver_alive: bool,
    recv_waker: Option<Waker>,
    send_wakers: VecDeque<Waker>,
}

/// 基于 FFRT 同步原语的共享状态
struct Shared<T> {
    mutex: NonNull<ffrt_mutex_t>,
    cond: NonNull<ffrt_cond_t>,
    data: UnsafeCell<Inner<T>>,
}

impl<T> Shared<T> {
    fn new(capacity: Option<usize>) -> Self {
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
                queue: VecDeque::new(),
                capacity,
                sender_count: 1,
                receiver_alive: true,
                recv_waker: None,
                send_wakers: VecDeque::new(),
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

    fn wait(&mut self) {
        unsafe {
            ffrt_cond_wait(self.shared.cond.as_ptr(), self.shared.mutex.as_ptr());
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
            let _ = Box::from_raw(self.mutex.as_ptr());
            let _ = Box::from_raw(self.cond.as_ptr());
            ffrt_cond_destroy(self.cond.as_ptr());
            ffrt_mutex_destroy(self.mutex.as_ptr());
        }
    }
}

unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

/// 有界 mpsc channel 的发送端
///
/// 可以克隆以创建多个发送者
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Sender<T> {
    /// 异步发送值到 channel
    ///
    /// 如果队列已满，会等待直到有空间
    pub async fn send(&self, value: T) -> Result<(), SendError<T>> {
        SendFuture {
            shared: self.shared.clone(),
            value: Some(value),
        }
        .await
    }

    /// 尝试立即发送值
    ///
    /// 如果队列已满或接收端已关闭，返回错误
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut guard = self.shared.lock();

        if !guard.inner().receiver_alive {
            return Err(TrySendError::Disconnected(value));
        }

        let capacity = guard.inner().capacity;
        if let Some(cap) = capacity {
            if guard.inner().queue.len() >= cap {
                return Err(TrySendError::Full(value));
            }
        }

        guard.inner_mut().queue.push_back(value);

        // 唤醒等待的接收者
        if let Some(waker) = guard.inner_mut().recv_waker.take() {
            waker.wake();
        }

        guard.broadcast();
        Ok(())
    }

    /// 阻塞式发送值
    ///
    /// 会阻塞当前线程直到发送成功或接收端关闭
    pub fn blocking_send(&self, value: T) -> Result<(), SendError<T>> {
        let mut guard = self.shared.lock();
        let mut current_value = Some(value);

        loop {
            if !guard.inner().receiver_alive {
                return Err(SendError(current_value.take().unwrap()));
            }

            let capacity = guard.inner().capacity;
            if let Some(cap) = capacity {
                if guard.inner().queue.len() >= cap {
                    guard.wait();
                    continue;
                }
            }

            guard
                .inner_mut()
                .queue
                .push_back(current_value.take().unwrap());

            // 唤醒等待的接收者
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }

            guard.broadcast();
            return Ok(());
        }
    }

    /// 检查接收端是否已关闭
    pub fn is_closed(&self) -> bool {
        let guard = self.shared.lock();
        !guard.inner().receiver_alive
    }

    /// 获取当前队列中的消息数量
    pub fn len(&self) -> usize {
        let guard = self.shared.lock();
        guard.inner().queue.len()
    }

    /// 检查队列是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取队列的容量限制
    pub fn capacity(&self) -> Option<usize> {
        let guard = self.shared.lock();
        guard.inner().capacity
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut guard = self.shared.lock();
        guard.inner_mut().sender_count += 1;
        drop(guard);

        Sender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().sender_count -= 1;

        if guard.inner().sender_count == 0 {
            // 唤醒等待的接收者
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }
            guard.broadcast();
        }
    }
}

/// 发送 Future
struct SendFuture<T> {
    shared: Arc<Shared<T>>,
    value: Option<T>,
}

impl<T> Future for SendFuture<T> {
    type Output = Result<(), SendError<T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We don't move out of self, only access fields
        let this = unsafe { self.get_unchecked_mut() };
        let mut guard = this.shared.lock();

        if !guard.inner().receiver_alive {
            return Poll::Ready(Err(SendError(this.value.take().unwrap())));
        }

        let capacity = guard.inner().capacity;
        if let Some(cap) = capacity {
            if guard.inner().queue.len() >= cap {
                // 队列已满，保存 waker 并返回 Pending
                guard.inner_mut().send_wakers.push_back(cx.waker().clone());
                return Poll::Pending;
            }
        }

        // 有空间，发送消息
        guard
            .inner_mut()
            .queue
            .push_back(this.value.take().unwrap());

        // 唤醒等待的接收者
        if let Some(waker) = guard.inner_mut().recv_waker.take() {
            waker.wake();
        }

        guard.broadcast();
        Poll::Ready(Ok(()))
    }
}

/// 有界 mpsc channel 的接收端
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Receiver<T> {
    /// 异步接收值
    ///
    /// 如果队列为空，会等待直到有消息或所有发送端关闭
    pub async fn recv(&mut self) -> Option<T> {
        RecvFuture {
            shared: self.shared.clone(),
        }
        .await
    }

    /// 尝试立即接收值
    ///
    /// 如果队列为空，立即返回错误
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut guard = self.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            // 唤醒等待的发送者
            if let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
                waker.wake();
            }
            guard.broadcast();
            Ok(value)
        } else if guard.inner().sender_count == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    /// 阻塞式接收值
    ///
    /// 会阻塞当前线程直到接收到值或所有发送端关闭
    pub fn blocking_recv(&mut self) -> Option<T> {
        let mut guard = self.shared.lock();

        loop {
            if let Some(value) = guard.inner_mut().queue.pop_front() {
                // 唤醒等待的发送者
                if let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
                    waker.wake();
                }
                guard.broadcast();
                return Some(value);
            }

            if guard.inner().sender_count == 0 {
                return None;
            }

            guard.wait();
        }
    }

    /// 关闭接收端
    ///
    /// 这会导致所有后续的发送操作失败
    pub fn close(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;

        // 唤醒所有等待的发送者
        while let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
            waker.wake();
        }

        guard.broadcast();
    }

    /// 获取当前队列中的消息数量
    pub fn len(&self) -> usize {
        let guard = self.shared.lock();
        guard.inner().queue.len()
    }

    /// 检查队列是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;

        // 唤醒所有等待的发送者
        while let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
            waker.wake();
        }

        guard.broadcast();
    }
}

/// 接收 Future
struct RecvFuture<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Future for RecvFuture<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut guard = self.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            // 唤醒等待的发送者
            if let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
                waker.wake();
            }
            guard.broadcast();
            return Poll::Ready(Some(value));
        }

        if guard.inner().sender_count == 0 {
            return Poll::Ready(None);
        }

        // 保存 waker 并等待
        guard.inner_mut().recv_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// 无界 mpsc channel 的发送端
///
/// 可以克隆以创建多个发送者
pub struct UnboundedSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> UnboundedSender<T> {
    /// 发送值到 channel
    ///
    /// 由于是无界 channel，此操作不会阻塞
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut guard = self.shared.lock();

        if !guard.inner().receiver_alive {
            return Err(SendError(value));
        }

        guard.inner_mut().queue.push_back(value);

        // 唤醒等待的接收者
        if let Some(waker) = guard.inner_mut().recv_waker.take() {
            waker.wake();
        }

        guard.broadcast();
        Ok(())
    }

    /// 检查接收端是否已关闭
    pub fn is_closed(&self) -> bool {
        let guard = self.shared.lock();
        !guard.inner().receiver_alive
    }

    /// 获取当前队列中的消息数量
    pub fn len(&self) -> usize {
        let guard = self.shared.lock();
        guard.inner().queue.len()
    }

    /// 检查队列是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Clone for UnboundedSender<T> {
    fn clone(&self) -> Self {
        let mut guard = self.shared.lock();
        guard.inner_mut().sender_count += 1;
        drop(guard);

        UnboundedSender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for UnboundedSender<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().sender_count -= 1;

        if guard.inner().sender_count == 0 {
            // 唤醒等待的接收者
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }
            guard.broadcast();
        }
    }
}

/// 无界 mpsc channel 的接收端
pub struct UnboundedReceiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> UnboundedReceiver<T> {
    /// 异步接收值
    ///
    /// 如果队列为空，会等待直到有消息或所有发送端关闭
    pub async fn recv(&mut self) -> Option<T> {
        RecvFuture {
            shared: self.shared.clone(),
        }
        .await
    }

    /// 尝试立即接收值
    ///
    /// 如果队列为空，立即返回错误
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut guard = self.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            guard.broadcast();
            Ok(value)
        } else if guard.inner().sender_count == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    /// 阻塞式接收值
    ///
    /// 会阻塞当前线程直到接收到值或所有发送端关闭
    pub fn blocking_recv(&mut self) -> Option<T> {
        let mut guard = self.shared.lock();

        loop {
            if let Some(value) = guard.inner_mut().queue.pop_front() {
                guard.broadcast();
                return Some(value);
            }

            if guard.inner().sender_count == 0 {
                return None;
            }

            guard.wait();
        }
    }

    /// 关闭接收端
    ///
    /// 这会导致所有后续的发送操作失败
    pub fn close(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        guard.broadcast();
    }

    /// 获取当前队列中的消息数量
    pub fn len(&self) -> usize {
        let guard = self.shared.lock();
        guard.inner().queue.len()
    }

    /// 检查队列是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Drop for UnboundedReceiver<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        guard.broadcast();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unbounded_send_recv() {
        let (tx, mut rx) = unbounded_channel();
        tx.send(42).unwrap();
        assert_eq!(rx.try_recv().unwrap(), 42);
    }

    #[test]
    fn test_bounded_send_recv() {
        let (tx, mut rx) = channel(10);
        tx.try_send(42).unwrap();
        assert_eq!(rx.try_recv().unwrap(), 42);
    }

    #[test]
    fn test_bounded_full() {
        let (tx, _rx) = channel(1);
        tx.try_send(42).unwrap();
        assert!(matches!(tx.try_send(43), Err(TrySendError::Full(_))));
    }

    #[test]
    fn test_clone_sender() {
        let (tx, mut rx) = unbounded_channel();
        let tx2 = tx.clone();

        tx.send(1).unwrap();
        tx2.send(2).unwrap();

        assert_eq!(rx.try_recv().unwrap(), 1);
        assert_eq!(rx.try_recv().unwrap(), 2);
    }

    #[test]
    fn test_all_senders_dropped() {
        let (tx, mut rx) = unbounded_channel::<i32>();
        drop(tx);
        assert!(rx.try_recv().is_err());
    }
}

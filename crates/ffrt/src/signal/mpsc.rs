use ffrt_sys::*;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};

/// 创建一个无界的 mpsc channel
///
/// 返回一个 (Sender, Receiver) 对，Sender 可以克隆
///
/// # Examples
///
/// ```no_run
/// # ffrt::Runtime::new().unwrap().block_on(async {
/// use ffrt::signal::mpsc;
///
/// let (tx, mut rx) = mpsc::unbounded_channel();
///
/// ffrt::spawn(async move {
///     tx.send(42).unwrap();
/// });
///
/// if let Some(value) = rx.recv().await {
///     println!("Got: {}", value);
/// }
/// # });
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
/// # ffrt::Runtime::new().unwrap().block_on(async {
/// use ffrt::signal::mpsc;
///
/// let (tx, mut rx) = mpsc::channel(10);
///
/// ffrt::spawn(async move {
///     for i in 0..5 {
///         tx.send(i).await.unwrap();
///     }
/// });
///
/// while let Some(value) = rx.recv().await {
///     println!("Got: {}", value);
/// }
/// # });
/// ```
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(
        capacity > 0,
        "mpsc bounded channel requires positive capacity"
    );
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

impl<T> SendError<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

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
    Closed(T),
}

impl<T> TrySendError<T> {
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full(_))
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed(_))
    }

    pub fn into_inner(self) -> T {
        match self {
            Self::Full(value) | Self::Closed(value) => value,
        }
    }
}

impl<T> std::fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrySendError::Full(_) => write!(f, "channel full"),
            TrySendError::Closed(_) => write!(f, "receiver dropped"),
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

/// Error returned by [`Sender::send_timeout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTimeoutError<T> {
    /// The send timed out.
    Timeout(T),
    /// The receiver was closed.
    Closed(T),
}

impl<T> SendTimeoutError<T> {
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout(_))
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed(_))
    }

    pub fn into_inner(self) -> T {
        match self {
            Self::Timeout(value) | Self::Closed(value) => value,
        }
    }
}

impl<T> std::fmt::Display for SendTimeoutError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendTimeoutError::Timeout(_) => write!(f, "send timed out"),
            SendTimeoutError::Closed(_) => write!(f, "receiver dropped"),
        }
    }
}

impl<T: std::fmt::Debug> std::error::Error for SendTimeoutError<T> {}

/// Error returned by [`Receiver::recv_timeout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvTimeoutError {
    /// The receive timed out.
    Timeout,
    /// All senders were closed.
    Closed,
}

impl std::fmt::Display for RecvTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecvTimeoutError::Timeout => write!(f, "receive timed out"),
            RecvTimeoutError::Closed => write!(f, "all senders dropped"),
        }
    }
}

impl std::error::Error for RecvTimeoutError {}

/// Tokio-compatible error namespace.
pub mod error {
    pub use super::{
        RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError,
    };
}

struct Inner<T> {
    queue: VecDeque<T>,
    capacity: Option<usize>,
    reserved: usize,
    sender_count: usize,
    receiver_alive: bool,
    recv_waker: Option<Waker>,
    send_wakers: VecDeque<Waker>,
    close_waiters: VecDeque<Waker>,
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
                reserved: 0,
                sender_count: 1,
                receiver_alive: true,
                recv_waker: None,
                send_wakers: VecDeque::new(),
                close_waiters: VecDeque::new(),
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
            ffrt_cond_destroy(self.cond.as_ptr());
            ffrt_mutex_destroy(self.mutex.as_ptr());
            let _ = Box::from_raw(self.mutex.as_ptr());
            let _ = Box::from_raw(self.cond.as_ptr());
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

    /// Sends a value, waiting at most `timeout` for queue capacity.
    pub fn send_timeout(&self, value: T, timeout: std::time::Duration) -> SendTimeoutFuture<T> {
        SendTimeoutFuture {
            send: SendFuture {
                shared: self.shared.clone(),
                value: Some(value),
            },
            sleep: crate::time::sleep(timeout),
        }
    }

    /// 尝试立即发送值
    ///
    /// 如果队列已满或接收端已关闭，返回错误
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut guard = self.shared.lock();

        if !guard.inner().receiver_alive {
            return Err(TrySendError::Closed(value));
        }

        let capacity = guard.inner().capacity;
        if capacity.is_some_and(|cap| guard.inner().queue.len() + guard.inner().reserved >= cap) {
            return Err(TrySendError::Full(value));
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
            if capacity.is_some_and(|cap| guard.inner().queue.len() + guard.inner().reserved >= cap)
            {
                guard.wait();
                continue;
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

    /// Waits until the receiver is closed.
    pub fn closed(&self) -> SenderClosedFuture<T> {
        SenderClosedFuture {
            shared: self.shared.clone(),
        }
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

    /// Returns currently available channel capacity.
    pub fn capacity(&self) -> usize {
        let guard = self.shared.lock();
        guard
            .inner()
            .capacity
            .expect("bounded sender has no capacity")
            .saturating_sub(guard.inner().queue.len() + guard.inner().reserved)
    }

    /// Returns the configured maximum capacity.
    pub fn max_capacity(&self) -> usize {
        self.shared
            .lock()
            .inner()
            .capacity
            .expect("bounded sender has no capacity")
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub fn downgrade(&self) -> WeakSender<T> {
        WeakSender {
            shared: Arc::downgrade(&self.shared),
        }
    }

    pub fn strong_count(&self) -> usize {
        self.shared.lock().inner().sender_count
    }

    pub fn weak_count(&self) -> usize {
        Arc::weak_count(&self.shared)
    }

    pub async fn reserve(&self) -> Result<Permit<'_, T>, SendError<()>> {
        std::future::poll_fn(|cx| self.poll_reserve(1, cx)).await?;
        Ok(Permit {
            sender: self,
            permits: 1,
        })
    }

    pub fn try_reserve(&self) -> Result<Permit<'_, T>, TrySendError<()>> {
        self.reserve_now(1)?;
        Ok(Permit {
            sender: self,
            permits: 1,
        })
    }

    pub async fn reserve_many(
        &self,
        permits: usize,
    ) -> Result<PermitIterator<'_, T>, SendError<()>> {
        std::future::poll_fn(|cx| self.poll_reserve(permits, cx)).await?;
        Ok(PermitIterator {
            sender: self,
            remaining: permits,
        })
    }

    pub fn try_reserve_many(
        &self,
        permits: usize,
    ) -> Result<PermitIterator<'_, T>, TrySendError<()>> {
        self.reserve_now(permits)?;
        Ok(PermitIterator {
            sender: self,
            remaining: permits,
        })
    }

    pub async fn reserve_owned(self) -> Result<OwnedPermit<T>, SendError<()>> {
        std::future::poll_fn(|cx| self.poll_reserve(1, cx)).await?;
        Ok(OwnedPermit {
            sender: Some(self),
            permits: 1,
        })
    }

    pub fn try_reserve_owned(self) -> Result<OwnedPermit<T>, TrySendError<Self>> {
        match self.reserve_now(1) {
            Ok(()) => Ok(OwnedPermit {
                sender: Some(self),
                permits: 1,
            }),
            Err(TrySendError::Full(())) => Err(TrySendError::Full(self)),
            Err(TrySendError::Closed(())) => Err(TrySendError::Closed(self)),
        }
    }

    fn reserve_now(&self, permits: usize) -> Result<(), TrySendError<()>> {
        let mut guard = self.shared.lock();
        if !guard.inner().receiver_alive {
            return Err(TrySendError::Closed(()));
        }
        let capacity = guard.inner().capacity.expect("bounded channel");
        if permits > capacity.saturating_sub(guard.inner().queue.len() + guard.inner().reserved) {
            return Err(TrySendError::Full(()));
        }
        guard.inner_mut().reserved += permits;
        Ok(())
    }

    fn poll_reserve(
        &self,
        permits: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), SendError<()>>> {
        let mut guard = self.shared.lock();
        if !guard.inner().receiver_alive {
            return Poll::Ready(Err(SendError(())));
        }
        let capacity = guard.inner().capacity.expect("bounded channel");
        if permits <= capacity.saturating_sub(guard.inner().queue.len() + guard.inner().reserved) {
            guard.inner_mut().reserved += permits;
            return Poll::Ready(Ok(()));
        }
        guard.inner_mut().send_wakers.push_back(cx.waker().clone());
        Poll::Pending
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

        if guard.inner().sender_count == 0
            || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
        {
            // 唤醒等待的接收者
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }
            guard.broadcast();
        }
    }
}

/// A weak bounded sender that does not keep the channel open.
pub struct WeakSender<T> {
    shared: Weak<Shared<T>>,
}

impl<T> WeakSender<T> {
    pub fn upgrade(&self) -> Option<Sender<T>> {
        let shared = self.shared.upgrade()?;
        {
            let mut guard = shared.lock();
            if guard.inner().sender_count == 0 {
                return None;
            }
            guard.inner_mut().sender_count += 1;
        }
        Some(Sender { shared })
    }

    pub fn strong_count(&self) -> usize {
        self.shared
            .upgrade()
            .map_or(0, |shared| shared.lock().inner().sender_count)
    }

    pub fn weak_count(&self) -> usize {
        self.shared.weak_count()
    }
}

impl<T> Clone for WeakSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

/// Reserved capacity tied to a borrowed bounded sender.
pub struct Permit<'a, T> {
    sender: &'a Sender<T>,
    permits: usize,
}

impl<T> Permit<'_, T> {
    pub fn send(mut self, value: T) {
        send_reserved(&self.sender.shared, value);
        self.permits = 0;
    }
}

impl<T> Drop for Permit<'_, T> {
    fn drop(&mut self) {
        release_reserved(&self.sender.shared, self.permits);
    }
}

/// Iterator over several units of reserved bounded-channel capacity.
pub struct PermitIterator<'a, T> {
    sender: &'a Sender<T>,
    remaining: usize,
}

impl<'a, T> Iterator for PermitIterator<'a, T> {
    type Item = Permit<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some(Permit {
            sender: self.sender,
            permits: 1,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T> ExactSizeIterator for PermitIterator<'_, T> {}
impl<T> std::iter::FusedIterator for PermitIterator<'_, T> {}

impl<T> Drop for PermitIterator<'_, T> {
    fn drop(&mut self) {
        release_reserved(&self.sender.shared, self.remaining);
    }
}

/// Reserved capacity that owns its bounded sender.
pub struct OwnedPermit<T> {
    sender: Option<Sender<T>>,
    permits: usize,
}

impl<T> OwnedPermit<T> {
    pub fn send(mut self, value: T) -> Sender<T> {
        let sender = self.sender.take().expect("owned permit missing sender");
        send_reserved(&sender.shared, value);
        self.permits = 0;
        sender
    }

    pub fn release(mut self) -> Sender<T> {
        let sender = self.sender.take().expect("owned permit missing sender");
        release_reserved(&sender.shared, self.permits);
        self.permits = 0;
        sender
    }

    pub fn same_channel(&self, other: &Self) -> bool {
        self.sender
            .as_ref()
            .zip(other.sender.as_ref())
            .is_some_and(|(sender, other)| sender.same_channel(other))
    }

    pub fn same_channel_as_sender(&self, sender: &Sender<T>) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|owned| owned.same_channel(sender))
    }
}

impl<T> Drop for OwnedPermit<T> {
    fn drop(&mut self) {
        if let Some(sender) = &self.sender {
            release_reserved(&sender.shared, self.permits);
        }
    }
}

fn send_reserved<T>(shared: &Arc<Shared<T>>, value: T) {
    let mut guard = shared.lock();
    guard.inner_mut().reserved -= 1;
    guard.inner_mut().queue.push_back(value);
    if let Some(waker) = guard.inner_mut().recv_waker.take() {
        waker.wake();
    }
    guard.broadcast();
}

fn release_reserved<T>(shared: &Arc<Shared<T>>, permits: usize) {
    if permits == 0 {
        return;
    }
    let mut guard = shared.lock();
    guard.inner_mut().reserved -= permits;
    if !guard.inner().receiver_alive && guard.inner().reserved == 0 {
        if let Some(waker) = guard.inner_mut().recv_waker.take() {
            waker.wake();
        }
    }
    while let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
        waker.wake();
    }
    guard.broadcast();
}

fn wake_senders<T>(guard: &mut SharedGuard<'_, T>) {
    // Send futures are cancellation-safe but their stale registrations can
    // remain queued. Waking all registered senders whenever capacity changes
    // guarantees that a cancelled front waiter cannot strand later senders.
    while let Some(waker) = guard.inner_mut().send_wakers.pop_front() {
        waker.wake();
    }
}

fn poll_recv_shared<T>(shared: &Arc<Shared<T>>, cx: &mut Context<'_>) -> Poll<Option<T>> {
    let mut guard = shared.lock();
    if let Some(value) = guard.inner_mut().queue.pop_front() {
        wake_senders(&mut guard);
        guard.broadcast();
        return Poll::Ready(Some(value));
    }
    if guard.inner().sender_count == 0
        || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
    {
        return Poll::Ready(None);
    }
    guard.inner_mut().recv_waker = Some(cx.waker().clone());
    Poll::Pending
}

fn poll_recv_many_shared<T>(
    shared: &Arc<Shared<T>>,
    cx: &mut Context<'_>,
    buffer: &mut Vec<T>,
    limit: usize,
) -> Poll<usize> {
    if limit == 0 {
        return Poll::Ready(0);
    }

    let mut guard = shared.lock();
    let initial_len = buffer.len();
    while buffer.len() - initial_len < limit {
        let Some(value) = guard.inner_mut().queue.pop_front() else {
            break;
        };
        buffer.push(value);
    }
    let received = buffer.len() - initial_len;
    if received != 0 {
        wake_senders(&mut guard);
        guard.broadcast();
        return Poll::Ready(received);
    }
    if guard.inner().sender_count == 0
        || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
    {
        return Poll::Ready(0);
    }
    guard.inner_mut().recv_waker = Some(cx.waker().clone());
    Poll::Pending
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
        if capacity.is_some_and(|cap| guard.inner().queue.len() + guard.inner().reserved >= cap) {
            // 队列已满，保存 waker 并返回 Pending
            guard.inner_mut().send_wakers.push_back(cx.waker().clone());
            return Poll::Pending;
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

/// 超时发送 Future
pub struct SendTimeoutFuture<T> {
    send: SendFuture<T>,
    sleep: crate::time::Sleep,
}

impl<T> Future for SendTimeoutFuture<T> {
    type Output = Result<(), SendTimeoutError<T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `send` is pinned through `self`.
        let this = unsafe { self.get_unchecked_mut() };
        let send = unsafe { Pin::new_unchecked(&mut this.send) };
        match send.poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(SendError(value))) => Poll::Ready(Err(SendTimeoutError::Closed(value))),
            Poll::Pending => match Pin::new(&mut this.sleep).poll(cx) {
                Poll::Ready(()) => {
                    let value = this
                        .send
                        .value
                        .take()
                        .expect("send value missing on timeout");
                    Poll::Ready(Err(SendTimeoutError::Timeout(value)))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

/// Future returned by mpsc sender `closed` methods.
pub struct SenderClosedFuture<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Future for SenderClosedFuture<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut guard = self.shared.lock();
        if !guard.inner().receiver_alive {
            Poll::Ready(())
        } else {
            guard
                .inner_mut()
                .close_waiters
                .push_back(cx.waker().clone());
            Poll::Pending
        }
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

    /// Receives a value, waiting at most `timeout`.
    pub fn recv_timeout(&mut self, timeout: std::time::Duration) -> RecvTimeoutFuture<'_, T> {
        RecvTimeoutFuture {
            receiver: self,
            sleep: crate::time::sleep(timeout),
        }
    }

    /// Receives at least one value and drains up to `limit` available values.
    pub async fn recv_many(&mut self, buffer: &mut Vec<T>, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }

        let initial_len = buffer.len();

        match self.recv().await {
            Some(value) => buffer.push(value),
            None => return 0,
        }

        while buffer.len() - initial_len < limit {
            match self.try_recv() {
                Ok(value) => buffer.push(value),
                Err(_) => break,
            }
        }

        buffer.len() - initial_len
    }

    /// 尝试立即接收值
    ///
    /// 如果队列为空，立即返回错误
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut guard = self.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            wake_senders(&mut guard);
            guard.broadcast();
            Ok(value)
        } else if guard.inner().sender_count == 0
            || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
        {
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
                wake_senders(&mut guard);
                guard.broadcast();
                return Some(value);
            }

            if guard.inner().sender_count == 0
                || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
            {
                return None;
            }

            guard.wait();
        }
    }

    /// Receives at least one value synchronously and drains up to `limit` values.
    pub fn blocking_recv_many(&mut self, buffer: &mut Vec<T>, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        let initial_len = buffer.len();
        let Some(value) = self.blocking_recv() else {
            return 0;
        };
        buffer.push(value);
        while buffer.len() - initial_len < limit {
            match self.try_recv() {
                Ok(value) => buffer.push(value),
                Err(_) => break,
            }
        }
        buffer.len() - initial_len
    }

    /// Polls to receive the next value.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        poll_recv_shared(&self.shared, cx)
    }

    /// Polls to receive at least one and up to `limit` values.
    pub fn poll_recv_many(
        &mut self,
        cx: &mut Context<'_>,
        buffer: &mut Vec<T>,
        limit: usize,
    ) -> Poll<usize> {
        poll_recv_many_shared(&self.shared, cx, buffer, limit)
    }

    /// 关闭接收端
    ///
    /// 这会导致所有后续的发送操作失败
    pub fn close(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        while let Some(waker) = guard.inner_mut().close_waiters.pop_front() {
            waker.wake();
        }

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

    pub fn is_closed(&self) -> bool {
        let guard = self.shared.lock();
        !guard.inner().receiver_alive || guard.inner().sender_count == 0
    }

    pub fn capacity(&self) -> usize {
        let guard = self.shared.lock();
        guard
            .inner()
            .capacity
            .expect("bounded receiver has no capacity")
            .saturating_sub(guard.inner().queue.len() + guard.inner().reserved)
    }

    pub fn max_capacity(&self) -> usize {
        self.shared
            .lock()
            .inner()
            .capacity
            .expect("bounded receiver has no capacity")
    }

    pub fn sender_strong_count(&self) -> usize {
        self.shared.lock().inner().sender_count
    }

    pub fn sender_weak_count(&self) -> usize {
        Arc::weak_count(&self.shared)
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        while let Some(waker) = guard.inner_mut().close_waiters.pop_front() {
            waker.wake();
        }

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
        poll_recv_shared(&self.shared, cx)
    }
}

/// 超时接收 Future
pub struct RecvTimeoutFuture<'a, T> {
    receiver: &'a mut Receiver<T>,
    sleep: crate::time::Sleep,
}

impl<T> Future for RecvTimeoutFuture<'_, T> {
    type Output = Result<T, RecvTimeoutError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let mut guard = this.receiver.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            wake_senders(&mut guard);
            guard.broadcast();
            return Poll::Ready(Ok(value));
        }

        if guard.inner().sender_count == 0
            || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
        {
            return Poll::Ready(Err(RecvTimeoutError::Closed));
        }

        guard.inner_mut().recv_waker = Some(cx.waker().clone());
        drop(guard);
        Pin::new(&mut this.sleep)
            .poll(cx)
            .map(|()| Err(RecvTimeoutError::Timeout))
    }
}

/// 无界 mpsc 超时接收 Future
pub struct UnboundedRecvTimeoutFuture<'a, T> {
    receiver: &'a mut UnboundedReceiver<T>,
    sleep: crate::time::Sleep,
}

impl<T> Future for UnboundedRecvTimeoutFuture<'_, T> {
    type Output = Result<T, RecvTimeoutError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let mut guard = this.receiver.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            guard.broadcast();
            return Poll::Ready(Ok(value));
        }

        if guard.inner().sender_count == 0
            || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
        {
            return Poll::Ready(Err(RecvTimeoutError::Closed));
        }

        guard.inner_mut().recv_waker = Some(cx.waker().clone());
        drop(guard);
        Pin::new(&mut this.sleep)
            .poll(cx)
            .map(|()| Err(RecvTimeoutError::Timeout))
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

    /// Waits until the receiver is closed.
    pub fn closed(&self) -> SenderClosedFuture<T> {
        SenderClosedFuture {
            shared: self.shared.clone(),
        }
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

    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub fn downgrade(&self) -> WeakUnboundedSender<T> {
        WeakUnboundedSender {
            shared: Arc::downgrade(&self.shared),
        }
    }

    pub fn strong_count(&self) -> usize {
        self.shared.lock().inner().sender_count
    }

    pub fn weak_count(&self) -> usize {
        Arc::weak_count(&self.shared)
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

        if guard.inner().sender_count == 0
            || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
        {
            // 唤醒等待的接收者
            if let Some(waker) = guard.inner_mut().recv_waker.take() {
                waker.wake();
            }
            guard.broadcast();
        }
    }
}

/// A weak unbounded sender that does not keep the channel open.
pub struct WeakUnboundedSender<T> {
    shared: Weak<Shared<T>>,
}

impl<T> WeakUnboundedSender<T> {
    pub fn upgrade(&self) -> Option<UnboundedSender<T>> {
        let shared = self.shared.upgrade()?;
        {
            let mut guard = shared.lock();
            if guard.inner().sender_count == 0 {
                return None;
            }
            guard.inner_mut().sender_count += 1;
        }
        Some(UnboundedSender { shared })
    }

    pub fn strong_count(&self) -> usize {
        self.shared
            .upgrade()
            .map_or(0, |shared| shared.lock().inner().sender_count)
    }

    pub fn weak_count(&self) -> usize {
        self.shared.weak_count()
    }
}

impl<T> Clone for WeakUnboundedSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
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

    /// Receives a value, waiting at most `timeout`.
    pub fn recv_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> UnboundedRecvTimeoutFuture<'_, T> {
        UnboundedRecvTimeoutFuture {
            receiver: self,
            sleep: crate::time::sleep(timeout),
        }
    }

    /// Receives at least one value and drains up to `limit` available values.
    pub async fn recv_many(&mut self, buffer: &mut Vec<T>, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }

        let initial_len = buffer.len();

        match self.recv().await {
            Some(value) => buffer.push(value),
            None => return 0,
        }

        while buffer.len() - initial_len < limit {
            match self.try_recv() {
                Ok(value) => buffer.push(value),
                Err(_) => break,
            }
        }

        buffer.len() - initial_len
    }

    /// 尝试立即接收值
    ///
    /// 如果队列为空，立即返回错误
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut guard = self.shared.lock();

        if let Some(value) = guard.inner_mut().queue.pop_front() {
            guard.broadcast();
            Ok(value)
        } else if guard.inner().sender_count == 0
            || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
        {
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

            if guard.inner().sender_count == 0
                || (!guard.inner().receiver_alive && guard.inner().reserved == 0)
            {
                return None;
            }

            guard.wait();
        }
    }

    /// Receives at least one value synchronously and drains up to `limit` values.
    pub fn blocking_recv_many(&mut self, buffer: &mut Vec<T>, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        let initial_len = buffer.len();
        let Some(value) = self.blocking_recv() else {
            return 0;
        };
        buffer.push(value);
        while buffer.len() - initial_len < limit {
            match self.try_recv() {
                Ok(value) => buffer.push(value),
                Err(_) => break,
            }
        }
        buffer.len() - initial_len
    }

    /// Polls to receive the next value.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        poll_recv_shared(&self.shared, cx)
    }

    /// Polls to receive at least one and up to `limit` values.
    pub fn poll_recv_many(
        &mut self,
        cx: &mut Context<'_>,
        buffer: &mut Vec<T>,
        limit: usize,
    ) -> Poll<usize> {
        poll_recv_many_shared(&self.shared, cx, buffer, limit)
    }

    /// 关闭接收端
    ///
    /// 这会导致所有后续的发送操作失败
    pub fn close(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        while let Some(waker) = guard.inner_mut().close_waiters.pop_front() {
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

    pub fn is_closed(&self) -> bool {
        let guard = self.shared.lock();
        !guard.inner().receiver_alive || guard.inner().sender_count == 0
    }

    pub fn sender_strong_count(&self) -> usize {
        self.shared.lock().inner().sender_count
    }

    pub fn sender_weak_count(&self) -> usize {
        Arc::weak_count(&self.shared)
    }
}

impl<T> Drop for UnboundedReceiver<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.lock();
        guard.inner_mut().receiver_alive = false;
        while let Some(waker) = guard.inner_mut().close_waiters.pop_front() {
            waker.wake();
        }
        guard.broadcast();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn recv_timeout_empty() {
        let (_tx, mut rx) = channel::<i32>(1);
        let result = crate::Runtime::new()
            .unwrap()
            .block_on(async move { rx.recv_timeout(Duration::from_millis(1)).await });
        assert!(matches!(result, Err(RecvTimeoutError::Timeout)));
    }

    #[test]
    fn send_timeout_full() {
        let (tx, mut rx) = channel::<i32>(1);
        tx.try_send(1).unwrap();
        let result = crate::Runtime::new().unwrap().block_on(async move {
            let send = tx.send_timeout(2, Duration::from_millis(1)).await;
            let _ = rx.try_recv();
            send
        });
        assert!(matches!(result, Err(SendTimeoutError::Timeout(_))));
    }

    #[test]
    fn recv_many_drains() {
        let (tx, mut rx) = channel::<i32>(10);
        for value in 0..3 {
            tx.try_send(value).unwrap();
        }

        let result = crate::Runtime::new().unwrap().block_on(async move {
            let mut buf = Vec::new();
            let n = rx.recv_many(&mut buf, 10).await;
            assert_eq!(n, 3);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

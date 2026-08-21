use super::WakerState;
use crate::signal::oneshot;
use crate::{RuntimeError, create_waker};
use crate::{Task, TaskAttr};
use ffrt_sys::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// FFRT Runtime
#[derive(Clone, Copy, Debug, Default)]
pub struct Runtime;

impl Runtime {
    pub fn new() -> Self {
        Self
    }

    /// Block the current thread and run future until it is ready
    pub fn block_on<F>(&self, future: F) -> Result<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let task = Task::default();
        task.submit(move || {
            let output = poll_once(future);
            let _ = tx.send(output);
        });

        rx.blocking_recv()
            .map_err(|_| RuntimeError::Other("Task failed".to_string()))
    }

    /// Spawn a new task on the runtime
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let task = Task::default();
        task.submit(move || {
            let output = poll_once(future);
            let _ = tx.send(output);
        });

        JoinHandle {
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn a new task with specified task attributes
    pub fn spawn_with_attr<F>(&self, attr: TaskAttr, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let task = Task::new(attr);
        task.submit(move || {
            let output = poll_once(future);
            let _ = tx.send(output);
        });

        JoinHandle {
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn a blocking closure on the runtime.
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let task = Task::default();
        task.submit(move || {
            let _ = tx.send(func());
        });

        JoinHandle {
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns a handle to this runtime.
    pub fn handle(&self) -> Handle {
        Handle(*self)
    }
}

/// An owned handle to an FFRT runtime.
#[derive(Clone, Copy, Debug, Default)]
pub struct Handle(Runtime);

impl Handle {
    /// Returns a handle for the active default runtime.
    pub fn current() -> Self {
        let runtime = super::active_runtime()
            .read()
            .ok()
            .and_then(|rt| rt.as_ref().copied())
            .expect("Access FFRT runtime failed in Handle::current");
        Self(runtime)
    }

    /// Run a future on the runtime.
    pub fn block_on<F>(&self, future: F) -> Result<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.0.block_on(future)
    }

    /// Spawn a future on the runtime.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.0.spawn(future)
    }

    /// Spawn a blocking closure on the runtime.
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.0.spawn_blocking(func)
    }
}

/// A minimal tokio-style runtime builder.
///
/// FFRT does not need a custom thread-pool configuration, so this builder is
/// intentionally tiny and always produces the default [`Runtime`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Builder;

impl Builder {
    /// Create a builder for a multi-threaded runtime.
    pub fn new_multi_thread() -> Self {
        Self
    }

    /// Create a builder for a current-thread runtime.
    pub fn new_current_thread() -> Self {
        Self
    }

    /// Enable all available runtime features. No-op for FFRT.
    pub fn enable_all(self) -> Self {
        self
    }

    /// Enable I/O support. No-op for FFRT.
    pub fn enable_io(self) -> Self {
        self
    }

    /// Enable timers. No-op for FFRT.
    pub fn enable_time(self) -> Self {
        self
    }

    /// Configure the number of worker threads. No-op for FFRT.
    pub fn worker_threads(self, _count: usize) -> Self {
        self
    }

    /// Configure the maximum number of blocking threads. No-op for FFRT.
    pub fn max_blocking_threads(self, _count: usize) -> Self {
        self
    }

    /// Set the worker thread name. No-op for FFRT.
    pub fn thread_name<N: Into<String>>(self, _name: N) -> Self {
        self
    }

    /// Set the worker thread stack size. No-op for FFRT.
    pub fn thread_stack_size(self, _size: usize) -> Self {
        self
    }

    /// Build the runtime.
    pub fn build(self) -> std::io::Result<Runtime> {
        Ok(Runtime::new())
    }
}

/// Poll future until it is ready
fn poll_once<F: Future>(mut future: F) -> F::Output {
    let mut future = unsafe { Pin::new_unchecked(&mut future) };

    // Create a waker based on FFRT condition variable
    let waker_state = Arc::new(WakerState::new());
    let waker = create_waker(waker_state.clone());
    let mut cx = Context::from_waker(&waker);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }

        waker_state.wait();
    }
}

/// JoinHandle for a task
pub struct JoinHandle<T> {
    rx: oneshot::Receiver<T>,
    cancelled: Arc<AtomicBool>,
}

impl<T> JoinHandle<T> {
    /// Wait for the task to complete
    pub fn join(self) -> JoinFuture<T> {
        JoinFuture { handle: Some(self) }
    }

    /// Check if the task is finished
    pub fn is_finished(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || self.rx.is_finished()
    }

    /// Abort the task.
    ///
    /// The underlying FFRT work item is not forcibly preempted, but join/future
    /// consumers observe cancellation immediately.
    pub fn abort(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.rx.wake_waiter();
    }
}

/// Future returned by [`JoinHandle::join`].
#[must_use = "futures do nothing unless polled"]
pub struct JoinFuture<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> Future for JoinFuture<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        let handle = this
            .handle
            .as_mut()
            .expect("JoinFuture polled after completion");

        if handle.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(Err(RuntimeError::Cancelled));
        }

        match Pin::new(&mut handle.rx).poll(cx) {
            Poll::Ready(Ok(value)) => {
                this.handle = None;
                Poll::Ready(Ok(value))
            }
            Poll::Ready(Err(_)) => {
                this.handle = None;
                Poll::Ready(Err(RuntimeError::Other("Task failed".to_string())))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(Err(RuntimeError::Cancelled));
        }

        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
            Poll::Ready(Err(_)) => Poll::Ready(Err(RuntimeError::Other("Task failed".to_string()))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Yield the current task's execution
pub async fn yield_now() {
    struct YieldNow {
        yielded: bool,
    }

    impl Future for YieldNow {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    YieldNow { yielded: false }.await
}

/// Async sleep
pub async fn sleep(duration: Duration) {
    struct Sleep {
        deadline: Instant,
    }

    impl Future for Sleep {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if Instant::now() >= self.deadline {
                Poll::Ready(())
            } else {
                let remaining = self.deadline - Instant::now();
                unsafe {
                    ffrt_usleep(remaining.as_micros() as u64);
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    Sleep {
        deadline: Instant::now() + duration,
    }
    .await
}

/// Run a future on the active runtime.
pub fn block_on<F>(future: F) -> Result<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    super::active_runtime()
        .read()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.block_on(future)))
        .expect("Access FFRT runtime failed in block_on")
}

/// Spawn a future on the active runtime.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    super::active_runtime()
        .read()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.spawn(future)))
        .expect("Access FFRT runtime failed in spawn")
}

/// Spawn a future with the supplied task attributes on the active runtime.
pub fn spawn_with_attr<F>(attr: TaskAttr, future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    super::active_runtime()
        .read()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.spawn_with_attr(attr, future)))
        .expect("Access FFRT runtime failed in spawn_with_attr")
}

/// Spawn a blocking closure on the active runtime.
pub fn spawn_blocking<F, R>(func: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    super::active_runtime()
        .read()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.spawn_blocking(func)))
        .expect("Access FFRT runtime failed in spawn_blocking")
}

/// Run a blocking closure on the current thread.
///
/// FFRT worker threads are native threads, so this simply runs the closure
/// synchronously. It is provided as a tokio-compatible convenience.
pub fn block_in_place<F, R>(func: F) -> R
where
    F: FnOnce() -> R,
{
    func()
}

/// Wait for all submitted tasks to complete
pub fn wait_all() {
    unsafe {
        ffrt_wait();
    }
}

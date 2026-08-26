// The OHOS target's `thread_local!` expansion triggers this lint even though
// the user-visible initializer is already const.
#![allow(clippy::missing_const_for_thread_local)]

use super::WakerState;
use super::trace::TaskTrace;
use crate::signal::oneshot;
use crate::{JoinError, create_waker};
use crate::{Task, TaskAttr};
use ffrt_sys::ffrt_wait;
use std::cell::Cell;
use std::future::Future;
use std::panic::{AssertUnwindSafe, Location, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, JoinError>;

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_TASKS: AtomicUsize = AtomicUsize::new(0);

std::thread_local! {
    static CURRENT_TASK_ID: Cell<Option<Id>> = const { Cell::new(None) };
}

pub(crate) struct CancellationState {
    cancelled: AtomicBool,
    finished: AtomicBool,
    waker: Mutex<Option<Waker>>,
    pub(crate) id: Id,
}

impl CancellationState {
    pub(crate) fn new() -> Arc<Self> {
        ACTIVE_TASKS.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            waker: Mutex::new(None),
            id: Id::next(),
        })
    }

    pub(crate) fn abort(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub(crate) fn register(&self, waker: &Waker) {
        let mut slot = self.waker.lock().unwrap();
        if slot
            .as_ref()
            .is_none_or(|registered| !registered.will_wake(waker))
        {
            *slot = Some(waker.clone());
        }
    }

    pub(crate) fn finish(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            ACTIVE_TASKS.fetch_sub(1, Ordering::Relaxed);
        }
        self.waker.lock().unwrap().take();
    }
}

impl Drop for CancellationState {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Acquire) {
            ACTIVE_TASKS.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl std::fmt::Debug for CancellationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationState")
            .field("cancelled", &self.is_cancelled())
            .field("finished", &self.is_finished())
            .field("id", &self.id)
            .finish()
    }
}

/// FFRT Runtime
#[derive(Debug, Default)]
pub struct Runtime;

impl Runtime {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    /// Block the current thread and run future until it is ready
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        poll_blocking(future)
    }

    /// Spawn a new task on the runtime
    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_named(future, None)
    }

    #[track_caller]
    pub(crate) fn spawn_named<F>(&self, future: F, name: Option<&str>) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_task(Task::default(), future, name, Location::caller())
    }

    /// Spawn a new task with specified task attributes
    #[track_caller]
    pub fn spawn_with_attr<F>(&self, attr: TaskAttr, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let name = attr.get_name().to_owned();
        self.spawn_task(Task::new(attr), future, Some(&name), Location::caller())
    }

    /// Spawn a blocking closure on the runtime.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_blocking_named(func, None)
    }

    #[track_caller]
    pub(crate) fn spawn_blocking_named<F, R>(&self, func: F, name: Option<&str>) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let state = CancellationState::new();
        let runner_state = state.clone();
        let trace = TaskTrace::new(
            "blocking",
            name,
            state.id,
            std::mem::size_of::<F>(),
            Some(std::any::type_name::<F>()),
            Location::caller(),
        );
        let task = Task::default();
        task.submit(move || {
            let _task_id = CurrentTaskIdGuard::enter(runner_state.id);
            // Tokio-compatible behavior: blocking work can only be cancelled
            // before it starts. Once the closure begins, abort has no effect.
            let output = trace.in_scope(|| {
                if runner_state.is_cancelled() {
                    Err(JoinError::cancelled(runner_state.id))
                } else {
                    catch_unwind(AssertUnwindSafe(func))
                        .map_err(|payload| panic_error(runner_state.id, payload))
                }
            });
            runner_state.finish();
            let _ = tx.send(output);
        });

        JoinHandle { rx, state }
    }

    /// Returns a handle to this runtime.
    pub fn handle(&self) -> &Handle {
        static HANDLE: Handle = Handle;
        &HANDLE
    }

    /// Enter this runtime context on the current thread.
    pub fn enter(&self) -> EnterGuard<'_> {
        self.handle().enter()
    }

    /// Returns process-wide FFRT runtime metrics.
    pub fn metrics(&self) -> RuntimeMetrics {
        self.handle().metrics()
    }

    /// Shut down the runtime after waiting for at most `duration`.
    ///
    /// FFRT owns the worker pool for the lifetime of the process, so dropping
    /// this lightweight runtime handle never tears down system workers.
    pub fn shutdown_timeout(self, _duration: Duration) {}

    /// Shut down without waiting for outstanding FFRT work.
    pub fn shutdown_background(self) {}

    fn spawn_task<F>(
        &self,
        task: Task,
        future: F,
        name: Option<&str>,
        location: &'static Location<'static>,
    ) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let state = CancellationState::new();
        let runner_state = state.clone();
        let trace = TaskTrace::new(
            "task",
            name,
            state.id,
            std::mem::size_of::<F>(),
            None,
            location,
        );
        task.submit(move || {
            let output = catch_unwind(AssertUnwindSafe(|| {
                poll_once(future, Some(&runner_state), &trace)
            }))
            .unwrap_or_else(|panic| Err(panic_error(runner_state.id, panic)));
            // poll_once has dropped the future before completion is published.
            runner_state.finish();
            let _ = tx.send(output);
        });
        JoinHandle { rx, state }
    }
}

/// An owned handle to an FFRT runtime.
#[derive(Clone, Debug, Default)]
pub struct Handle;

/// Scheduler flavor reported for the process-wide FFRT executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeFlavor {
    CurrentThread,
    MultiThread,
}

impl Handle {
    /// Returns a handle for the active default runtime.
    pub fn current() -> Self {
        Self::try_current().expect("there is no FFRT runtime context")
    }

    /// Returns a handle when the process-wide FFRT runtime is available.
    pub fn try_current() -> std::result::Result<Self, TryCurrentError> {
        let available = super::active_runtime()
            .lock()
            .map(|runtime| runtime.is_some())
            .unwrap_or(false);
        available.then_some(Self).ok_or(TryCurrentError(()))
    }

    /// Enter this runtime context on the current thread.
    pub fn enter(&self) -> EnterGuard<'_> {
        EnterGuard { _handle: self }
    }

    /// Run a future on the runtime.
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        poll_blocking(future)
    }

    /// Spawn a future on the runtime.
    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Runtime.spawn_named(future, None)
    }

    #[track_caller]
    #[cfg(feature = "tracing")]
    pub(crate) fn spawn_named<F>(&self, future: F, name: Option<&str>) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Runtime.spawn_named(future, name)
    }

    /// Spawn a blocking closure on the runtime.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        Runtime.spawn_blocking_named(func, None)
    }

    #[track_caller]
    #[cfg(feature = "tracing")]
    pub(crate) fn spawn_blocking_named<F, R>(&self, func: F, name: Option<&str>) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        Runtime.spawn_blocking_named(func, name)
    }

    /// FFRT schedules work across its process-wide worker pool.
    pub fn runtime_flavor(&self) -> RuntimeFlavor {
        RuntimeFlavor::MultiThread
    }

    /// Returns the identifier of the process-wide FFRT runtime.
    pub fn id(&self) -> Id {
        static RUNTIME_ID: std::sync::OnceLock<Id> = std::sync::OnceLock::new();
        *RUNTIME_ID.get_or_init(Id::next)
    }

    /// FFRT owns the underlying runtime and does not expose a configurable name.
    pub fn name(&self) -> Option<&str> {
        None
    }

    /// Returns process-wide FFRT runtime metrics.
    pub fn metrics(&self) -> RuntimeMetrics {
        RuntimeMetrics
    }
}

/// Stable runtime metrics available from the process-wide FFRT executor.
#[derive(Clone, Debug)]
pub struct RuntimeMetrics;

impl RuntimeMetrics {
    /// Returns the number of logical FFRT workers visible to the process.
    pub fn num_workers(&self) -> usize {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
    }

    /// Returns the number of spawned tasks that have not completed.
    pub fn num_alive_tasks(&self) -> usize {
        ACTIVE_TASKS.load(Ordering::Relaxed)
    }

    /// FFRT does not expose its global queue depth.
    pub fn global_queue_depth(&self) -> usize {
        0
    }
}

/// Error returned by [`Handle::try_current`] outside an FFRT runtime context.
#[derive(Clone, Copy, Debug)]
pub struct TryCurrentError(());

impl std::fmt::Display for TryCurrentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("there is no FFRT runtime context")
    }
}

impl std::error::Error for TryCurrentError {}

impl TryCurrentError {
    pub fn is_missing_context(&self) -> bool {
        true
    }

    pub fn is_thread_local_destroyed(&self) -> bool {
        false
    }
}

/// Guard returned by [`Runtime::enter`] and [`Handle::enter`].
#[must_use]
#[derive(Debug)]
pub struct EnterGuard<'a> {
    _handle: &'a Handle,
}

/// A minimal tokio-style runtime builder.
///
/// FFRT does not need a custom thread-pool configuration, so this builder is
/// intentionally tiny and always produces the default [`Runtime`].
#[derive(Clone, Debug, Default)]
pub struct Builder {
    worker_threads: Option<usize>,
    max_blocking_threads: Option<usize>,
    thread_name: Option<String>,
    thread_stack_size: Option<usize>,
    thread_keep_alive: Option<Duration>,
}

impl Builder {
    /// Create a builder for a multi-threaded runtime.
    pub fn new_multi_thread() -> Self {
        Self::default()
    }

    /// Create a builder for a current-thread runtime.
    pub fn new_current_thread() -> Self {
        Self::default()
    }

    /// Enable all available runtime features. No-op for FFRT.
    pub fn enable_all(&mut self) -> &mut Self {
        self
    }

    /// Enable I/O support. No-op for FFRT.
    pub fn enable_io(&mut self) -> &mut Self {
        self
    }

    /// Enable timers. No-op for FFRT.
    pub fn enable_time(&mut self) -> &mut Self {
        self
    }

    /// Configure the number of worker threads. No-op for FFRT.
    pub fn worker_threads(&mut self, count: usize) -> &mut Self {
        assert!(count > 0, "Worker threads must be greater than 0");
        self.worker_threads = Some(count);
        self
    }

    /// Configure the maximum number of blocking threads. No-op for FFRT.
    pub fn max_blocking_threads(&mut self, count: usize) -> &mut Self {
        assert!(count > 0, "Max blocking threads must be greater than 0");
        self.max_blocking_threads = Some(count);
        self
    }

    /// Set the worker thread name. No-op for FFRT.
    pub fn thread_name<N: Into<String>>(&mut self, name: N) -> &mut Self {
        self.thread_name = Some(name.into());
        self
    }

    /// Sets the logical runtime name. FFRT owns the process-wide runtime.
    pub fn name<N: Into<String>>(&mut self, name: N) -> &mut Self {
        self.thread_name = Some(name.into());
        self
    }

    /// Set a callback that provides worker thread names. FFRT owns naming.
    pub fn thread_name_fn<F>(&mut self, _name_fn: F) -> &mut Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        self
    }

    /// Set the worker thread stack size. No-op for FFRT.
    pub fn thread_stack_size(&mut self, size: usize) -> &mut Self {
        self.thread_stack_size = Some(size);
        self
    }

    /// Set how long idle blocking workers are retained. FFRT owns retention.
    pub fn thread_keep_alive(&mut self, duration: Duration) -> &mut Self {
        self.thread_keep_alive = Some(duration);
        self
    }

    pub fn on_thread_start<F>(&mut self, _callback: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self
    }

    pub fn on_thread_stop<F>(&mut self, _callback: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self
    }

    pub fn on_thread_park<F>(&mut self, _callback: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self
    }

    pub fn on_thread_unpark<F>(&mut self, _callback: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self
    }

    /// Configures how often the global queue is checked. FFRT owns this policy.
    pub fn global_queue_interval(&mut self, value: u32) -> &mut Self {
        assert!(value > 0, "global_queue_interval must be greater than 0");
        self
    }

    /// Configures the scheduler event interval. FFRT owns this policy.
    pub fn event_interval(&mut self, value: u32) -> &mut Self {
        assert!(value > 0, "event_interval must be greater than 0");
        self
    }

    /// Configures the I/O event batch size. FFRT owns the loop dispatch policy.
    pub fn max_io_events_per_tick(&mut self, capacity: usize) -> &mut Self {
        assert!(
            capacity > 0,
            "max_io_events_per_tick must be greater than 0"
        );
        self
    }

    /// Build the runtime.
    pub fn build(&mut self) -> std::io::Result<Runtime> {
        Runtime::new()
    }
}

fn poll_blocking<F: Future>(mut future: F) -> F::Output {
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    let waker_state = Arc::new(WakerState::new());
    let waker = create_waker(waker_state.clone(), None);
    let mut cx = Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => waker_state.wait(),
        }
    }
}

/// Polls a future until it is ready or cooperatively cancelled.
fn poll_once<F: Future>(
    mut future: F,
    cancellation: Option<&CancellationState>,
    trace: &TaskTrace,
) -> Result<F::Output> {
    let _task_id = cancellation.map(|state| CurrentTaskIdGuard::enter(state.id));
    let mut future = unsafe { Pin::new_unchecked(&mut future) };

    // Create a waker based on FFRT condition variable
    let waker_state = Arc::new(WakerState::new());
    let waker = create_waker(waker_state.clone(), Some(trace.clone()));
    let mut cx = Context::from_waker(&waker);
    if let Some(cancellation) = cancellation {
        cancellation.register(&waker);
    }

    loop {
        if cancellation.is_some_and(CancellationState::is_cancelled) {
            return Err(JoinError::cancelled(
                cancellation.expect("state missing").id,
            ));
        }

        if let Poll::Ready(output) = trace.in_scope(|| future.as_mut().poll(&mut cx)) {
            return Ok(output);
        }

        if cancellation.is_some_and(CancellationState::is_cancelled) {
            return Err(JoinError::cancelled(
                cancellation.expect("state missing").id,
            ));
        }

        waker_state.wait();
    }
}

pub(crate) fn panic_error(id: Id, payload: Box<dyn std::any::Any + Send>) -> JoinError {
    JoinError::panic(id, payload)
}

/// A unique identifier for a spawned task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(u64);

impl Id {
    pub(crate) fn next() -> Self {
        Self(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[cfg(feature = "tracing")]
    pub(crate) const fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Returns the identifier of the currently running task.
pub fn id() -> Id {
    try_id().expect("task::id() called outside a task")
}

/// Returns the identifier of the currently running task, when available.
pub fn try_id() -> Option<Id> {
    CURRENT_TASK_ID.with(Cell::get)
}

struct CurrentTaskIdGuard {
    previous: Option<Id>,
}

impl CurrentTaskIdGuard {
    fn enter(id: Id) -> Self {
        let previous = CURRENT_TASK_ID.with(|current| current.replace(Some(id)));
        Self { previous }
    }
}

impl Drop for CurrentTaskIdGuard {
    fn drop(&mut self) {
        CURRENT_TASK_ID.with(|current| current.set(self.previous));
    }
}

/// A handle that can abort a spawned task.
#[derive(Clone, Debug)]
pub struct AbortHandle {
    pub(crate) state: Arc<CancellationState>,
}

impl AbortHandle {
    pub(crate) fn new(state: Arc<CancellationState>) -> Self {
        Self { state }
    }

    /// Aborts the task associated with this handle.
    pub fn abort(&self) {
        self.state.abort();
    }

    /// Returns `true` if the task has been aborted.
    pub fn is_aborted(&self) -> bool {
        self.state.is_cancelled()
    }

    /// Returns `true` once the task has stopped and its future was dropped.
    pub fn is_finished(&self) -> bool {
        self.state.is_finished()
    }

    /// Returns the ID of the associated task.
    pub fn id(&self) -> Id {
        self.state.id
    }
}

/// JoinHandle for a task
pub struct JoinHandle<T> {
    rx: oneshot::Receiver<Result<T>>,
    state: Arc<CancellationState>,
}

pub(crate) fn local_join_channel<T>() -> (oneshot::Sender<Result<T>>, JoinHandle<T>) {
    let (tx, rx) = oneshot::channel();
    let state = CancellationState::new();
    (tx, JoinHandle { rx, state })
}

impl<T> JoinHandle<T> {
    /// Wait for the task to complete
    pub fn join(self) -> JoinFuture<T> {
        JoinFuture { handle: Some(self) }
    }

    /// Check if the task is finished
    pub fn is_finished(&self) -> bool {
        self.state.is_finished()
    }

    /// Abort the task.
    ///
    /// Async tasks stop at the next poll boundary. Blocking tasks can only be
    /// prevented from starting; once running they complete normally.
    pub fn abort(&self) {
        self.state.abort();
    }

    /// Returns a handle that can be used to abort the task remotely.
    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle::new(self.state.clone())
    }

    /// Returns the task ID.
    pub fn id(&self) -> Id {
        self.state.id
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
        let id = handle.state.id;

        match Pin::new(&mut handle.rx).poll(cx) {
            Poll::Ready(Ok(output)) => {
                this.handle = None;
                Poll::Ready(output)
            }
            Poll::Ready(Err(_)) => {
                this.handle = None;
                Poll::Ready(Err(JoinError::other(id, "task result channel closed")))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(output)) => Poll::Ready(output),
            Poll::Ready(Err(_)) => Poll::Ready(Err(JoinError::other(
                self.state.id,
                "task result channel closed",
            ))),
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

/// Creates a non-blocking FFRT loop sleep future.
pub fn sleep(duration: Duration) -> crate::time::Sleep {
    crate::time::sleep(duration)
}

/// Run a future on the active runtime.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    let available = super::active_runtime()
        .lock()
        .map(|runtime| runtime.is_some())
        .unwrap_or(false);
    assert!(available, "Access FFRT runtime failed in block_on");
    poll_blocking(future)
}

/// Spawn a future on the active runtime.
#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    super::active_runtime()
        .lock()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.spawn(future)))
        .expect("Access FFRT runtime failed in spawn")
}

/// Spawn a future with the supplied task attributes on the active runtime.
#[track_caller]
pub fn spawn_with_attr<F>(attr: TaskAttr, future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    super::active_runtime()
        .lock()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.spawn_with_attr(attr, future)))
        .expect("Access FFRT runtime failed in spawn_with_attr")
}

/// Spawn a blocking closure on the active runtime.
#[track_caller]
pub fn spawn_blocking<F, R>(func: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    super::active_runtime()
        .lock()
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

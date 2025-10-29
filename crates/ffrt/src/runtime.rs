//! 异步运行时 - Tokio风格的FFRT运行时
//!
//! 这个模块提供了一个类似Tokio的异步运行时，基于鸿蒙的FFRT框架。
//! 支持future执行、任务生成、超时、channel等异步操作。

use ohos_ffrt_sys::*;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// 运行时错误类型
#[derive(Debug, Clone)]
pub enum RuntimeError {
    /// 任务被取消
    Cancelled,
    /// 任务panic
    Panicked(String),
    /// 超时
    Timeout,
    /// 其他错误
    Other(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Cancelled => write!(f, "Task cancelled"),
            RuntimeError::Panicked(msg) => write!(f, "Task panicked: {}", msg),
            RuntimeError::Timeout => write!(f, "Operation timeout"),
            RuntimeError::Other(msg) => write!(f, "Runtime error: {}", msg),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<std::io::Error> for RuntimeError {
    fn from(error: std::io::Error) -> Self {
        RuntimeError::Other(error.to_string())
    }
}

/// 运行时Result类型
pub type Result<T> = std::result::Result<T, RuntimeError>;

/// 任务ID生成器
static TASK_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn next_task_id() -> usize {
    TASK_ID_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// 类型擦除的Future包装
type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    /// 待运行
    Pending,
    /// 运行中
    Running,
    /// 已完成
    Completed,
    /// 已取消
    Cancelled,
}

/// FFRT任务包装器
struct FfrtTaskWrapper {
    #[allow(dead_code)]
    task_id: usize,
    future: StdMutex<Option<BoxFuture>>,
    result: StdMutex<Option<()>>,
    state: AtomicUsize, // TaskState as usize
    cancelled: AtomicBool,
    #[allow(dead_code)]
    wakers: StdMutex<VecDeque<Waker>>,
}

impl FfrtTaskWrapper {
    fn new(future: BoxFuture) -> Arc<Self> {
        Arc::new(Self {
            task_id: next_task_id(),
            future: StdMutex::new(Some(future)),
            result: StdMutex::new(None),
            state: AtomicUsize::new(TaskState::Pending as usize),
            cancelled: AtomicBool::new(false),
            wakers: StdMutex::new(VecDeque::new()),
        })
    }

    fn get_state(&self) -> TaskState {
        match self.state.load(Ordering::Relaxed) {
            0 => TaskState::Pending,
            1 => TaskState::Running,
            2 => TaskState::Completed,
            3 => TaskState::Cancelled,
            _ => TaskState::Pending,
        }
    }

    fn set_state(&self, state: TaskState) {
        self.state.store(state as usize, Ordering::Release);
    }

    fn poll(&self) -> bool {
        // 检查是否被取消
        if self.cancelled.load(Ordering::Relaxed) {
            self.set_state(TaskState::Cancelled);
            return true;
        }

        self.set_state(TaskState::Running);
        let waker = self.create_waker();
        let mut cx = Context::from_waker(&waker);

        let mut future_opt = self.future.lock().unwrap();
        if let Some(mut future) = future_opt.take() {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(&mut cx)
            })) {
                Ok(Poll::Ready(_)) => {
                    *self.result.lock().unwrap() = Some(());
                    self.set_state(TaskState::Completed);
                    true
                }
                Ok(Poll::Pending) => {
                    *future_opt = Some(future);
                    self.set_state(TaskState::Pending);
                    false
                }
                Err(_) => {
                    self.set_state(TaskState::Cancelled);
                    true
                }
            }
        } else {
            self.set_state(TaskState::Completed);
            true
        }
    }

    fn create_waker(&self) -> Waker {
        use std::task::{RawWaker, RawWakerVTable};

        unsafe fn clone_raw(data: *const ()) -> RawWaker {
            let wrapper = unsafe { Arc::from_raw(data as *const FfrtTaskWrapper) };
            let cloned = wrapper.clone();
            std::mem::forget(wrapper);
            std::mem::forget(cloned.clone());
            RawWaker::new(Arc::into_raw(cloned) as *const (), &VTABLE)
        }

        unsafe fn wake_raw(data: *const ()) {
            let wrapper = unsafe { Arc::from_raw(data as *const FfrtTaskWrapper) };
            submit_task_to_ffrt(wrapper.clone());
        }

        unsafe fn wake_by_ref_raw(data: *const ()) {
            let wrapper = unsafe { Arc::from_raw(data as *const FfrtTaskWrapper) };
            submit_task_to_ffrt(wrapper.clone());
            std::mem::forget(wrapper);
        }

        unsafe fn drop_raw(data: *const ()) {
            drop(unsafe { Arc::from_raw(data as *const FfrtTaskWrapper) });
        }

        static VTABLE: RawWakerVTable =
            RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

        let wrapper_ptr = self as *const Self;
        unsafe {
            Arc::increment_strong_count(wrapper_ptr);
            let raw = RawWaker::new(wrapper_ptr as *const (), &VTABLE);
            Waker::from_raw(raw)
        }
    }

    #[allow(dead_code)]
    fn is_completed(&self) -> bool {
        matches!(
            self.get_state(),
            TaskState::Completed | TaskState::Cancelled
        )
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// 提交任务到FFRT
fn submit_task_to_ffrt(wrapper: Arc<FfrtTaskWrapper>) {
    extern "C" fn task_fn(arg: *mut std::ffi::c_void) {
        let wrapper = unsafe {
            let ptr = arg as *const FfrtTaskWrapper;
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        };

        wrapper.poll();
    }

    extern "C" fn cleanup_fn(_arg: *mut std::ffi::c_void) {
        // 清理函数
    }

    unsafe {
        let mut attr = std::mem::MaybeUninit::<FfrtTaskAttr>::uninit();
        ffrt_task_attr_init(attr.as_mut_ptr());
        ffrt_task_attr_set_qos(attr.as_mut_ptr(), FfrtQos::Default);

        // 增加引用计数，传递给FFRT
        let wrapper_ptr = Arc::into_raw(wrapper) as *mut std::ffi::c_void;

        ffrt_submit_base(
            task_fn,
            cleanup_fn,
            wrapper_ptr,
            std::ptr::null(),
            std::ptr::null(),
            attr.as_ptr(),
        );

        ffrt_task_attr_destroy(attr.as_mut_ptr());
    }
}

/// 全局运行时实例
#[allow(dead_code)]
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// 异步运行时
#[derive(Clone)]
pub struct Runtime {
    _inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    // 运行时特定的数据可以放在这里
}

impl Runtime {
    /// 创建新的运行时
    pub fn new() -> Self {
        Self {
            _inner: Arc::new(RuntimeInner {}),
        }
    }

    /// 获取全局运行时实例
    pub fn global() -> Self {
        Self {
            _inner: Arc::new(RuntimeInner {}),
        }
    }

    /// 在运行时上执行future直到完成
    pub fn block_on<F>(&self, future: F) -> Result<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(StdMutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().unwrap() = Some(output);
            completed_clone.store(true, Ordering::Release);
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task_to_ffrt(wrapper.clone());

        // 自旋等待完成
        loop {
            if completed.load(Ordering::Acquire) {
                break;
            }
            unsafe {
                ffrt_usleep(1000); // 休眠1ms
            }
        }

        let opt = { result.lock().unwrap().take() };
        opt.ok_or(RuntimeError::Other("Task result missing".to_string()))
    }

    /// 生成新任务
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(StdMutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().unwrap() = Some(output);
            completed_clone.store(true, Ordering::Release);
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task_to_ffrt(wrapper.clone());

        JoinHandle {
            result,
            completed,
            wrapper,
        }
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

/// 任务句柄
pub struct JoinHandle<T> {
    result: Arc<StdMutex<Option<T>>>,
    completed: Arc<AtomicBool>,
    wrapper: Arc<FfrtTaskWrapper>,
}

impl<T> JoinHandle<T> {
    /// 等待任务完成
    pub async fn join(self) -> Result<T> {
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or(RuntimeError::Other("Task result missing".to_string()));
            }

            yield_now().await;
        }
    }

    /// 检查任务是否完成
    pub fn is_finished(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    /// 取消任务
    pub fn cancel(&self) {
        self.wrapper.cancel();
    }

    /// 等待指定超时时间后返回
    pub async fn join_timeout(self, duration: Duration) -> Result<T> {
        let start = Instant::now();
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or(RuntimeError::Other("Task result missing".to_string()));
            }

            if start.elapsed() > duration {
                self.cancel();
                return Err(RuntimeError::Timeout);
            }

            yield_now().await;
        }
    }
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.completed.load(Ordering::Acquire) {
            Poll::Ready(
                self.result
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or(RuntimeError::Other("Task result missing".to_string())),
            )
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// 让出当前任务的执行权
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

/// 休眠指定时间
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
                // 使用FFRT的sleep
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

/// 等待所有任务完成
pub fn wait_all() {
    unsafe {
        ffrt_wait();
    }
}

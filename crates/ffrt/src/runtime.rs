use crate::error::RuntimeError;
use crate::lock::Mutex;
use crate::{Task, TaskAttr};
use ohos_ffrt_sys::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, RuntimeError>;

static TASK_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn next_task_id() -> usize {
    TASK_ID_COUNTER.fetch_add(1, Ordering::SeqCst)
}

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Pending,
    Running,
    Completed,
    Cancelled,
}

/// FFRT 任务包装器
struct FfrtTaskWrapper {
    #[allow(dead_code)]
    task_id: usize,
    future: Mutex<Option<BoxFuture>>,
    result: Mutex<Option<()>>,
    state: AtomicUsize,
    cancelled: AtomicBool,
    join_waker: Mutex<Option<Waker>>,
}

impl FfrtTaskWrapper {
    fn new(future: BoxFuture) -> Arc<Self> {
        Arc::new(Self {
            task_id: next_task_id(),
            future: Mutex::new(Some(future)),
            result: Mutex::new(None),
            state: AtomicUsize::new(TaskState::Pending as usize),
            cancelled: AtomicBool::new(false),
            join_waker: Mutex::new(None),
        })
    }

    fn get_state(&self) -> TaskState {
        match self.state.load(Ordering::Acquire) {
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

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn poll_with_arc(arc_self: &Arc<Self>) -> bool {
        if arc_self.is_cancelled() {
            arc_self.set_state(TaskState::Cancelled);
            return true;
        }

        arc_self.set_state(TaskState::Running);

        let waker = create_waker_for_task(Arc::clone(arc_self));
        let mut cx = Context::from_waker(&waker);

        let mut future_opt = arc_self.future.lock().expect("Failed to lock future");
        if let Some(mut future) = future_opt.take() {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(&mut cx)
            })) {
                Ok(Poll::Ready(_)) => {
                    *arc_self.result.lock().expect("Failed to lock result") = Some(());
                    arc_self.set_state(TaskState::Completed);
                    // 唤醒等待的 JoinHandle
                    if let Some(waker) = arc_self
                        .join_waker
                        .lock()
                        .expect("Failed to lock join_waker")
                        .take()
                    {
                        waker.wake();
                    }
                    true
                }
                Ok(Poll::Pending) => {
                    *future_opt = Some(future);
                    arc_self.set_state(TaskState::Pending);
                    false
                }
                Err(_) => {
                    arc_self.set_state(TaskState::Cancelled);
                    // 唤醒等待的 JoinHandle
                    if let Some(waker) = arc_self
                        .join_waker
                        .lock()
                        .expect("Failed to lock join_waker")
                        .take()
                    {
                        waker.wake();
                    }
                    true
                }
            }
        } else {
            arc_self.set_state(TaskState::Completed);
            // 唤醒等待的 JoinHandle
            if let Some(waker) = arc_self
                .join_waker
                .lock()
                .expect("Failed to lock join_waker")
                .take()
            {
                waker.wake();
            }
            true
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

/// 创建 Waker
fn create_waker_for_task(wrapper: Arc<FfrtTaskWrapper>) -> Waker {
    use std::task::{RawWaker, RawWakerVTable};

    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        if data.is_null() {
            panic!("Waker clone_raw received null data");
        }

        unsafe {
            let wrapper = data as *const FfrtTaskWrapper;
            Arc::increment_strong_count(wrapper);
            RawWaker::new(wrapper as *const (), &VTABLE)
        }
    }

    unsafe fn wake_raw(data: *const ()) {
        if data.is_null() {
            return;
        }

        unsafe {
            let wrapper = Arc::from_raw(data as *const FfrtTaskWrapper);
            submit_task(wrapper, None);
        }
    }

    unsafe fn wake_by_ref_raw(data: *const ()) {
        if data.is_null() {
            return;
        }

        unsafe {
            Arc::increment_strong_count(data as *const FfrtTaskWrapper);
            let wrapper = Arc::from_raw(data as *const FfrtTaskWrapper);
            submit_task(wrapper, None);
        }
    }

    unsafe fn drop_raw(data: *const ()) {
        if data.is_null() {
            return;
        }

        unsafe {
            drop(Arc::from_raw(data as *const FfrtTaskWrapper));
        }
    }

    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

    let wrapper_ptr = Arc::into_raw(wrapper);

    unsafe {
        let raw = RawWaker::new(wrapper_ptr as *const (), &VTABLE);
        Waker::from_raw(raw)
    }
}

/// 使用 Task 提交任务到 FFRT
fn submit_task(wrapper: Arc<FfrtTaskWrapper>, attr: Option<TaskAttr>) {
    let task = attr.map(Task::new).unwrap_or_else(Task::default);

    task.submit(move || {
        // 执行任务
        let finished = FfrtTaskWrapper::poll_with_arc(&wrapper);

        // 如果任务未完成，需要重新提交
        if !finished {
            submit_task(wrapper, None);
        }
    });
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

#[derive(Clone, Copy, Default)]
pub struct Runtime;

impl Runtime {
    pub fn new() -> Self {
        Self
    }

    pub fn global() -> Self {
        *RUNTIME.get_or_init(|| Self::new())
    }

    /// 阻塞当前线程，运行 future 直到完成
    pub fn block_on<F>(&self, future: F) -> Result<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(Mutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().expect("Failed to lock result") = Some(output);
            completed_clone.store(true, Ordering::Release);
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task(wrapper, None);

        // 等待完成
        while !completed.load(Ordering::Acquire) {
            unsafe {
                ffrt_usleep(100);
            }
        }

        let opt = { result.lock().expect("Failed to lock result").take() };
        opt.ok_or(RuntimeError::Other("Task result missing".to_string()))
    }

    /// 在 runtime 上生成一个新的异步任务
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(Mutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().expect("Failed to lock result") = Some(output);
            completed_clone.store(true, Ordering::Release);
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task(wrapper.clone(), None);

        JoinHandle {
            result,
            completed,
            wrapper,
        }
    }

    /// 使用指定的任务属性生成异步任务
    pub fn spawn_with_attr<F>(&self, attr: TaskAttr, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(Mutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().expect("Failed to lock result") = Some(output);
            completed_clone.store(true, Ordering::Release);
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task(wrapper.clone(), Some(attr));

        JoinHandle {
            result,
            completed,
            wrapper,
        }
    }
}

pub struct JoinHandle<T> {
    result: Arc<Mutex<Option<T>>>,
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
                    .expect("Failed to lock result")
                    .take()
                    .ok_or(RuntimeError::Other("Task result missing".to_string()));
            }

            yield_now().await;
        }
    }

    /// 检查任务是否已完成
    pub fn is_finished(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    /// 取消任务
    pub fn cancel(&self) {
        self.wrapper.cancel();
    }

    /// 带超时的等待
    pub async fn join_timeout(self, duration: Duration) -> Result<T> {
        let start = Instant::now();
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("Failed to lock result")
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
                    .expect("Failed to lock result")
                    .take()
                    .ok_or(RuntimeError::Other("Task result missing".to_string())),
            )
        } else {
            // 保存 waker，以便任务完成时能唤醒
            *self
                .wrapper
                .join_waker
                .lock()
                .expect("Failed to lock join_waker") = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

unsafe impl<T: Send> Send for JoinHandle<T> {}
unsafe impl<T: Send> Sync for JoinHandle<T> {}

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

/// 异步睡眠
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

/// 等待所有提交的任务完成
pub fn wait_all() {
    unsafe {
        ffrt_wait();
    }
}

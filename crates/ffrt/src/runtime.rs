use crate::error::RuntimeError;
use crate::mutex::Mutex;
use ohos_ffrt_sys::*;
use std::future::Future;
use std::pin::Pin;
use std::ptr;
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

/// FFRT任务包装器
struct FfrtTaskWrapper {
    #[allow(dead_code)]
    task_id: usize,
    future: Mutex<Option<BoxFuture>>,
    result: Mutex<Option<()>>,
    state: AtomicUsize,
    cancelled: AtomicBool,
}

impl FfrtTaskWrapper {
    fn new(future: BoxFuture) -> Arc<Self> {
        Arc::new(Self {
            task_id: next_task_id(),
            future: Mutex::new(Some(future)),
            result: Mutex::new(None),
            state: AtomicUsize::new(TaskState::Pending as usize),
            cancelled: AtomicBool::new(false),
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

    // 修复：将Arc作为参数传入，这样可以正确克隆
    fn poll_with_arc(arc_self: &Arc<Self>) -> bool {
        if arc_self.is_cancelled() {
            arc_self.set_state(TaskState::Cancelled);
            return true;
        }

        arc_self.set_state(TaskState::Running);

        // 创建waker时传入Arc的克隆
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
                    true
                }
                Ok(Poll::Pending) => {
                    *future_opt = Some(future);
                    arc_self.set_state(TaskState::Pending);
                    false
                }
                Err(_) => {
                    arc_self.set_state(TaskState::Cancelled);
                    true
                }
            }
        } else {
            arc_self.set_state(TaskState::Completed);
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

// 修复：独立的函数来创建Waker，接收Arc作为参数
fn create_waker_for_task(wrapper: Arc<FfrtTaskWrapper>) -> Waker {
    use std::task::{RawWaker, RawWakerVTable};

    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        if data.is_null() {
            panic!("Waker clone_raw received null data");
        }

        unsafe {
            let wrapper = data as *const FfrtTaskWrapper;
            // 增加引用计数
            Arc::increment_strong_count(wrapper);
            RawWaker::new(wrapper as *const (), &VTABLE)
        }
    }

    unsafe fn wake_raw(data: *const ()) {
        if data.is_null() {
            return;
        }

        unsafe {
            // 重建Arc并消费它
            let wrapper = Arc::from_raw(data as *const FfrtTaskWrapper);
            submit_task(wrapper);
        }
    }

    unsafe fn wake_by_ref_raw(data: *const ()) {
        if data.is_null() {
            return;
        }

        unsafe {
            // 增加引用计数以创建新的Arc
            Arc::increment_strong_count(data as *const FfrtTaskWrapper);
            let wrapper = Arc::from_raw(data as *const FfrtTaskWrapper);
            submit_task(wrapper);
        }
    }

    unsafe fn drop_raw(data: *const ()) {
        if data.is_null() {
            return;
        }

        unsafe {
            // 减少引用计数
            drop(Arc::from_raw(data as *const FfrtTaskWrapper));
        }
    }

    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

    // 将Arc转换为原始指针
    let wrapper_ptr = Arc::into_raw(wrapper);

    unsafe {
        let raw = RawWaker::new(wrapper_ptr as *const (), &VTABLE);
        Waker::from_raw(raw)
    }
}

/// 任务执行上下文
#[repr(C)]
struct TaskContext {
    header: ffrt_function_header_t,
    wrapper: *const FfrtTaskWrapper,
}

/// 提交任务到FFRT
fn submit_task(wrapper: Arc<FfrtTaskWrapper>) {
    unsafe extern "C" fn task_exec(arg: *mut std::ffi::c_void) {
        if arg.is_null() {
            eprintln!("FFRT task_exec received null argument!");
            return;
        }

        unsafe {
            let ctx = &*(arg as *const TaskContext);

            if ctx.wrapper.is_null() {
                eprintln!("TaskContext wrapper is null!");
                return;
            }

            // 修复：从原始指针重建Arc（不改变引用计数）
            // 因为这个指针是在submit_task中通过Arc::into_raw创建的
            let wrapper = Arc::from_raw(ctx.wrapper);

            // 执行任务，传入Arc引用
            let finished = FfrtTaskWrapper::poll_with_arc(&wrapper);

            // 如果任务未完成，需要保持引用
            if !finished {
                // 将Arc转回原始指针，保持引用计数不变
                let _ = Arc::into_raw(wrapper);
            } else {
                // 任务完成，将指针设为null，防止task_destroy重复释放
                // 注意：这里我们不能修改ctx，因为它是const引用
                // 所以我们依赖task_destroy的逻辑来处理
                // wrapper会在这里被drop，引用计数-1
            }
        }
    }

    unsafe extern "C" fn task_destroy(arg: *mut std::ffi::c_void) {
        if arg.is_null() {
            eprintln!("FFRT task_destroy received null argument!");
            return;
        }

        unsafe {
            // 重建Box以释放TaskContext
            let ctx = Box::from_raw(arg as *mut TaskContext);

            // 修复：检查wrapper指针是否仍然有效
            // 如果task_exec已经完成并drop了wrapper，这里不需要再处理
            // 但由于我们无法修改ctx.wrapper的值，我们需要小心处理

            // 使用Arc::strong_count来检查是否还有其他引用
            if !ctx.wrapper.is_null() {
                // 尝试重建Arc以检查引用计数
                Arc::increment_strong_count(ctx.wrapper);
                let test_arc = Arc::from_raw(ctx.wrapper);
                let strong_count = Arc::strong_count(&test_arc);

                if strong_count > 1 {
                    // 还有其他引用存在，正常drop
                    drop(test_arc);
                } else {
                    // 这是最后一个引用，drop它
                    drop(test_arc);
                }
            }
        }
    }

    unsafe {
        // 将Arc转换为原始指针
        let wrapper_ptr = Arc::into_raw(wrapper);

        let ctx = Box::new(TaskContext {
            header: ffrt_function_header_t {
                exec: Some(task_exec),
                destroy: Some(task_destroy),
                reserve: [0; 2],
            },
            wrapper: wrapper_ptr,
        });

        let ctx_ptr = Box::into_raw(ctx);

        // 提交任务
        ffrt_submit_base(
            &mut (*ctx_ptr).header as *mut ffrt_function_header_t,
            ptr::null(),
            ptr::null(),
            ptr::null(),
        );
    }
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

#[derive(Clone, Copy, Default)]
pub struct Runtime;

impl Runtime {
    pub fn new() -> Self {
        Self
    }

    #[deprecated(note = "使用 ffrt_submit_base 时不再区分串行/并发模式")]
    pub fn new_serial() -> Self {
        Self
    }

    pub fn global() -> Self {
        *RUNTIME.get_or_init(|| Self::new())
    }

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
        submit_task(wrapper);

        while !completed.load(Ordering::Acquire) {
            unsafe {
                ffrt_usleep(100);
            }
        }

        let opt = { result.lock().expect("Failed to lock result").take() };
        opt.ok_or(RuntimeError::Other("Task result missing".to_string()))
    }

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
        submit_task(wrapper.clone());

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

    pub fn is_finished(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.wrapper.cancel();
    }

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
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

unsafe impl<T: Send> Send for JoinHandle<T> {}
unsafe impl<T: Send> Sync for JoinHandle<T> {}

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

pub fn wait_all() {
    unsafe {
        ffrt_wait();
    }
}

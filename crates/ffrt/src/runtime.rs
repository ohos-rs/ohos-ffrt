//! 异步运行时

use ohos_ffrt_sys::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll, Waker};

/// 任务ID生成器
static TASK_ID_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn next_task_id() -> usize {
    TASK_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// 类型擦除的Future包装
type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// FFRT任务包装器
struct FfrtTaskWrapper {
    task_id: usize,
    future: StdMutex<Option<BoxFuture>>,
    completed: StdMutex<bool>,
}

impl FfrtTaskWrapper {
    fn new(future: BoxFuture) -> Arc<Self> {
        Arc::new(Self {
            task_id: next_task_id(),
            future: StdMutex::new(Some(future)),
            completed: StdMutex::new(false),
        })
    }

    fn poll(&self) -> bool {
        let waker = self.create_waker();
        let mut cx = Context::from_waker(&waker);

        let mut future_opt = self.future.lock().unwrap();
        if let Some(mut future) = future_opt.take() {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(_) => {
                    *self.completed.lock().unwrap() = true;
                    true
                }
                Poll::Pending => {
                    *future_opt = Some(future);
                    false
                }
            }
        } else {
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

    fn is_completed(&self) -> bool {
        *self.completed.lock().unwrap()
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

/// 异步运行时
pub struct Runtime {
    _private: (),
}

impl Runtime {
    /// 创建新的运行时
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// 在运行时上执行future直到完成
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(StdMutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(StdMutex::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().unwrap() = Some(output);
            *completed_clone.lock().unwrap() = true;
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task_to_ffrt(wrapper.clone());

        // 自旋等待完成
        loop {
            if *completed.lock().unwrap() {
                break;
            }
            unsafe {
                ffrt_usleep(1000); // 休眠1ms
            }
        }

        result.lock().unwrap().take().expect("Task result missing")
    }

    /// 生成新任务
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let result = Arc::new(StdMutex::new(None));
        let result_clone = result.clone();
        let completed = Arc::new(StdMutex::new(false));
        let completed_clone = completed.clone();

        let wrapper_future = async move {
            let output = future.await;
            *result_clone.lock().unwrap() = Some(output);
            *completed_clone.lock().unwrap() = true;
        };

        let wrapper = FfrtTaskWrapper::new(Box::pin(wrapper_future));
        submit_task_to_ffrt(wrapper);

        JoinHandle { result, completed }
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
    completed: Arc<StdMutex<bool>>,
}

impl<T> JoinHandle<T> {
    /// 等待任务完成
    pub async fn join(self) -> T {
        loop {
            if *self.completed.lock().unwrap() {
                return self
                    .result
                    .lock()
                    .unwrap()
                    .take()
                    .expect("Task result missing");
            }

            yield_now().await;
        }
    }

    /// 检查任务是否完成
    pub fn is_finished(&self) -> bool {
        *self.completed.lock().unwrap()
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
pub async fn sleep(duration: std::time::Duration) {
    struct Sleep {
        deadline: std::time::Instant,
    }

    impl Future for Sleep {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if std::time::Instant::now() >= self.deadline {
                Poll::Ready(())
            } else {
                // 使用FFRT的sleep
                let remaining = self.deadline - std::time::Instant::now();
                unsafe {
                    ffrt_usleep(remaining.as_micros() as u64);
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    Sleep {
        deadline: std::time::Instant::now() + duration,
    }
    .await
}

/// 等待所有任务完成
pub fn wait_all() {
    unsafe {
        ffrt_wait();
    }
}

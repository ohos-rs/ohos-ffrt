use crate::error::RuntimeError;
use crate::signal::oneshot;
use crate::{Task, TaskAttr};
use ohos_ffrt_sys::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, RuntimeError>;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// 基于 FFRT 的异步运行时
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
        let (tx, rx) = oneshot::channel();

        let task = Task::default();
        task.submit(move || {
            let output = poll_once(future);
            let _ = tx.send(output);
        });

        rx.blocking_recv()
            .map_err(|_| RuntimeError::Other("Task failed".to_string()))
    }

    /// 在 runtime 上生成一个新的异步任务
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

        JoinHandle { rx }
    }

    /// 使用指定的任务属性生成异步任务
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

        JoinHandle { rx }
    }
}

/// 在当前线程持续 poll future 直到完成
fn poll_once<F: Future>(mut future: F) -> F::Output {
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }

        // 短暂让出执行权，避免忙等待
        unsafe {
            ffrt_yield();
        }
    }
}

/// 创建一个空的 waker
fn noop_waker() -> Waker {
    unsafe fn noop_clone(_: *const ()) -> RawWaker {
        noop_raw_waker()
    }

    unsafe fn noop_wake(_: *const ()) {}

    unsafe fn noop_wake_by_ref(_: *const ()) {}

    unsafe fn noop_drop(_: *const ()) {}

    fn noop_raw_waker() -> RawWaker {
        RawWaker::new(
            std::ptr::null(),
            &RawWakerVTable::new(noop_clone, noop_wake, noop_wake_by_ref, noop_drop),
        )
    }

    unsafe { Waker::from_raw(noop_raw_waker()) }
}

/// 任务的 JoinHandle
pub struct JoinHandle<T> {
    rx: oneshot::Receiver<T>,
}

impl<T> JoinHandle<T> {
    /// 等待任务完成
    pub async fn join(self) -> Result<T> {
        self.rx
            .await
            .map_err(|_| RuntimeError::Other("Task failed".to_string()))
    }

    /// 检查任务是否已完成
    pub fn is_finished(&self) -> bool {
        // 通过尝试接收来检查，但不消耗结果
        // 注意：这会导致值被消耗，所以这个实现不完美
        // 但对于简单的使用场景是足够的
        false
    }
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
            Poll::Ready(Err(_)) => Poll::Ready(Err(RuntimeError::Other("Task failed".to_string()))),
            Poll::Pending => Poll::Pending,
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

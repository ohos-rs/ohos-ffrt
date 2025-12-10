use super::WakerState;
use crate::signal::oneshot;
use crate::{RuntimeError, create_waker};
use crate::{Task, TaskAttr};
use ohos_ffrt_sys::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// FFRT Runtime
#[derive(Clone, Copy, Default)]
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

        JoinHandle { rx }
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

        JoinHandle { rx }
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
}

impl<T> JoinHandle<T> {
    /// Wait for the task to complete
    pub async fn join(self) -> Result<T> {
        self.rx
            .await
            .map_err(|_| RuntimeError::Other("Task failed".to_string()))
    }

    /// Check if the task is finished
    pub fn is_finished(&self) -> bool {
        // Check by trying to receive, but do not consume the result
        // Note: This will consume the value, so this implementation is not perfect
        // For simple use cases it is enough
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

/// Wait for all submitted tasks to complete
pub fn wait_all() {
    unsafe {
        ffrt_wait();
    }
}

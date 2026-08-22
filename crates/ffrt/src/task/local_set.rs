use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use crate::lock::Mutex;

struct LocalState {
    run_waker: Mutex<Option<Waker>>,
}

struct TaskWaker {
    state: Arc<LocalState>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        let mut guard = self.state.run_waker.lock().unwrap();
        if let Some(waker) = guard.take() {
            waker.wake();
        }
    }
}

struct LocalTask {
    future: Pin<Box<dyn Future<Output = ()> + 'static>>,
}

/// A single-threaded, tokio-style local task set.
///
/// Futures scheduled on a `LocalSet` are polled on the current FFRT worker
/// thread and are not required to be `Send`.
pub struct LocalSet {
    tasks: VecDeque<LocalTask>,
    state: Arc<LocalState>,
}

impl LocalSet {
    /// Creates an empty local task set.
    pub fn new() -> Self {
        Self {
            tasks: VecDeque::new(),
            state: Arc::new(LocalState {
                run_waker: Mutex::new(None),
            }),
        }
    }

    /// Spawns a `!Send` future on this task set.
    pub fn spawn_local<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push_back(LocalTask {
            future: Box::pin(future),
        });
    }

    /// Runs `future` while polling local tasks until it completes.
    pub fn run_until<F>(&mut self, future: F) -> RunUntil<'_, F>
    where
        F: Future,
    {
        RunUntil {
            local: self,
            future,
        }
    }

    /// Returns the number of pending local tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Returns `true` when no local tasks are pending.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

impl Default for LocalSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Future returned by [`LocalSet::run_until`].
#[must_use = "futures do nothing unless polled"]
pub struct RunUntil<'a, F> {
    local: &'a mut LocalSet,
    future: F,
}

impl<F> Future for RunUntil<'_, F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `future` is pinned through `self` and never moved.
        let this = unsafe { self.get_unchecked_mut() };

        loop {
            let future = unsafe { Pin::new_unchecked(&mut this.future) };
            if let Poll::Ready(output) = future.poll(cx) {
                return Poll::Ready(output);
            }

            let mut tasks = std::mem::take(&mut this.local.tasks);
            let mut made_progress = false;

            while let Some(mut task) = tasks.pop_front() {
                let task_waker = Waker::from(Arc::new(TaskWaker {
                    state: this.local.state.clone(),
                }));
                let mut task_cx = Context::from_waker(&task_waker);

                if task.future.as_mut().poll(&mut task_cx).is_ready() {
                    made_progress = true;
                } else {
                    tasks.push_back(task);
                }
            }

            this.local.tasks = tasks;

            if !made_progress {
                let mut guard = this.local.state.run_waker.lock().unwrap();
                *guard = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_set_accepts_non_send_future() {
        use std::rc::Rc;

        let mut local = LocalSet::new();
        let value = Rc::new(7);
        local.spawn_local(async move {
            let _ = value;
        });

        assert_eq!(local.len(), 1);
        assert!(!local.is_empty());
    }
}

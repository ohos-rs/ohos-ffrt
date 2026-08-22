use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use crate::lock::Mutex;
use crate::runtime::{CancellationState, JoinHandle, Result as RuntimeResult};

thread_local! {
    static CURRENT_LOCAL: Cell<*const LocalSet> = const { Cell::new(std::ptr::null()) };
}

struct LocalState {
    run_waker: Mutex<Option<Waker>>,
    notified: AtomicBool,
}

struct TaskWaker {
    state: Arc<LocalState>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.state.notified.store(true, Ordering::Release);
        let mut guard = self.state.run_waker.lock().unwrap();
        if let Some(waker) = guard.take() {
            waker.wake();
        }
    }
}

struct LocalTask {
    future: Pin<Box<dyn Future<Output = ()> + 'static>>,
}

struct LocalJoin<F: Future> {
    future: Pin<Box<F>>,
    sender: Option<crate::signal::oneshot::Sender<RuntimeResult<F::Output>>>,
    state: Arc<CancellationState>,
}

impl<F: Future> Future for LocalJoin<F> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        this.state.register(cx.waker());
        if this.state.is_cancelled() {
            this.state.finish();
            if let Some(sender) = this.sender.take() {
                let _ = sender.send(Err(crate::JoinError::cancelled(this.state.id)));
            }
            return Poll::Ready(());
        }

        match catch_unwind(AssertUnwindSafe(|| this.future.as_mut().poll(cx))) {
            Ok(Poll::Ready(output)) => {
                this.state.finish();
                if let Some(sender) = this.sender.take() {
                    let _ = sender.send(Ok(output));
                }
                Poll::Ready(())
            }
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => {
                this.state.finish();
                if let Some(sender) = this.sender.take() {
                    let _ = sender.send(Err(crate::runtime::panic_error(this.state.id, payload)));
                }
                Poll::Ready(())
            }
        }
    }
}

/// A single-threaded set for futures that do not implement `Send`.
pub struct LocalSet {
    tasks: RefCell<VecDeque<LocalTask>>,
    state: Arc<LocalState>,
}

impl LocalSet {
    pub fn new() -> Self {
        Self {
            tasks: RefCell::new(VecDeque::new()),
            state: Arc::new(LocalState {
                run_waker: Mutex::new(None),
                notified: AtomicBool::new(false),
            }),
        }
    }

    /// Spawns a non-`Send` future on this local set.
    pub fn spawn_local<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let (sender, handle) = crate::runtime::local_join_channel();
        let state = handle.abort_handle().state;
        self.tasks.borrow_mut().push_back(LocalTask {
            future: Box::pin(LocalJoin {
                future: Box::pin(future),
                sender: Some(sender),
                state,
            }),
        });
        self.state.notified.store(true, Ordering::Release);
        if let Some(waker) = self.state.run_waker.lock().unwrap().take() {
            waker.wake();
        }
        handle
    }

    /// Enters this local set so [`spawn_local`] can be called synchronously.
    pub fn enter(&self) -> LocalEnterGuard<'_> {
        LocalEnterGuard::enter(self)
    }

    pub fn run_until<F>(&self, future: F) -> RunUntil<'_, F>
    where
        F: Future,
    {
        RunUntil {
            local: self,
            future,
        }
    }

    pub fn block_on<F>(&self, runtime: &crate::Runtime, future: F) -> F::Output
    where
        F: Future,
    {
        runtime.block_on(self.run_until(future))
    }

    pub fn len(&self) -> usize {
        self.tasks.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.borrow().is_empty()
    }

    fn poll_tasks(&self, cx: &mut Context<'_>) -> bool {
        let mut tasks = self.tasks.take();
        let initial = tasks.len();
        for _ in 0..initial {
            let Some(mut task) = tasks.pop_front() else {
                break;
            };
            let task_waker = Waker::from(Arc::new(TaskWaker {
                state: self.state.clone(),
            }));
            let mut task_cx = Context::from_waker(&task_waker);
            if task.future.as_mut().poll(&mut task_cx).is_pending() {
                tasks.push_back(task);
            }
        }

        // Tasks spawned while polling are kept ahead of the pending tasks.
        self.tasks.borrow_mut().append(&mut tasks);
        if self.state.notified.swap(false, Ordering::AcqRel) {
            cx.waker().wake_by_ref();
        }
        self.tasks.borrow().is_empty()
    }
}

impl Default for LocalSet {
    fn default() -> Self {
        Self::new()
    }
}

impl Future for LocalSet {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let _context = this.enter();
        if this.poll_tasks(cx) {
            Poll::Ready(())
        } else {
            *this.state.run_waker.lock().unwrap() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct RunUntil<'a, F> {
    local: &'a LocalSet,
    future: F,
}

impl<F: Future> Future for RunUntil<'_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let _context = this.local.enter();
        let future = unsafe { Pin::new_unchecked(&mut this.future) };
        if let Poll::Ready(output) = future.poll(cx) {
            return Poll::Ready(output);
        }
        this.local.poll_tasks(cx);
        *this.local.state.run_waker.lock().unwrap() = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Guard returned by [`LocalSet::enter`].
#[must_use]
pub struct LocalEnterGuard<'a> {
    previous: *const LocalSet,
    _local: std::marker::PhantomData<&'a LocalSet>,
}

impl<'a> LocalEnterGuard<'a> {
    fn enter(local: &'a LocalSet) -> Self {
        let previous = CURRENT_LOCAL.with(|current| current.replace(local));
        Self {
            previous,
            _local: std::marker::PhantomData,
        }
    }
}

impl Drop for LocalEnterGuard<'_> {
    fn drop(&mut self) {
        CURRENT_LOCAL.with(|current| current.set(self.previous));
    }
}

/// Spawns a non-`Send` future on the currently running [`LocalSet`].
pub fn spawn_local<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    CURRENT_LOCAL.with(|current| {
        let local = current.get();
        assert!(!local.is_null(), "spawn_local called outside a LocalSet");
        unsafe { (&*local).spawn_local(future) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_set_accepts_non_send_future() {
        use std::rc::Rc;

        let local = LocalSet::new();
        let value = Rc::new(7);
        let handle = local.spawn_local(async move { *value });
        let result = crate::Runtime::new()
            .unwrap()
            .block_on(local.run_until(handle));
        assert_eq!(result.unwrap(), 7);
    }
}

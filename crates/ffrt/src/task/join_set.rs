use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use crate::RuntimeError;
use crate::runtime::{AbortHandle, Result as RuntimeResult};
use crate::signal::mpsc;

struct Abortable<F> {
    inner: F,
    cancelled: Arc<AtomicBool>,
}

impl<F: Future> Future for Abortable<F> {
    type Output = RuntimeResult<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(Err(RuntimeError::Cancelled));
        }

        // SAFETY: the inner future is pinned through `self` and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        let inner = unsafe { Pin::new_unchecked(&mut this.inner) };
        match inner.poll(cx) {
            Poll::Ready(output) => Poll::Ready(Ok(output)),
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn run_task<F, V>(
    id: u64,
    future: F,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<(u64, RuntimeResult<V>)>,
) where
    F: Future<Output = V>,
{
    let output = Abortable {
        inner: future,
        cancelled,
    }
    .await;
    let _ = sender.send((id, output));
}

/// A collection of tasks with tokio-like `join_next` semantics.
pub struct JoinSet<V> {
    sender: mpsc::UnboundedSender<(u64, RuntimeResult<V>)>,
    receiver: mpsc::UnboundedReceiver<(u64, RuntimeResult<V>)>,
    next_id: u64,
    pending: usize,
    abort_handles: Vec<AbortHandle>,
}

impl<V> JoinSet<V> {
    /// Creates an empty join set.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self {
            sender,
            receiver,
            next_id: 0,
            pending: 0,
            abort_handles: Vec::new(),
        }
    }

    /// Spawns a future and returns an abort handle.
    pub fn spawn<F>(&mut self, future: F) -> AbortHandle
    where
        F: Future<Output = V> + Send + 'static,
        V: Send + 'static,
    {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let cancelled = Arc::new(AtomicBool::new(false));
        let abort_handle = AbortHandle::new(cancelled.clone());
        self.abort_handles.push(abort_handle.clone());
        self.pending += 1;

        let sender = self.sender.clone();
        crate::spawn(run_task(id, future, cancelled, sender));

        abort_handle
    }

    /// Spawns a blocking closure and returns an abort handle.
    pub fn spawn_blocking<F>(&mut self, func: F) -> AbortHandle
    where
        F: FnOnce() -> V + Send + 'static,
        V: Send + 'static,
    {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let cancelled = Arc::new(AtomicBool::new(false));
        let abort_handle = AbortHandle::new(cancelled.clone());
        self.abort_handles.push(abort_handle.clone());
        self.pending += 1;

        let sender = self.sender.clone();
        crate::spawn_blocking(move || {
            if cancelled.load(Ordering::Acquire) {
                let _ = sender.send((id, Err(RuntimeError::Cancelled)));
            } else {
                let _ = sender.send((id, Ok(func())));
            }
        });

        abort_handle
    }

    /// Waits for the next task to complete.
    pub fn join_next(&mut self) -> JoinNext<'_, V> {
        JoinNext { set: self }
    }

    /// Returns the number of pending tasks.
    pub fn len(&self) -> usize {
        self.pending
    }

    /// Returns `true` when no tasks are pending.
    pub fn is_empty(&self) -> bool {
        self.pending == 0
    }

    /// Aborts all tasks in the set.
    pub fn abort_all(&mut self) {
        for handle in &self.abort_handles {
            handle.abort();
        }
    }

    /// Shuts down the set by aborting all tasks.
    pub fn shutdown(&mut self) {
        self.abort_all();
    }

    /// Detaches all tasks without aborting them.
    pub fn detach_all(&mut self) {
        self.abort_handles.clear();
    }
}

impl<V> Default for JoinSet<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Future returned by [`JoinSet::join_next`].
#[must_use = "futures do nothing unless polled"]
pub struct JoinNext<'a, V> {
    set: &'a mut JoinSet<V>,
}

impl<V> Future for JoinNext<'_, V> {
    type Output = Option<RuntimeResult<V>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if this.set.pending == 0 {
            return Poll::Ready(None);
        }

        let mut recv = std::pin::pin!(this.set.receiver.recv());
        match recv.as_mut().poll(cx) {
            Poll::Ready(Some((_id, output))) => {
                this.set.pending -= 1;
                Poll::Ready(Some(output))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

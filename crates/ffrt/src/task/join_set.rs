use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::runtime::{AbortHandle, Handle, Id, JoinHandle, Result as RuntimeResult};
use crate::signal::mpsc;

async fn track_task<V: Send + 'static>(
    id: Id,
    handle: JoinHandle<V>,
    sender: mpsc::UnboundedSender<(Id, RuntimeResult<V>)>,
) {
    let output = handle.await;
    let _ = sender.send((id, output));
}

/// A collection of tasks with tokio-like `join_next` semantics.
pub struct JoinSet<V> {
    sender: mpsc::UnboundedSender<(Id, RuntimeResult<V>)>,
    receiver: mpsc::UnboundedReceiver<(Id, RuntimeResult<V>)>,
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
        let handle = crate::spawn(future);
        self.insert(handle)
    }

    /// Spawns a future on a specific runtime handle.
    pub fn spawn_on<F>(&mut self, future: F, runtime: &Handle) -> AbortHandle
    where
        F: Future<Output = V> + Send + 'static,
        V: Send + 'static,
    {
        let handle = runtime.spawn(future);
        self.insert(handle)
    }

    /// Spawns a non-`Send` future on the currently entered local set.
    pub fn spawn_local<F>(&mut self, future: F) -> AbortHandle
    where
        F: Future<Output = V> + 'static,
        V: 'static,
    {
        let handle = crate::task::spawn_local(future);
        self.insert_local(handle, None)
    }

    /// Spawns a non-`Send` future on a specific local set.
    pub fn spawn_local_on<F>(&mut self, future: F, local_set: &crate::task::LocalSet) -> AbortHandle
    where
        F: Future<Output = V> + 'static,
        V: 'static,
    {
        let handle = local_set.spawn_local(future);
        self.insert_local(handle, Some(local_set))
    }

    fn insert(&mut self, handle: JoinHandle<V>) -> AbortHandle
    where
        V: Send + 'static,
    {
        let id = handle.id();
        let abort_handle = handle.abort_handle();
        self.abort_handles.push(abort_handle.clone());
        self.pending += 1;

        let sender = self.sender.clone();
        crate::spawn(track_task(id, handle, sender));

        abort_handle
    }

    fn insert_local(
        &mut self,
        handle: JoinHandle<V>,
        local_set: Option<&crate::task::LocalSet>,
    ) -> AbortHandle
    where
        V: 'static,
    {
        let id = handle.id();
        let abort_handle = handle.abort_handle();
        self.abort_handles.push(abort_handle.clone());
        self.pending += 1;

        let sender = self.sender.clone();
        let tracker = async move {
            let output = handle.await;
            let _ = sender.send((id, output));
        };
        if let Some(local_set) = local_set {
            local_set.spawn_local(tracker);
        } else {
            crate::task::spawn_local(tracker);
        }

        abort_handle
    }

    /// Spawns a blocking closure and returns an abort handle.
    pub fn spawn_blocking<F>(&mut self, func: F) -> AbortHandle
    where
        F: FnOnce() -> V + Send + 'static,
        V: Send + 'static,
    {
        let handle = crate::spawn_blocking(func);
        self.insert(handle)
    }

    /// Spawns blocking work on a specific runtime handle.
    pub fn spawn_blocking_on<F>(&mut self, func: F, runtime: &Handle) -> AbortHandle
    where
        F: FnOnce() -> V + Send + 'static,
        V: Send + 'static,
    {
        self.insert(runtime.spawn_blocking(func))
    }

    /// Waits for the next task to complete.
    pub fn join_next(&mut self) -> JoinNext<'_, V> {
        JoinNext { set: self }
    }

    /// Waits for the next task and includes its task ID on success.
    pub fn join_next_with_id(&mut self) -> JoinNextWithId<'_, V> {
        JoinNextWithId { set: self }
    }

    /// Returns a completed task immediately, if one is ready.
    pub fn try_join_next(&mut self) -> Option<RuntimeResult<V>> {
        self.try_join_next_with_id()
            .map(|output| output.map(|(_, value)| value))
    }

    /// Returns a completed task and its ID immediately, if one is ready.
    pub fn try_join_next_with_id(&mut self) -> Option<RuntimeResult<(Id, V)>> {
        if self.pending == 0 {
            return None;
        }
        match self.receiver.try_recv() {
            Ok((id, Ok(value))) => {
                self.pending -= 1;
                self.remove_abort_handle(id);
                Some(Ok((id, value)))
            }
            Ok((id, Err(error))) => {
                self.pending -= 1;
                self.remove_abort_handle(id);
                Some(Err(error))
            }
            Err(_) => None,
        }
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
    pub async fn shutdown(&mut self) {
        self.abort_all();
        while self.join_next().await.is_some() {}
    }

    /// Detaches all tasks without aborting them.
    pub fn detach_all(&mut self) {
        self.abort_handles.clear();
    }

    /// Waits for all tasks, panicking if any task failed to join.
    pub async fn join_all(mut self) -> Vec<V> {
        let mut values = Vec::with_capacity(self.pending);
        while let Some(result) = self.join_next().await {
            match result {
                Ok(value) => values.push(value),
                Err(error) => panic!("task failed to join: {error}"),
            }
        }
        values
    }

    /// Polls for the next completed task.
    pub fn poll_join_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<RuntimeResult<V>>> {
        match self.poll_join_next_with_id(cx) {
            Poll::Ready(Some(Ok((_id, value)))) => Poll::Ready(Some(Ok(value))),
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    /// Polls for the next completed task and includes its ID on success.
    pub fn poll_join_next_with_id(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<RuntimeResult<(Id, V)>>> {
        if self.pending == 0 {
            return Poll::Ready(None);
        }

        let received = {
            let mut recv = std::pin::pin!(self.receiver.recv());
            recv.as_mut().poll(cx)
        };
        match received {
            Poll::Ready(Some((id, Ok(value)))) => {
                self.pending -= 1;
                self.remove_abort_handle(id);
                Poll::Ready(Some(Ok((id, value))))
            }
            Poll::Ready(Some((id, Err(error)))) => {
                self.pending -= 1;
                self.remove_abort_handle(id);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn remove_abort_handle(&mut self, id: Id) {
        if let Some(position) = self
            .abort_handles
            .iter()
            .position(|handle| handle.id() == id)
        {
            self.abort_handles.swap_remove(position);
        }
    }
}

impl<V> Drop for JoinSet<V> {
    fn drop(&mut self) {
        self.abort_all();
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
        self.get_mut().set.poll_join_next(cx)
    }
}

/// Future returned by [`JoinSet::join_next_with_id`].
#[must_use = "futures do nothing unless polled"]
pub struct JoinNextWithId<'a, V> {
    set: &'a mut JoinSet<V>,
}

impl<V> Future for JoinNextWithId<'_, V> {
    type Output = Option<RuntimeResult<(Id, V)>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().set.poll_join_next_with_id(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_set_receives_results() {
        let result = crate::Runtime::new().unwrap().block_on(async move {
            let mut set = JoinSet::new();
            set.spawn(async { 1 });
            set.spawn(async { 2 });

            let mut values = Vec::new();
            while let Some(item) = set.join_next().await {
                values.push(item?);
            }

            values.sort();
            assert_eq!(values, vec![1, 2]);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn abort_handle_marks_cancelled() {
        let mut set = JoinSet::new();
        let handle = set.spawn(async { 1 });
        handle.abort();
        assert_eq!(set.len(), 1);
        crate::Runtime::new().unwrap().block_on(set.shutdown());
    }
}

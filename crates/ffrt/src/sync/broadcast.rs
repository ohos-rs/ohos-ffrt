use crate::lock::Mutex;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

struct BroadcastState<T> {
    queue: VecDeque<(u64, T)>,
    capacity: usize,
    next_seq: u64,
    senders: usize,
    receivers: usize,
    waiters: VecDeque<Waker>,
}

/// Creates a tokio-style multi-producer, multi-consumer broadcast channel.
///
/// `capacity` is the number of retained messages; it is clamped to at least 1.
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>)
where
    T: Clone,
{
    let capacity = capacity.max(1);
    let shared = Arc::new(Mutex::new(BroadcastState {
        queue: VecDeque::new(),
        capacity,
        next_seq: 0,
        senders: 1,
        receivers: 1,
        waiters: VecDeque::new(),
    }));

    (
        Sender {
            shared: shared.clone(),
        },
        Receiver {
            shared,
            next_seq: 0,
        },
    )
}

/// Error returned by broadcast channel send operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError<T>(pub T);

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "broadcast channel has no receivers")
    }
}

impl<T: fmt::Debug> std::error::Error for SendError<T> {}

/// Error returned by broadcast channel receive operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    /// All senders have been dropped.
    Closed,
    /// A receiver skipped `n` messages because it fell behind.
    Lagged(u64),
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecvError::Closed => write!(f, "broadcast channel closed"),
            RecvError::Lagged(n) => write!(f, "broadcast receiver lagged by {} message(s)", n),
        }
    }
}

impl std::error::Error for RecvError {}

/// Error returned by the non-blocking broadcast receive method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    /// The channel is empty but senders are still alive.
    Empty,
    /// All senders have been dropped.
    Closed,
    /// A receiver skipped `n` messages because it fell behind.
    Lagged(u64),
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryRecvError::Empty => write!(f, "broadcast channel empty"),
            TryRecvError::Closed => write!(f, "broadcast channel closed"),
            TryRecvError::Lagged(n) => {
                write!(f, "broadcast receiver lagged by {} message(s)", n)
            }
        }
    }
}

impl std::error::Error for TryRecvError {}

/// The producing end of a broadcast channel.
pub struct Sender<T> {
    shared: Arc<Mutex<BroadcastState<T>>>,
}

impl<T: Clone> Sender<T> {
    /// Sends a value to all receivers.
    pub fn send(&self, value: T) -> Result<usize, SendError<T>> {
        let mut state = self.shared.lock().unwrap();
        if state.receivers == 0 {
            return Err(SendError(value));
        }

        Self::push_locked(&mut state, value);
        Ok(state.receivers)
    }

    /// Creates a receiver that only observes values sent after this call.
    pub fn subscribe(&self) -> Receiver<T> {
        let mut state = self.shared.lock().unwrap();
        state.receivers += 1;
        Receiver {
            shared: self.shared.clone(),
            next_seq: state.next_seq,
        }
    }

    /// Returns the number of connected receivers.
    pub fn receiver_count(&self) -> usize {
        let state = self.shared.lock().unwrap();
        state.receivers
    }

    /// Returns the number of active senders.
    pub fn sender_count(&self) -> usize {
        let state = self.shared.lock().unwrap();
        state.senders
    }

    /// Returns the number of retained messages.
    pub fn len(&self) -> usize {
        let state = self.shared.lock().unwrap();
        state.queue.len()
    }

    /// Returns `true` if no messages are currently retained.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn push_locked(state: &mut BroadcastState<T>, value: T) {
        let seq = state.next_seq;
        state.next_seq = state.next_seq.wrapping_add(1);
        state.queue.push_back((seq, value));
        while state.queue.len() > state.capacity {
            state.queue.pop_front();
        }

        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.lock().unwrap().senders += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap();
        state.senders -= 1;
        if state.senders == 0 {
            while let Some(waker) = state.waiters.pop_front() {
                waker.wake();
            }
        }
    }
}

/// The receiving end of a broadcast channel.
pub struct Receiver<T> {
    shared: Arc<Mutex<BroadcastState<T>>>,
    next_seq: u64,
}

impl<T: Clone> Receiver<T> {
    /// Receives the next available value.
    pub fn recv(&mut self) -> RecvFuture<'_, T> {
        RecvFuture { receiver: self }
    }

    /// Attempts to receive a value without waiting.
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let shared = self.shared.clone();
        let mut state = shared.lock().unwrap();
        Self::try_recv_locked(&mut state, &mut self.next_seq)
    }

    /// Returns the number of retained messages.
    pub fn len(&self) -> usize {
        let state = self.shared.lock().unwrap();
        state.queue.len()
    }

    /// Returns `true` if no messages are currently retained.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn try_recv_locked(
        state: &mut BroadcastState<T>,
        next_seq: &mut u64,
    ) -> Result<T, TryRecvError> {
        match state.queue.iter().position(|(seq, _)| *seq >= *next_seq) {
            Some(index) => {
                let (seq, _) = state.queue[index];
                if seq > *next_seq {
                    let skipped = seq - *next_seq;
                    *next_seq = seq;
                    return Err(TryRecvError::Lagged(skipped));
                }

                let (_, value) = state.queue.remove(index).expect("position checked above");
                *next_seq = next_seq.wrapping_add(1);
                Ok(value)
            }
            None => {
                if state.senders == 0 {
                    Err(TryRecvError::Closed)
                } else {
                    Err(TryRecvError::Empty)
                }
            }
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let mut state = self.shared.lock().unwrap();
        state.receivers += 1;
        Receiver {
            shared: self.shared.clone(),
            next_seq: state.next_seq,
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap();
        state.receivers -= 1;
    }
}

/// Future returned by [`Receiver::recv`].
#[must_use = "futures do nothing unless polled"]
pub struct RecvFuture<'a, T> {
    receiver: &'a mut Receiver<T>,
}

impl<T: Clone> Future for RecvFuture<'_, T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.receiver.shared.lock().unwrap();

        match state
            .queue
            .iter()
            .position(|(seq, _)| *seq >= this.receiver.next_seq)
        {
            Some(index) => {
                let (seq, _) = state.queue[index];
                if seq > this.receiver.next_seq {
                    let skipped = seq - this.receiver.next_seq;
                    this.receiver.next_seq = seq;
                    return Poll::Ready(Err(RecvError::Lagged(skipped)));
                }

                let (_, value) = state.queue.remove(index).expect("position checked above");
                this.receiver.next_seq = this.receiver.next_seq.wrapping_add(1);
                Poll::Ready(Ok(value))
            }
            None => {
                if state.senders == 0 {
                    Poll::Ready(Err(RecvError::Closed))
                } else {
                    state.waiters.push_back(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

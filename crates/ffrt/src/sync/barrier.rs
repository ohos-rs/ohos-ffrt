use crate::lock::Mutex;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

struct BarrierState {
    generation: u64,
    arrived: usize,
    waiters: VecDeque<Waker>,
}

/// A tokio-style reusable barrier.
pub struct Barrier {
    state: Mutex<BarrierState>,
    threshold: usize,
}

impl Barrier {
    /// Creates a barrier that waits for `n` tasks.
    pub fn new(n: usize) -> Self {
        Self {
            state: Mutex::new(BarrierState {
                generation: 0,
                arrived: 0,
                waiters: VecDeque::new(),
            }),
            threshold: if n == 0 { 1 } else { n },
        }
    }

    /// Waits until all tasks have rendezvoused at the barrier.
    pub fn wait(&self) -> BarrierFuture<'_> {
        BarrierFuture {
            barrier: self,
            generation: u64::MAX,
            registered: false,
            leader: false,
        }
    }
}

/// Future returned by [`Barrier::wait`].
#[must_use = "futures do nothing unless polled"]
pub struct BarrierFuture<'a> {
    barrier: &'a Barrier,
    generation: u64,
    registered: bool,
    leader: bool,
}

impl Future for BarrierFuture<'_> {
    type Output = BarrierWaitResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.barrier.state.lock().unwrap();

        if !this.registered {
            this.registered = true;
            this.generation = state.generation;
            state.arrived += 1;

            if state.arrived == this.barrier.threshold {
                state.arrived = 0;
                state.generation = state.generation.wrapping_add(1);
                this.leader = true;

                while let Some(waker) = state.waiters.pop_front() {
                    waker.wake();
                }

                return Poll::Ready(BarrierWaitResult { leader: true });
            }

            state.waiters.push_back(cx.waker().clone());
            return Poll::Pending;
        }

        if state.generation != this.generation {
            return Poll::Ready(BarrierWaitResult {
                leader: this.leader,
            });
        }

        state.waiters.push_back(cx.waker().clone());
        Poll::Pending
    }
}

/// Result returned by [`Barrier::wait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarrierWaitResult {
    leader: bool,
}

impl BarrierWaitResult {
    /// Returns `true` for the arbitrary task chosen as the barrier leader.
    pub fn is_leader(&self) -> bool {
        self.leader
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn barrier_releases_all_tasks() {
        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        let result = crate::Runtime::new().block_on(async move {
            let handle = crate::spawn(async move { b.wait().await });

            let mine = barrier.wait().await;
            let theirs = handle.await?;
            assert!(mine.is_leader() || theirs.is_leader());
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

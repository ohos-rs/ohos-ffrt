//! Cooperative scheduling compatibility helpers.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Yields once so other ready FFRT tasks can make progress.
pub async fn consume_budget() {
    crate::task::yield_now().await;
}

/// FFRT does not impose Tokio's per-poll operation budget.
pub fn has_budget_remaining() -> bool {
    true
}

/// Marks a future as exempt from cooperative budgeting.
pub fn unconstrained<F>(inner: F) -> Unconstrained<F> {
    Unconstrained { inner }
}

/// Future returned by [`unconstrained`].
#[derive(Clone, Copy, Debug)]
pub struct Unconstrained<F> {
    inner: F,
}

impl<F: Future> Future for Unconstrained<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { self.map_unchecked_mut(|this| &mut this.inner) }.poll(cx)
    }
}

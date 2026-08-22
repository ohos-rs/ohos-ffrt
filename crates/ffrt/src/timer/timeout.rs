use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use super::{Sleep, sleep_until};

/// Error returned when a future exceeds the supplied duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed(pub(crate) ());

impl fmt::Display for Elapsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}

/// Creates a future that completes when `future` completes or `deadline` is
/// reached.
pub fn timeout_at<F>(deadline: Instant, future: F) -> Timeout<F>
where
    F: Future,
{
    Timeout {
        value: future,
        delay: sleep_until(deadline),
    }
}

/// Creates a future that completes when `future` completes or `duration`
/// elapses.
pub fn timeout<F>(duration: Duration, future: F) -> Timeout<F>
where
    F: Future,
{
    timeout_at(Instant::now() + duration, future)
}

/// Future returned by [`timeout`] and [`timeout_at`].
#[must_use = "futures do nothing unless polled"]
pub struct Timeout<T> {
    value: T,
    delay: Sleep,
}

impl<T> Timeout<T> {
    /// Returns a shared reference to the wrapped value.
    pub fn get_ref(&self) -> &T {
        &self.value
    }

    /// Returns a mutable reference to the wrapped value.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// Returns a pinned mutable reference to the wrapped value.
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
        // `value` is structurally pinned with `self`.
        unsafe { self.map_unchecked_mut(|this| &mut this.value) }
    }

    /// Consumes the timeout and returns the wrapped value.
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Neither field moves while `self` is pinned.
        let this = unsafe { self.get_unchecked_mut() };
        if let Poll::Ready(output) = unsafe { Pin::new_unchecked(&mut this.value) }.poll(cx) {
            return Poll::Ready(Ok(output));
        }

        match unsafe { Pin::new_unchecked(&mut this.delay) }.poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(Elapsed(()))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Timeout<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timeout")
            .field("value", &self.value)
            .field("delay", &self.delay)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_ready_future() {
        let result = crate::Runtime::new()
            .unwrap()
            .block_on(timeout(Duration::from_millis(10), async { 42 }));
        assert!(matches!(result, Ok(42)));
    }

    #[test]
    fn timeout_at_deadline() {
        let deadline = Instant::now() + Duration::from_millis(1);
        let result = crate::Runtime::new()
            .unwrap()
            .block_on(timeout_at(deadline, std::future::pending::<i32>()));
        assert!(matches!(result, Err(Elapsed(()))));
    }
}

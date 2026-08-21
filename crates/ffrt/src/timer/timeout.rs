use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use ffrt_sys::ffrt_usleep;

/// Error returned when a future exceeds the supplied duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

impl fmt::Display for Elapsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operation timed out")
    }
}

impl std::error::Error for Elapsed {}

/// Wrap a future with a timeout based on an absolute deadline.
pub async fn timeout_at<F>(deadline: Instant, future: F) -> Result<F::Output, Elapsed>
where
    F: Future,
{
    Timeout { future, deadline }.await
}

/// Wrap a future with a timeout.
///
/// If the future completes before the deadline, its output is returned as `Ok`.
/// Otherwise `Err(Elapsed)` is returned.
pub async fn timeout<F>(duration: Duration, future: F) -> Result<F::Output, Elapsed>
where
    F: Future,
{
    timeout_at(Instant::now() + duration, future).await
}

struct Timeout<F> {
    future: F,
    deadline: Instant,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: we do not move out of `self`; we only access the pinned future
        // through a reborrowed pin.
        let this = unsafe { self.get_unchecked_mut() };
        let future = unsafe { Pin::new_unchecked(&mut this.future) };

        match future.poll(cx) {
            Poll::Ready(output) => Poll::Ready(Ok(output)),
            Poll::Pending => {
                let now = Instant::now();
                if now >= this.deadline {
                    return Poll::Ready(Err(Elapsed));
                }

                let remaining = this.deadline - now;
                unsafe {
                    ffrt_usleep(remaining.as_micros() as u64);
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

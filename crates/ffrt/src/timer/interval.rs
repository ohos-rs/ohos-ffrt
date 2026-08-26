use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use crate::timer::{Sleep, sleep_until};

/// Behavior used when an interval fires later than scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissedTickBehavior {
    /// Immediately catch up by firing for every missed tick.
    #[default]
    Burst,
    /// Skip missed ticks and delay the next tick by one period.
    Delay,
    /// Skip missed ticks and keep the original period alignment.
    Skip,
}

impl MissedTickBehavior {
    fn next_timeout(self, timeout: Instant, now: Instant, period: Duration) -> Instant {
        match self {
            Self::Burst => timeout + period,
            Self::Delay => now + period,
            Self::Skip => {
                let remainder = (now - timeout).as_nanos() % period.as_nanos();
                now + period - Duration::from_nanos(u64::try_from(remainder).unwrap())
            }
        }
    }
}

impl fmt::Display for MissedTickBehavior {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A periodic timer driven by the FFRT loop.
#[derive(Debug)]
pub struct Interval {
    delay: Pin<Box<Sleep>>,
    period: Duration,
    missed_tick_behavior: MissedTickBehavior,
}

impl Interval {
    fn new(start: Instant, period: Duration) -> Self {
        assert!(!period.is_zero(), "`period` must be non-zero");
        Self {
            delay: Box::pin(sleep_until(start)),
            period,
            missed_tick_behavior: MissedTickBehavior::Burst,
        }
    }

    /// Completes when the next instant in the interval has been reached.
    pub async fn tick(&mut self) -> Instant {
        poll_fn(|cx| self.poll_tick(cx)).await
    }

    /// Polls for the next interval tick.
    pub fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Instant> {
        ready!(self.delay.as_mut().poll(cx));
        let timeout = self.delay.deadline();
        let now = Instant::now();
        let next = if now > timeout + Duration::from_millis(5) {
            self.missed_tick_behavior
                .next_timeout(timeout, now, self.period)
        } else {
            timeout
                .checked_add(self.period)
                .expect("interval deadline overflow")
        };
        self.delay.as_mut().reset(next);
        Poll::Ready(timeout)
    }

    /// Returns the interval period.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Returns the configured missed-tick behavior.
    pub fn missed_tick_behavior(&self) -> MissedTickBehavior {
        self.missed_tick_behavior
    }

    /// Sets the configured missed-tick behavior.
    pub fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
        self.missed_tick_behavior = behavior;
    }

    /// Resets the next tick to one period from now.
    pub fn reset(&mut self) {
        self.reset_at(Instant::now() + self.period);
    }

    /// Resets the next tick to complete immediately.
    pub fn reset_immediately(&mut self) {
        self.reset_at(Instant::now());
    }

    /// Resets the next tick to `after` from now.
    pub fn reset_after(&mut self, after: Duration) {
        self.reset_at(Instant::now() + after);
    }

    /// Resets the next tick to the supplied deadline.
    pub fn reset_at(&mut self, deadline: Instant) {
        self.delay.as_mut().reset(deadline);
    }
}

/// Creates an interval that first fires at `start`.
pub fn interval_at(start: Instant, period: Duration) -> Interval {
    Interval::new(start, period)
}

/// Creates an interval whose first tick completes immediately.
pub fn interval(period: Duration) -> Interval {
    Interval::new(Instant::now(), period)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_ticks_forward() {
        let result = crate::Runtime::new().unwrap().block_on(async move {
            let mut interval = interval(Duration::from_millis(1));
            let first = interval.tick().await;
            let second = interval.tick().await;
            assert!(second >= first);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::timer::r#async::sleep_until;

/// Behavior used when an interval fires later than scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissedTickBehavior {
    /// Immediately catch up by firing for every missed tick.
    Burst,
    /// Skip missed ticks and delay the next tick by one period.
    #[default]
    Delay,
    /// Skip missed ticks and keep the original period alignment.
    Skip,
}

impl fmt::Display for MissedTickBehavior {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissedTickBehavior::Burst => write!(f, "Burst"),
            MissedTickBehavior::Delay => write!(f, "Delay"),
            MissedTickBehavior::Skip => write!(f, "Skip"),
        }
    }
}

/// A tokio-style periodic interval.
pub struct Interval {
    period: Duration,
    next: Instant,
    missed: MissedTickBehavior,
}

impl Interval {
    /// Creates an interval that first fires after `period`.
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            next: Instant::now() + period,
            missed: MissedTickBehavior::Delay,
        }
    }

    /// Returns the interval period.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Returns the configured missed-tick behavior.
    pub fn missed_tick_behavior(&self) -> MissedTickBehavior {
        self.missed
    }

    /// Sets the missed-tick behavior.
    pub fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
        self.missed = behavior;
    }

    /// Resets the interval so the next tick is `period` away from now.
    pub fn reset(&mut self) {
        self.next = Instant::now() + self.period;
    }

    /// Resets the interval so the next tick is `after` away from now.
    pub fn reset_after(&mut self, after: Duration) {
        self.next = Instant::now() + after;
    }

    /// Resets the interval to tick at the supplied instant.
    pub fn reset_at(&mut self, deadline: Instant) {
        self.next = deadline;
    }

    /// Waits for the next tick.
    pub fn tick(&mut self) -> Tick<'_> {
        Tick { interval: self }
    }

    fn advance(&mut self) -> Instant {
        let now = Instant::now();
        if self.next > now {
            self.next
        } else {
            match self.missed {
                MissedTickBehavior::Burst => {
                    let tick = self.next;
                    self.next += self.period;
                    tick
                }
                MissedTickBehavior::Delay => {
                    self.next = now + self.period;
                    now
                }
                MissedTickBehavior::Skip => {
                    let elapsed = now.duration_since(self.next);
                    let missed = (elapsed.as_nanos() / self.period.as_nanos().max(1)) as u32 + 1;
                    self.next += self.period * missed;
                    now
                }
            }
        }
    }
}

/// Creates an interval that fires at `start` and then every `period`.
pub fn interval_at(start: Instant, period: Duration) -> Interval {
    Interval {
        period,
        next: start,
        missed: MissedTickBehavior::Delay,
    }
}

/// Creates an interval that first fires after `period`.
pub fn interval(period: Duration) -> Interval {
    Interval::new(period)
}

/// Future returned by [`Interval::tick`].
#[must_use = "futures do nothing unless polled"]
pub struct Tick<'a> {
    interval: &'a mut Interval,
}

impl Future for Tick<'_> {
    type Output = Instant;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let now = Instant::now();

        if this.interval.next > now {
            let deadline = this.interval.next;
            let mut fut = std::pin::pin!(sleep_until(deadline));
            match fut.as_mut().poll(cx) {
                Poll::Ready(()) => {}
                Poll::Pending => return Poll::Pending,
            }
        }

        Poll::Ready(this.interval.advance())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_ticks_forward() {
        let result = crate::Runtime::new().block_on(async move {
            let mut interval = interval(Duration::from_millis(1));
            let first = interval.tick().await;
            let second = interval.tick().await;
            assert!(second >= first);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }
}

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::looper::LooperTimer;

pub mod sync {
    use ffrt_sys::{ffrt_error_t_ffrt_success, ffrt_usleep};
    use std::time::Duration;

    /// Blocks the current FFRT task for a given duration.
    pub fn sleep(duration: Duration) {
        let ret = unsafe { ffrt_usleep(duration.as_micros() as u64) };

        #[cfg(debug_assertions)]
        assert!(ret == ffrt_error_t_ffrt_success, "Failed to sleep");
    }
}

#[derive(Default)]
struct TimerState {
    fired: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl TimerState {
    fn register(&self, waker: &Waker) {
        let mut slot = self.waker.lock().unwrap();
        if slot
            .as_ref()
            .is_none_or(|registered| !registered.will_wake(waker))
        {
            *slot = Some(waker.clone());
        }
    }

    fn fire(&self) {
        self.fired.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

/// Future returned by [`sleep`] and [`sleep_until`].
///
/// Dropping this value cancels its FFRT loop timer. The timer does not occupy
/// an FFRT worker while it is pending.
#[must_use = "futures do nothing unless polled"]
pub struct Sleep {
    deadline: Instant,
    state: Arc<TimerState>,
    registration: Option<LooperTimer>,
}

impl Sleep {
    fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            state: Arc::new(TimerState::default()),
            registration: None,
        }
    }

    /// Returns the instant at which this sleep completes.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Returns whether the deadline has elapsed.
    pub fn is_elapsed(&self) -> bool {
        self.state.fired.load(Ordering::Acquire) || Instant::now() >= self.deadline
    }

    /// Resets this sleep to a new deadline.
    pub fn reset(self: Pin<&mut Self>, deadline: Instant) {
        // The registration is never structurally pinned.
        let this = unsafe { self.get_unchecked_mut() };
        this.registration.take();
        this.deadline = deadline;
        this.state = Arc::new(TimerState::default());
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // No field is projected as pinned.
        let this = unsafe { self.get_unchecked_mut() };
        if this.state.fired.load(Ordering::Acquire) || Instant::now() >= this.deadline {
            this.registration.take();
            return Poll::Ready(());
        }

        this.state.register(cx.waker());
        if this.registration.is_none() {
            let state = this.state.clone();
            let remaining = this.deadline.saturating_duration_since(Instant::now());
            let timer = crate::reactor::register_timer(remaining, move || state.fire())
                .expect("failed to register FFRT sleep timer");
            this.registration = Some(timer);
        }

        // Close the race where the callback fired between the initial check and
        // waker installation.
        if this.state.fired.load(Ordering::Acquire) || Instant::now() >= this.deadline {
            this.registration.take();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl fmt::Debug for Sleep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sleep")
            .field("deadline", &self.deadline)
            .field("elapsed", &self.is_elapsed())
            .finish()
    }
}

/// Tokio-compatible asynchronous timer entry points.
pub mod r#async {
    use super::{Duration, Instant, Sleep};

    /// Waits for a duration without occupying an FFRT worker.
    pub fn sleep(duration: Duration) -> Sleep {
        sleep_until(Instant::now() + duration)
    }

    /// Waits until the supplied deadline without occupying an FFRT worker.
    pub fn sleep_until(deadline: Instant) -> Sleep {
        Sleep::new(deadline)
    }
}

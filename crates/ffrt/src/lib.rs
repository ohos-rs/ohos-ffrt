//! OpenHarmony FFRT Runtime

mod macros;

pub mod lock;
pub mod looper;
pub mod queue;
pub mod runtime;
pub mod signal;
pub mod task;
pub mod this_task;
pub mod timer;

pub mod sync;

/// Tokio-style time compatibility module.
pub mod time {
    pub use std::time::{Duration, Instant};

    pub use crate::timer::r#async::sleep;
    pub use crate::timer::r#async::sleep_until;
    pub use crate::timer::interval::{Interval, MissedTickBehavior, interval, interval_at};
    pub use crate::timer::sync::sleep as sleep_blocking;
    pub use crate::timer::timeout::{Elapsed, timeout, timeout_at};
}

pub use lock::*;
pub use runtime::*;
pub use signal::*;
pub use task::*;

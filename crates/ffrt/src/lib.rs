//! OpenHarmony FFRT Runtime

extern crate self as ffrt;

#[cfg(feature = "macros")]
pub use ffrt_macros::{main, test};

#[doc(hidden)]
pub use ffrt_macros::__select;

mod macros;

pub mod fs;
pub mod io;
pub mod lock;
pub mod looper;
pub mod net;
pub mod process;
pub mod queue;
pub mod reactor;
pub mod runtime;
pub mod signal;
pub mod task;
pub mod this_task;
pub mod timer;

pub mod sync;

/// Tokio-style time compatibility module.
pub mod time {
    pub mod error {
        pub use crate::timer::timeout::Elapsed;
    }
    pub use std::time::{Duration, Instant};

    pub use crate::timer::Sleep;
    pub use crate::timer::r#async::{sleep, sleep_until};
    pub use crate::timer::interval::{Interval, MissedTickBehavior, interval, interval_at};
    pub use crate::timer::sync::sleep as sleep_blocking;
    pub use crate::timer::timeout::{Elapsed, Timeout, timeout, timeout_at};
}

pub use lock::*;
pub use runtime::*;
pub use signal::*;
pub use task::{Qos, Task, TaskAttr, TaskLocalAccessError, TaskPriority};

/// Returns a pseudo-random starting branch for the unbiased [`select!`] macro.
#[doc(hidden)]
pub fn __select_start(branches: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};

    if branches <= 1 {
        return 0;
    }
    static STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let mut current = STATE.load(Ordering::Relaxed);
    loop {
        let mut next = current;
        next ^= next << 13;
        next ^= next >> 7;
        next ^= next << 17;
        match STATE.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next as usize % branches,
            Err(observed) => current = observed,
        }
    }
}

/// Wraps a branch offset for the [`select!`] macro without exposing a modulo
/// expression to lints in the downstream crate where the macro is expanded.
#[doc(hidden)]
pub fn __select_index(start: usize, offset: usize, branches: usize) -> usize {
    (start + offset) % branches
}

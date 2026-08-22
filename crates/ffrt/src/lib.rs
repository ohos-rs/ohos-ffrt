//! OpenHarmony FFRT Runtime

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

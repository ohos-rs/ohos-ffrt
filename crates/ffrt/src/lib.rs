//! OpenHarmony FFRT Runtime

pub mod lock;
pub mod runtime;
pub mod signal;
pub mod task;
pub mod this_task;
pub mod timer;

/// Tokio-style sync compatibility module.
pub mod sync {
    pub use crate::lock::*;
    pub use crate::signal::*;
}

/// Tokio-style time compatibility module.
pub mod time {
    pub use std::time::{Duration, Instant};

    pub use crate::timer::r#async::sleep;
    pub use crate::timer::r#async::sleep_until;
    pub use crate::timer::sync::sleep as sleep_blocking;
    pub use crate::timer::timeout::{Elapsed, timeout};
}

pub use lock::*;
pub use runtime::*;
pub use signal::*;
pub use task::*;

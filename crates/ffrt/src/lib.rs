//! OpenHarmony FFRT Runtime

pub mod lock;
pub mod runtime;
pub mod signal;
pub mod task;
pub mod timer;

pub use lock::*;
pub use runtime::*;
pub use signal::*;
pub use task::*;

// Run future on default runtime
pub fn block_on<F>(future: F) -> Result<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME
        .read()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.block_on(future)))
        .expect("Access FFRT runtime failed in spawn")
}

/// Spawn a new task on default runtime
pub fn spawn<F>(future: F) -> runtime::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME
        .read()
        .ok()
        .and_then(|rt| rt.as_ref().map(|rt| rt.spawn(future)))
        .expect("Access FFRT runtime failed in spawn")
}

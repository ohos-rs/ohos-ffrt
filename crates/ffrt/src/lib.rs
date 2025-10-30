//! 同步原语

pub mod channel;
pub mod error;
pub mod runtime;
pub mod utils;
pub mod lock;
pub mod task;
pub mod timer;

pub use channel::{Receiver, Sender, channel};
pub use error::*;
pub use runtime::{JoinHandle, Result, Runtime, sleep, wait_all, yield_now};
pub use utils::*;
pub use lock::*;
pub use task::*;
pub use timer::*;

/// 在默认运行时上执行future
pub fn block_on<F>(future: F) -> Result<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    Runtime::new().block_on(future)
}

/// 生成新任务
pub fn spawn<F>(future: F) -> runtime::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    Runtime::new().spawn(future)
}

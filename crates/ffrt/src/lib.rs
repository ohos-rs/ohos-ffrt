//! 同步原语

pub mod channel;
pub mod error;
pub mod mutex;
pub mod runtime;
pub mod rwlock;
pub mod utils;

pub use channel::{Receiver, Sender, channel};
pub use error::*;
pub use mutex::Mutex;
pub use runtime::{JoinHandle, Result, Runtime, sleep, wait_all, yield_now};
pub use rwlock::RwLock;
pub use utils::*;

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

//! OHOS FFRT Runtime
//!
//! 基于 FFRT (Function Flow Runtime) 的异步运行时实现

pub mod error;
pub mod lock;
pub mod runtime;
pub mod signal;
pub mod task;
pub mod timer;
pub mod utils;

pub use error::*;
pub use lock::*;
pub use runtime::{JoinHandle, Result, Runtime, wait_all, yield_now};
pub use signal::*;
pub use task::*;
pub use utils::*;

// 重新导出异步 sleep 为主要的 sleep
pub use runtime::sleep;

// 同步 sleep 从 timer 模块单独导入
pub use timer::sleep as blocking_sleep;

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

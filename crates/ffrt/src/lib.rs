//! OHOS FFRT Runtime
//!
//! 基于 FFRT (Function Flow Runtime) 的异步运行时实现

pub mod lock;
pub mod runtime;
pub mod signal;
pub mod task;
pub mod timer;

pub use lock::*;
pub use runtime::*;
pub use signal::*;
pub use task::*;

// 在默认运行时上执行future
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

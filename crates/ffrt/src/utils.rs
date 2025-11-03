//! 异步工具函数 - timeout, select等

use std::future::Future;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use super::error::RuntimeError;
use super::runtime::Result;

/// 为future添加超时
pub async fn timeout<F: Future>(duration: Duration, _future: F) -> Result<F::Output> {
    let start = Instant::now();

    // 使用简单的轮询方式
    loop {
        if start.elapsed() > duration {
            return Err(RuntimeError::Timeout);
        }
        super::yield_now().await;
    }
}

/// 实现timeout的宏版本
#[macro_export]
macro_rules! timeout {
    ($duration:expr, $future:expr) => {
        $crate::utils::timeout($duration, $future)
    };
}

/// 创建一个noop waker
fn create_noop_waker() -> std::task::Waker {
    use std::task::{RawWaker, RawWakerVTable};

    unsafe fn clone_raw(_data: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    unsafe fn wake_raw(_data: *const ()) {}

    unsafe fn wake_by_ref_raw(_data: *const ()) {}

    unsafe fn drop_raw(_data: *const ()) {}

    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

    unsafe { std::task::Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// Select两个future中最先完成的
pub async fn select<F1, F2>(future1: F1, future2: F2) -> SelectResult<F1::Output, F2::Output>
where
    F1: Future + Unpin,
    F2: Future + Unpin,
{
    use std::pin::pin;

    let mut f1 = pin!(future1);
    let mut f2 = pin!(future2);
    let noop_waker = create_noop_waker();
    let mut cx = Context::from_waker(&noop_waker);

    loop {
        // 首先轮询第一个future
        if let Poll::Ready(val1) = f1.as_mut().poll(&mut cx) {
            return SelectResult::First(val1);
        }

        // 然后轮询第二个future
        if let Poll::Ready(val2) = f2.as_mut().poll(&mut cx) {
            return SelectResult::Second(val2);
        }

        super::yield_now().await;
    }
}

/// Select结果
#[derive(Debug)]
pub enum SelectResult<T1, T2> {
    /// 第一个future完成
    First(T1),
    /// 第二个future完成
    Second(T2),
}

/// Select宏 - 选择最先完成的future
#[macro_export]
macro_rules! select {
    ($future1:expr, $future2:expr) => {
        $crate::utils::select($future1, $future2)
    };
}

/// 并行运行多个future
pub async fn join_all<F, T>(futures: Vec<F>) -> Vec<T>
where
    F: Future<Output = T>,
{
    let mut results = Vec::new();
    for future in futures {
        results.push(future.await);
    }
    results
}

/// 并发运行多个future，返回任意一个成功的结果
pub async fn try_join_all<F, T, E>(futures: Vec<F>) -> std::result::Result<Vec<T>, E>
where
    F: Future<Output = std::result::Result<T, E>>,
{
    let mut results = Vec::new();
    for future in futures {
        match future.await {
            Ok(val) => results.push(val),
            Err(e) => return Err(e),
        }
    }
    Ok(results)
}

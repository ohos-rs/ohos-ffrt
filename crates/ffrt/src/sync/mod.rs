//! Tokio-compatible synchronization primitives backed by FFRT locks.

mod barrier;
pub mod broadcast;
mod mutex;
mod notify;
mod rwlock;
mod semaphore;
pub mod watch;

pub use crate::signal::*;

pub use barrier::{Barrier, BarrierWaitResult};
pub use mutex::{Mutex, MutexGuard, TryLockError};
pub use notify::Notify;
pub use rwlock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
pub use semaphore::{Acquire, AcquireError, Semaphore, SemaphorePermit, TryAcquireError};

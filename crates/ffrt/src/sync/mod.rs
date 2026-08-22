//! Tokio-compatible synchronization primitives backed by FFRT locks.

mod barrier;
pub mod broadcast;
mod mutex;
mod notify;
mod once_cell;
mod rwlock;
mod semaphore;
pub mod watch;

pub use crate::signal::*;

pub use barrier::{Barrier, BarrierWaitResult};
pub use mutex::{Mutex, MutexGuard, OwnedMutexGuard, OwnedMutexLockFuture, TryLockError};
pub use notify::Notify;
pub use once_cell::{GetOrInitFuture, GetOrTryInitFuture, OnceCell, SetError};
pub use rwlock::{
    OwnedRwLockReadFuture, OwnedRwLockReadGuard, OwnedRwLockWriteFuture, OwnedRwLockWriteGuard,
    RwLock, RwLockReadGuard, RwLockWriteGuard,
};
pub use semaphore::{
    Acquire, AcquireError, AcquireOwned, OwnedSemaphorePermit, Semaphore, SemaphorePermit,
    TryAcquireError,
};

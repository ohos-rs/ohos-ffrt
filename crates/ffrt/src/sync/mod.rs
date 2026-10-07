//! Tokio-compatible synchronization primitives backed by FFRT locks.

mod barrier;
pub mod broadcast;
mod mutex;
mod notify;
mod once_cell;
mod rwlock;
mod semaphore;
mod set_once;
pub mod watch;

pub use crate::signal::*;

pub use barrier::{Barrier, BarrierWaitResult};
pub use mutex::{
    MappedMutexGuard, Mutex, MutexGuard, OwnedMappedMutexGuard, OwnedMutexGuard,
    OwnedMutexLockFuture, TryLockError,
};
pub use notify::{Notified, Notify, OwnedNotified};
pub use once_cell::{OnceCell, SetError};
pub use rwlock::{
    OwnedRwLockMappedWriteGuard, OwnedRwLockReadFuture, OwnedRwLockReadGuard,
    OwnedRwLockWriteFuture, OwnedRwLockWriteGuard, RwLock, RwLockMappedWriteGuard, RwLockReadGuard,
    RwLockWriteGuard,
};
pub use semaphore::{
    Acquire, AcquireError, AcquireOwned, OwnedSemaphorePermit, Semaphore, SemaphorePermit,
    TryAcquireError,
};
pub use set_once::{SetOnce, SetOnceError};

/// Named futures returned by synchronization primitives.
pub mod futures {
    pub use super::notify::{Notified, OwnedNotified};
}

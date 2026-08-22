mod attr;
pub mod coop;
mod join_set;
mod local_set;
mod priority;
mod qos;
mod task;
mod task_local;

pub use attr::*;
pub use join_set::{JoinNext, JoinNextWithId, JoinSet};
pub use local_set::{LocalEnterGuard, LocalSet, RunUntil, spawn_local};
pub use priority::*;
pub use qos::*;
pub use task::*;
pub use task_local::{
    AccessError as TaskLocalAccessError, LocalKey, LocalKeyInner, TaskLocalFuture,
};

/// Task-related future types.
pub mod futures {
    pub use super::task_local::TaskLocalFuture;
}

#[deprecated = "Moved to task::coop::Unconstrained"]
pub use coop::Unconstrained;
#[deprecated = "Moved to task::coop::consume_budget"]
pub use coop::consume_budget;
#[deprecated = "Moved to task::coop::unconstrained"]
pub use coop::unconstrained;

pub use crate::runtime::{
    AbortHandle, Id, JoinError, JoinFuture, JoinHandle, block_in_place, id, spawn, spawn_blocking,
    spawn_with_attr, try_id, yield_now,
};

use std::future::Future;
use std::io;

use crate::runtime::Handle;

use super::{JoinHandle, LocalSet};

/// Configures and spawns a task with Tokio-compatible tracing metadata.
#[derive(Default, Debug)]
pub struct Builder<'a> {
    name: Option<&'a str>,
}

impl<'a> Builder<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn name(&self, name: &'a str) -> Self {
        Self { name: Some(name) }
    }

    #[track_caller]
    pub fn spawn<Fut>(self, future: Fut) -> io::Result<JoinHandle<Fut::Output>>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        self.spawn_on(future, &Handle::current())
    }

    #[track_caller]
    pub fn spawn_on<Fut>(self, future: Fut, handle: &Handle) -> io::Result<JoinHandle<Fut::Output>>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        Ok(handle.spawn_named(future, self.name))
    }

    #[track_caller]
    pub fn spawn_local<Fut>(self, future: Fut) -> io::Result<JoinHandle<Fut::Output>>
    where
        Fut: Future + 'static,
        Fut::Output: 'static,
    {
        Ok(super::local_set::spawn_local_named(future, self.name))
    }

    #[track_caller]
    pub fn spawn_local_on<Fut>(
        self,
        future: Fut,
        local_set: &LocalSet,
    ) -> io::Result<JoinHandle<Fut::Output>>
    where
        Fut: Future + 'static,
        Fut::Output: 'static,
    {
        Ok(local_set.spawn_named(future, self.name))
    }

    #[track_caller]
    pub fn spawn_blocking<Function, Output>(
        self,
        function: Function,
    ) -> io::Result<JoinHandle<Output>>
    where
        Function: FnOnce() -> Output + Send + 'static,
        Output: Send + 'static,
    {
        self.spawn_blocking_on(function, &Handle::current())
    }

    #[track_caller]
    pub fn spawn_blocking_on<Function, Output>(
        self,
        function: Function,
        handle: &Handle,
    ) -> io::Result<JoinHandle<Output>>
    where
        Function: FnOnce() -> Output + Send + 'static,
        Output: Send + 'static,
    {
        Ok(handle.spawn_blocking_named(function, self.name))
    }
}

use std::panic::Location;

use super::Id;

/// Tracing context retained for the complete lifetime of a spawned task.
#[cfg(feature = "tracing")]
#[derive(Clone, Debug)]
pub(crate) struct TaskTrace {
    span: tracing::Span,
    id: Id,
}

#[cfg(feature = "tracing")]
impl TaskTrace {
    pub(crate) fn new(
        kind: &'static str,
        name: Option<&str>,
        id: Id,
        size: usize,
        function_type: Option<&'static str>,
        location: &'static Location<'static>,
    ) -> Self {
        let name = name.unwrap_or_default();
        let id_value = id.as_u64();
        let span = if kind == "blocking" {
            tracing::trace_span!(
                target: "tokio::task::blocking",
                "runtime.spawn",
                kind = %kind,
                task.name = %name,
                task.id = id_value,
                "fn" = %function_type.unwrap_or_default(),
                original_size.bytes = Option::<usize>::None,
                size.bytes = size,
                loc.file = location.file(),
                loc.line = location.line(),
                loc.col = location.column(),
            )
        } else {
            tracing::trace_span!(
                target: "tokio::task",
                parent: None,
                "runtime.spawn",
                kind = %kind,
                task.name = %name,
                task.id = id_value,
                original_size.bytes = Option::<usize>::None,
                size.bytes = size,
                loc.file = location.file(),
                loc.line = location.line(),
                loc.col = location.column(),
            )
        };
        Self { span, id }
    }

    #[inline]
    pub(crate) fn in_scope<R>(&self, operation: impl FnOnce() -> R) -> R {
        self.span.in_scope(operation)
    }

    #[inline]
    pub(crate) fn waker_event(&self, operation: &'static str) {
        tracing::trace!(
            target: "tokio::task::waker",
            op = operation,
            task.id = self.id.as_u64(),
        );
    }
}

#[cfg(not(feature = "tracing"))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TaskTrace;

#[cfg(not(feature = "tracing"))]
impl TaskTrace {
    #[inline]
    pub(crate) fn new(
        _kind: &'static str,
        _name: Option<&str>,
        _id: Id,
        _size: usize,
        _function_type: Option<&'static str>,
        _location: &'static Location<'static>,
    ) -> Self {
        Self
    }

    #[inline]
    pub(crate) fn in_scope<R>(&self, operation: impl FnOnce() -> R) -> R {
        operation()
    }

    #[inline]
    pub(crate) fn waker_event(&self, _operation: &'static str) {}
}

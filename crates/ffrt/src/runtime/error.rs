#[derive(Debug, Clone)]
pub enum RuntimeError {
    /// Task cancelled
    Cancelled,
    /// Task panicked
    Panicked(String),
    /// Timeout
    Timeout,
    /// Other error
    Other(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Cancelled => write!(f, "Task cancelled"),
            RuntimeError::Panicked(msg) => write!(f, "Task panicked: {}", msg),
            RuntimeError::Timeout => write!(f, "Operation timeout"),
            RuntimeError::Other(msg) => write!(f, "Runtime error: {}", msg),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl RuntimeError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, RuntimeError::Cancelled)
    }

    pub fn is_panic(&self) -> bool {
        matches!(self, RuntimeError::Panicked(_))
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(error: std::io::Error) -> Self {
        RuntimeError::Other(error.to_string())
    }
}

use super::Id;

enum JoinErrorKind {
    Cancelled,
    Panic(PanicPayload),
    Other(String),
}

struct PanicPayload(Box<dyn std::any::Any + Send + 'static>);

// Shared access only downcasts to Sync string types; the arbitrary payload is
// returned only by consuming this wrapper.
unsafe impl Sync for PanicPayload {}

/// Error returned when a spawned task cannot produce its output.
pub struct JoinError {
    id: Id,
    kind: JoinErrorKind,
}

impl JoinError {
    pub(crate) fn cancelled(id: Id) -> Self {
        Self {
            id,
            kind: JoinErrorKind::Cancelled,
        }
    }

    pub(crate) fn panic(id: Id, payload: Box<dyn std::any::Any + Send + 'static>) -> Self {
        Self {
            id,
            kind: JoinErrorKind::Panic(PanicPayload(payload)),
        }
    }

    pub(crate) fn other(id: Id, message: impl Into<String>) -> Self {
        Self {
            id,
            kind: JoinErrorKind::Other(message.into()),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self.kind, JoinErrorKind::Cancelled)
    }

    pub fn is_panic(&self) -> bool {
        matches!(self.kind, JoinErrorKind::Panic(_))
    }

    pub fn try_into_panic(self) -> Result<Box<dyn std::any::Any + Send + 'static>, JoinError> {
        match self.kind {
            JoinErrorKind::Panic(payload) => Ok(payload.0),
            _ => Err(self),
        }
    }

    pub fn into_panic(self) -> Box<dyn std::any::Any + Send + 'static> {
        self.try_into_panic()
            .expect("JoinError reason is not a panic")
    }

    pub fn id(&self) -> Id {
        self.id
    }
}

fn panic_message(payload: &PanicPayload) -> &str {
    if let Some(message) = payload.0.downcast_ref::<&'static str>() {
        message
    } else if let Some(message) = payload.0.downcast_ref::<String>() {
        message
    } else {
        "task panicked with a non-string payload"
    }
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            JoinErrorKind::Cancelled => write!(f, "task {} was cancelled", self.id),
            JoinErrorKind::Panic(payload) => {
                write!(f, "task {} panicked: {}", self.id, panic_message(payload))
            }
            JoinErrorKind::Other(message) => write!(f, "task {} failed: {message}", self.id),
        }
    }
}

impl std::fmt::Debug for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinError")
            .field("id", &self.id)
            .field("cancelled", &self.is_cancelled())
            .field("panic", &self.is_panic())
            .field("message", &self.to_string())
            .finish()
    }
}

impl std::error::Error for JoinError {}

impl From<JoinError> for std::io::Error {
    fn from(error: JoinError) -> Self {
        let message = if error.is_cancelled() {
            "task was cancelled"
        } else if error.is_panic() {
            "task panicked"
        } else {
            "task failed"
        };
        std::io::Error::other(message)
    }
}

impl From<JoinError> for RuntimeError {
    fn from(error: JoinError) -> Self {
        match error.kind {
            JoinErrorKind::Cancelled => RuntimeError::Cancelled,
            JoinErrorKind::Panic(payload) => RuntimeError::Panicked(panic_message(&payload).into()),
            JoinErrorKind::Other(message) => RuntimeError::Other(message),
        }
    }
}

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
    /// Returns `true` if the task was cancelled.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, RuntimeError::Cancelled)
    }

    /// Returns `true` if the task panicked.
    pub fn is_panic(&self) -> bool {
        matches!(self, RuntimeError::Panicked(_))
    }
}

/// Alias used by the tokio-compatible API.
pub type JoinError = RuntimeError;

impl From<std::io::Error> for RuntimeError {
    fn from(error: std::io::Error) -> Self {
        RuntimeError::Other(error.to_string())
    }
}

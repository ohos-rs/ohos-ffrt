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

impl From<std::io::Error> for RuntimeError {
    fn from(error: std::io::Error) -> Self {
        RuntimeError::Other(error.to_string())
    }
}

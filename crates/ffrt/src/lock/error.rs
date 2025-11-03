#[derive(Debug, Clone)]
pub enum LockError {
    InnerError(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::InnerError(msg) => write!(f, "Inner error: {}", msg),
        }
    }
}

impl std::error::Error for LockError {}

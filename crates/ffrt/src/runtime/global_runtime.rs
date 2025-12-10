use std::sync::{LazyLock, OnceLock, RwLock};

use crate::Runtime;

pub(crate) static RUNTIME: LazyLock<RwLock<Option<Runtime>>> =
    LazyLock::new(|| RwLock::new(Some(Runtime::new())));

static USER_RUNTIME: OnceLock<RwLock<Option<Runtime>>> = OnceLock::new();

static IS_USER_RUNTIME: OnceLock<bool> = OnceLock::new();

/// Create a custom runtime
pub fn create_custom_runtime(rt: Runtime) {
    USER_RUNTIME.get_or_init(|| RwLock::new(Some(rt)));
    IS_USER_RUNTIME.get_or_init(|| true);
}

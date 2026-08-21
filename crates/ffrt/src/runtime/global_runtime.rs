use std::sync::{LazyLock, OnceLock, RwLock};

use crate::Runtime;

pub(crate) static RUNTIME: LazyLock<RwLock<Option<Runtime>>> =
    LazyLock::new(|| RwLock::new(Some(Runtime::new())));

static USER_RUNTIME: OnceLock<RwLock<Option<Runtime>>> = OnceLock::new();

static IS_USER_RUNTIME: OnceLock<bool> = OnceLock::new();

/// Returns the runtime that should be used by the free `spawn`/`block_on`
/// entry points.
pub(crate) fn active_runtime() -> &'static RwLock<Option<Runtime>> {
    if IS_USER_RUNTIME.get().copied().unwrap_or(false) {
        USER_RUNTIME
            .get()
            .expect("custom runtime flag set before custom runtime initialized")
    } else {
        &RUNTIME
    }
}

/// Create a custom runtime used by the free `spawn` and `block_on` functions.
pub fn create_custom_runtime(rt: Runtime) {
    USER_RUNTIME.get_or_init(|| RwLock::new(Some(rt)));
    IS_USER_RUNTIME.get_or_init(|| true);
}

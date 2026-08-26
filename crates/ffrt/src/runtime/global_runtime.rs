use std::sync::{LazyLock, OnceLock};

use crate::Runtime;
use crate::lock::Mutex;

pub(crate) static RUNTIME: LazyLock<Mutex<Option<Runtime>>> =
    LazyLock::new(|| Mutex::new(Some(Runtime)));

static USER_RUNTIME: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();

static IS_USER_RUNTIME: OnceLock<bool> = OnceLock::new();

/// Returns the runtime that should be used by the free `spawn`/`block_on`
/// entry points.
pub(crate) fn active_runtime() -> &'static Mutex<Option<Runtime>> {
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
    USER_RUNTIME.get_or_init(|| Mutex::new(Some(rt)));
    IS_USER_RUNTIME.get_or_init(|| true);
}

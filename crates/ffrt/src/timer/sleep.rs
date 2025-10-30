use ohos_ffrt_sys::{ffrt_error_t_ffrt_success, ffrt_usleep};
use std::time::Duration;

/// Sleep for a given duration
pub fn sleep(duration: Duration) {
    let ret = unsafe { ffrt_usleep(duration.as_micros() as u64) };

    #[cfg(debug_assertions)]
    assert!(ret == ffrt_error_t_ffrt_success, "Failed to sleep");
}

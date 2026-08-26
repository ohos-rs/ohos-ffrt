//! Helpers for querying and updating the currently executing FFRT task.

use ffrt_sys::{
    ffrt_error_t_ffrt_success, ffrt_this_task_get_id, ffrt_this_task_get_qos,
    ffrt_this_task_update_qos,
};

use crate::Qos;

/// Returns the ID of the current FFRT task.
pub fn get_id() -> u64 {
    unsafe { ffrt_this_task_get_id() }
}

/// Returns the QoS of the current FFRT task.
pub fn get_qos() -> Qos {
    let qos = unsafe { ffrt_this_task_get_qos() };
    qos.into()
}

/// Updates the QoS of the current FFRT task.
pub fn update_qos(qos: Qos) -> Result<(), i32> {
    let ret = unsafe { ffrt_this_task_update_qos(qos.into()) };
    if ret == ffrt_error_t_ffrt_success {
        Ok(())
    } else {
        Err(ret)
    }
}

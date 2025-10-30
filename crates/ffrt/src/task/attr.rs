use std::ptr;

use ohos_ffrt_sys::{
    ffrt_error_t_ffrt_success, ffrt_task_attr_get_qos, ffrt_task_attr_init, ffrt_task_attr_set_qos,
    ffrt_task_attr_t,
};

use crate::Qos;

#[derive(Debug, Clone, Copy)]
pub struct TaskAttr {
    inner: ffrt_task_attr_t,
}

impl TaskAttr {
    pub fn new() -> Self {
        let mut inner: *mut ffrt_task_attr_t = ptr::null_mut();
        let ret = unsafe { ffrt_task_attr_init(&mut inner) };
        #[cfg(debug_assertions)]
        assert!(
            ret == ffrt_error_t_ffrt_success,
            "Failed to initialize task attribute"
        );
        Self { inner }
    }

    pub fn set_name(&self, name: &str) {
        let ret = unsafe { ffrt_task_attr_set_name(self.inner, name.as_ptr()) };
        #[cfg(debug_assertions)]
        assert!(
            ret == ffrt_error_t_ffrt_success,
            "Failed to set task attribute name"
        );
    }

    pub fn get_name(&self) -> &str {
        let name = unsafe { ffrt_task_attr_get_name(self.inner) };
        unsafe { std::ffi::CStr::from_ptr(name).to_str().unwrap() }
    }

    pub fn set_qos(&self, qos: TaskQos) {
        let ret = unsafe { ffrt_task_attr_set_qos(self.inner, qos.into()) };
        #[cfg(debug_assertions)]
        assert!(
            ret == ffrt_error_t_ffrt_success,
            "Failed to set task attribute qos"
        );
    }

    pub fn get_qos(&self) -> TaskQos {
        let qos = unsafe { ffrt_task_attr_get_qos(self.inner) };
        qos.into()
    }
}

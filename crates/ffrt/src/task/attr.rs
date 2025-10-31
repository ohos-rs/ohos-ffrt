use std::ptr::{self, NonNull};

use ohos_ffrt_sys::{
    ffrt_error_t_ffrt_success, ffrt_task_attr_get_delay, ffrt_task_attr_get_name,
    ffrt_task_attr_get_qos, ffrt_task_attr_get_queue_priority, ffrt_task_attr_get_stack_size,
    ffrt_task_attr_init, ffrt_task_attr_set_delay, ffrt_task_attr_set_name, ffrt_task_attr_set_qos,
    ffrt_task_attr_set_queue_priority, ffrt_task_attr_set_stack_size, ffrt_task_attr_t,
};

use crate::{Qos, TaskPriority};

#[derive(Debug, Clone, Copy)]
pub struct TaskAttr {
    pub(crate) inner: NonNull<ffrt_task_attr_t>,
}

impl TaskAttr {
    pub fn new() -> Self {
        use std::mem::MaybeUninit;

        let mut uninit = Box::new(MaybeUninit::<ffrt_task_attr_t>::uninit());

        let ret = unsafe { ffrt_task_attr_init(uninit.as_mut_ptr()) };

        #[cfg(debug_assertions)]
        assert!(
            ret == ffrt_error_t_ffrt_success,
            "Failed to initialize task attribute"
        );

        let inner = unsafe { uninit.assume_init() };
        let ptr = Box::into_raw(inner);

        Self {
            inner: unsafe { NonNull::new_unchecked(ptr) },
        }
    }

    pub fn set_name(&self, name: &str) {
        unsafe { ffrt_task_attr_set_name(self.inner.as_ptr(), name.as_ptr() as _) };
    }

    pub fn get_name(&self) -> &str {
        let name = unsafe { ffrt_task_attr_get_name(self.inner.as_ptr()) };
        unsafe { std::ffi::CStr::from_ptr(name).to_str().unwrap() }
    }

    pub fn set_qos(&self, qos: Qos) {
        unsafe { ffrt_task_attr_set_qos(self.inner.as_ptr(), qos.into()) };
    }

    pub fn get_qos(&self) -> Qos {
        let qos = unsafe { ffrt_task_attr_get_qos(self.inner.as_ptr()) };
        qos.into()
    }

    pub fn set_delay(&self, delay: u64) {
        unsafe { ffrt_task_attr_set_delay(self.inner.as_ptr(), delay) };
    }

    pub fn get_delay(&self) -> u64 {
        unsafe { ffrt_task_attr_get_delay(self.inner.as_ptr()) }
    }

    pub fn set_priority(&self, priority: TaskPriority) {
        unsafe { ffrt_task_attr_set_queue_priority(self.inner.as_ptr(), priority.into()) };
    }

    pub fn get_priority(&self) -> TaskPriority {
        let priority = unsafe { ffrt_task_attr_get_queue_priority(self.inner.as_ptr()) };
        priority.into()
    }

    pub fn set_stack_size(&self, stack_size: u64) {
        unsafe { ffrt_task_attr_set_stack_size(self.inner.as_ptr(), stack_size) };
    }

    pub fn get_stack_size(&self) -> u64 {
        unsafe { ffrt_task_attr_get_stack_size(self.inner.as_ptr()) }
    }
}

impl Default for TaskAttr {
    fn default() -> Self {
        Self::new()
    }
}

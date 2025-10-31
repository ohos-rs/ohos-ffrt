use std::ptr;
use std::sync::Arc;

use ohos_ffrt_sys::{
    ffrt_alloc_auto_managed_function_storage_base, ffrt_function_header_t,
    ffrt_function_kind_t_ffrt_function_kind_general, ffrt_submit_h_base, ffrt_task_handle_destroy,
    ffrt_task_handle_t,
};

use crate::TaskAttr;

struct FuncWrapper {
    func: Box<dyn FnOnce() + Send + 'static>,
}

#[repr(C)]
struct TaskWrapper {
    header: ffrt_function_header_t,
    func_ptr: *mut FuncWrapper,
}

#[derive(Clone)]
pub struct Task {
    handle: Option<Arc<TaskHandle>>,
    attr: TaskAttr,
}

struct TaskHandle(ffrt_task_handle_t);

impl Drop for TaskHandle {
    fn drop(&mut self) {
        unsafe { ffrt_task_handle_destroy(self.0) };
    }
}

impl Task {
    pub fn new(attr: TaskAttr) -> Self {
        Self { attr, handle: None }
    }

    pub fn default() -> Self {
        Self::new(TaskAttr::new())
    }

    pub fn submit<F>(&self, func: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let storage = unsafe {
            ffrt_alloc_auto_managed_function_storage_base(
                ffrt_function_kind_t_ffrt_function_kind_general,
            )
        } as *mut TaskWrapper;

        let func_wrapper = Box::new(FuncWrapper {
            func: Box::new(func),
        });
        let func_ptr = Box::into_raw(func_wrapper);

        unsafe {
            ptr::write(
                ptr::addr_of_mut!((*storage).header),
                ffrt_function_header_t {
                    exec: Some(task_exec),
                    destroy: Some(task_destroy),
                    reserve: [0; 2],
                },
            );
            ptr::write(ptr::addr_of_mut!((*storage).func_ptr), func_ptr);
        }

        let _ = unsafe {
            ffrt_submit_h_base(
                storage as *mut ffrt_function_header_t,
                ptr::null(),
                ptr::null(),
                self.attr.inner.as_ptr(),
            )
        };
    }
}

unsafe extern "C" fn task_exec(arg: *mut std::ffi::c_void) {
    let wrapper = arg as *mut TaskWrapper;
    if wrapper.is_null() {
        return;
    }

    let func_ptr = unsafe { ptr::read(ptr::addr_of!((*wrapper).func_ptr)) };
    if func_ptr.is_null() {
        return;
    }

    let func_wrapper = unsafe { Box::from_raw(func_ptr) };
    (func_wrapper.func)();

    unsafe {
        ptr::write(ptr::addr_of_mut!((*wrapper).func_ptr), ptr::null_mut());
    }
}

unsafe extern "C" fn task_destroy(arg: *mut std::ffi::c_void) {
    let wrapper = arg as *mut TaskWrapper;
    if wrapper.is_null() {
        return;
    }

    let func_ptr = unsafe { ptr::read(ptr::addr_of!((*wrapper).func_ptr)) };
    if !func_ptr.is_null() {
        let _ = unsafe { Box::from_raw(func_ptr) };
    }

    unsafe { ptr::drop_in_place(wrapper) };
}

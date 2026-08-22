//! Safe wrappers around FFRT queues.

use std::ffi::CString;
use std::ptr::{self, NonNull};

use ffrt_sys::{
    ffrt_alloc_auto_managed_function_storage_base, ffrt_error_t_ffrt_success,
    ffrt_function_header_t, ffrt_function_kind_t_ffrt_function_kind_queue, ffrt_get_current_queue,
    ffrt_get_main_queue, ffrt_queue_attr_destroy, ffrt_queue_attr_get_max_concurrency,
    ffrt_queue_attr_get_qos, ffrt_queue_attr_get_timeout, ffrt_queue_attr_init,
    ffrt_queue_attr_set_max_concurrency, ffrt_queue_attr_set_qos, ffrt_queue_attr_set_timeout,
    ffrt_queue_attr_t, ffrt_queue_cancel, ffrt_queue_create, ffrt_queue_destroy, ffrt_queue_submit,
    ffrt_queue_submit_h, ffrt_queue_t, ffrt_queue_type_t, ffrt_queue_type_t_ffrt_queue_concurrent,
    ffrt_queue_type_t_ffrt_queue_serial, ffrt_queue_wait, ffrt_task_handle_destroy,
    ffrt_task_handle_t,
};
#[cfg(feature = "api-20")]
use ffrt_sys::{ffrt_queue_attr_get_thread_mode, ffrt_queue_attr_set_thread_mode};

use crate::{Qos, TaskAttr};

/// FFRT queue type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueType {
    /// Tasks in the queue run serially.
    Serial,
    /// Tasks in the queue may run concurrently.
    Concurrent,
}

impl From<QueueType> for ffrt_queue_type_t {
    fn from(value: QueueType) -> Self {
        match value {
            QueueType::Serial => ffrt_queue_type_t_ffrt_queue_serial,
            QueueType::Concurrent => ffrt_queue_type_t_ffrt_queue_concurrent,
        }
    }
}

/// Owned FFRT queue attributes.
pub struct QueueAttr {
    inner: NonNull<ffrt_queue_attr_t>,
}

impl QueueAttr {
    /// Creates queue attributes.
    pub fn new() -> Self {
        use std::mem::MaybeUninit;

        let mut uninit = Box::new(MaybeUninit::<ffrt_queue_attr_t>::uninit());
        let ret = unsafe { ffrt_queue_attr_init(uninit.as_mut_ptr()) };
        assert!(
            ret == ffrt_error_t_ffrt_success,
            "failed to init queue attr"
        );

        let inner = unsafe { uninit.assume_init() };
        Self {
            inner: unsafe { NonNull::new_unchecked(Box::into_raw(inner)) },
        }
    }

    /// Sets the queue QoS.
    pub fn set_qos(&self, qos: Qos) {
        unsafe { ffrt_queue_attr_set_qos(self.inner.as_ptr(), qos.into()) };
    }

    /// Gets the queue QoS.
    pub fn get_qos(&self) -> Qos {
        let qos = unsafe { ffrt_queue_attr_get_qos(self.inner.as_ptr()) };
        qos.into()
    }

    /// Sets the serial-queue execution timeout, in microseconds.
    pub fn set_timeout(&self, timeout_us: u64) {
        unsafe { ffrt_queue_attr_set_timeout(self.inner.as_ptr(), timeout_us) };
    }

    /// Gets the serial-queue execution timeout, in microseconds.
    pub fn get_timeout(&self) -> u64 {
        unsafe { ffrt_queue_attr_get_timeout(self.inner.as_ptr()) }
    }

    /// Sets the maximum concurrency.
    pub fn set_max_concurrency(&self, max_concurrency: i32) {
        unsafe { ffrt_queue_attr_set_max_concurrency(self.inner.as_ptr(), max_concurrency) };
    }

    /// Gets the maximum concurrency.
    pub fn get_max_concurrency(&self) -> i32 {
        unsafe { ffrt_queue_attr_get_max_concurrency(self.inner.as_ptr()) }
    }

    /// Sets whether tasks run in native-thread mode.
    #[cfg(feature = "api-20")]
    pub fn set_thread_mode(&self, mode: bool) {
        unsafe { ffrt_queue_attr_set_thread_mode(self.inner.as_ptr(), mode) };
    }

    /// Gets whether tasks run in native-thread mode.
    #[cfg(feature = "api-20")]
    pub fn get_thread_mode(&self) -> bool {
        unsafe { ffrt_queue_attr_get_thread_mode(self.inner.as_ptr()) }
    }
}

impl Default for QueueAttr {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for QueueAttr {
    fn drop(&mut self) {
        unsafe {
            ffrt_queue_attr_destroy(self.inner.as_ptr());
            let _ = Box::from_raw(self.inner.as_ptr());
        }
    }
}

struct QueueFuncWrapper {
    func: Box<dyn FnOnce() + Send + 'static>,
}

#[repr(C)]
struct QueueTaskWrapper {
    header: ffrt_function_header_t,
    func_ptr: *mut QueueFuncWrapper,
}

unsafe extern "C" fn queue_task_exec(arg: *mut std::ffi::c_void) {
    let wrapper = arg as *mut QueueTaskWrapper;
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

unsafe extern "C" fn queue_task_destroy(arg: *mut std::ffi::c_void) {
    let wrapper = arg as *mut QueueTaskWrapper;
    if wrapper.is_null() {
        return;
    }

    let func_ptr = unsafe { ptr::read(ptr::addr_of!((*wrapper).func_ptr)) };
    if !func_ptr.is_null() {
        let _ = unsafe { Box::from_raw(func_ptr) };
    }

    unsafe { ptr::drop_in_place(wrapper) };
}

fn prepare_task<F>(func: F) -> *mut QueueTaskWrapper
where
    F: FnOnce() + Send + 'static,
{
    let storage = unsafe {
        ffrt_alloc_auto_managed_function_storage_base(ffrt_function_kind_t_ffrt_function_kind_queue)
    } as *mut QueueTaskWrapper;
    assert!(!storage.is_null(), "failed to allocate FFRT queue task");

    let func_wrapper = Box::new(QueueFuncWrapper {
        func: Box::new(func),
    });
    let func_ptr = Box::into_raw(func_wrapper);

    unsafe {
        ptr::write(
            ptr::addr_of_mut!((*storage).header),
            ffrt_function_header_t {
                exec: Some(queue_task_exec),
                destroy: Some(queue_task_destroy),
                reserve: [0; 2],
            },
        );
        ptr::write(ptr::addr_of_mut!((*storage).func_ptr), func_ptr);
    }

    storage
}

/// A handle to an FFRT queue.
pub struct Queue {
    inner: ffrt_queue_t,
    owned: bool,
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

impl Queue {
    pub(crate) fn as_raw(&self) -> ffrt_queue_t {
        self.inner
    }

    /// Creates an FFRT queue.
    pub fn new(queue_type: QueueType, name: &str, attr: Option<&QueueAttr>) -> Self {
        let name = CString::new(name).expect("queue name must not contain NUL");
        let queue = unsafe {
            ffrt_queue_create(
                queue_type.into(),
                name.as_ptr(),
                attr.map(|a| a.inner.as_ptr()).unwrap_or(ptr::null_mut()),
            )
        };
        assert!(!queue.is_null(), "failed to create FFRT queue");

        Self {
            inner: queue,
            owned: true,
        }
    }

    fn from_borrowed(queue: ffrt_queue_t) -> Self {
        assert!(!queue.is_null(), "FFRT queue handle is null");
        Self {
            inner: queue,
            owned: false,
        }
    }

    /// Returns the application main-thread queue.
    pub fn main_queue() -> Self {
        Self::from_borrowed(unsafe { ffrt_get_main_queue() })
    }

    /// Returns the current application worker-thread queue.
    pub fn current_queue() -> Self {
        Self::from_borrowed(unsafe { ffrt_get_current_queue() })
    }

    /// Submits a task to the queue.
    pub fn submit<F>(&self, func: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.submit_with_attr(func, None);
    }

    /// Submits a task with task attributes to the queue.
    pub fn submit_with_attr<F>(&self, func: F, attr: Option<&TaskAttr>)
    where
        F: FnOnce() + Send + 'static,
    {
        let task = prepare_task(func);
        unsafe {
            ffrt_queue_submit(
                self.inner,
                task as *mut ffrt_function_header_t,
                attr.map(|a| a.inner.as_ptr()).unwrap_or(ptr::null_mut()),
            );
        }
    }

    /// Submits a task and returns a handle for waiting/cancelling it.
    pub fn submit_h<F>(&self, func: F) -> QueueTaskHandle
    where
        F: FnOnce() + Send + 'static,
    {
        self.submit_h_with_attr(func, None)
    }

    /// Submits a task with attributes and returns a handle.
    pub fn submit_h_with_attr<F>(&self, func: F, attr: Option<&TaskAttr>) -> QueueTaskHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let task = prepare_task(func);
        let handle = unsafe {
            ffrt_queue_submit_h(
                self.inner,
                task as *mut ffrt_function_header_t,
                attr.map(|a| a.inner.as_ptr()).unwrap_or(ptr::null_mut()),
            )
        };
        QueueTaskHandle::new(handle)
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        if self.owned {
            unsafe { ffrt_queue_destroy(self.inner) };
        }
    }
}

/// Handle to a task submitted to an FFRT queue.
pub struct QueueTaskHandle {
    inner: ffrt_task_handle_t,
}

impl QueueTaskHandle {
    fn new(handle: ffrt_task_handle_t) -> Self {
        assert!(!handle.is_null(), "FFRT queue task handle is null");
        Self { inner: handle }
    }

    /// Waits until the task completes.
    pub fn wait(&self) {
        unsafe { ffrt_queue_wait(self.inner) };
    }

    /// Cancels the queued task.
    pub fn cancel(&self) -> bool {
        let ret = unsafe { ffrt_queue_cancel(self.inner) };
        ret == ffrt_error_t_ffrt_success
    }
}

impl Drop for QueueTaskHandle {
    fn drop(&mut self) {
        unsafe { ffrt_task_handle_destroy(self.inner) };
    }
}

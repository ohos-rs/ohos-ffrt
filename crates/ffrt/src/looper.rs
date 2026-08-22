//! Safe wrappers around the FFRT event loop.

use std::os::raw::{c_char, c_int, c_void};

use ffrt_sys::{
    ffrt_loop_create, ffrt_loop_destroy, ffrt_loop_epoll_ctl, ffrt_loop_run, ffrt_loop_stop,
    ffrt_loop_t, ffrt_loop_timer_start, ffrt_loop_timer_stop, ffrt_poller_cb, ffrt_timer_cb,
    ffrt_timer_t,
};

use crate::queue::Queue;

/// `epoll_ctl` ADD operation.
pub const EPOLL_CTL_ADD: c_int = 1;
/// `epoll_ctl` DEL operation.
pub const EPOLL_CTL_DEL: c_int = 2;
/// `epoll_ctl` MOD operation.
pub const EPOLL_CTL_MOD: c_int = 3;

/// Data may be read.
pub const EPOLLIN: u32 = 0x001;
/// Data may be written.
pub const EPOLLOUT: u32 = 0x004;
/// Error condition.
pub const EPOLLERR: u32 = 0x008;
/// Hang up.
pub const EPOLLHUP: u32 = 0x010;

/// An owned FFRT event loop.
pub struct Looper {
    inner: ffrt_loop_t,
}

unsafe impl Send for Looper {}
unsafe impl Sync for Looper {}

impl Looper {
    /// Creates a loop backed by `queue`.
    pub fn new(queue: &Queue) -> Self {
        let inner = unsafe { ffrt_loop_create(queue.as_raw()) };
        assert!(!inner.is_null(), "failed to create FFRT loop");
        Self { inner }
    }

    /// Starts the loop. This call blocks until [`Looper::stop`] is called.
    pub fn run(&self) -> Result<(), c_int> {
        let ret = unsafe { ffrt_loop_run(self.inner) };
        if ret == 0 { Ok(()) } else { Err(ret) }
    }

    /// Stops a running loop.
    pub fn stop(&self) {
        unsafe { ffrt_loop_stop(self.inner) };
    }

    /// Registers, modifies, or removes an epoll fd on the loop.
    ///
    /// # Safety
    ///
    /// `data` must remain valid until the callback is removed with
    /// `EPOLL_CTL_DEL` or the loop is destroyed.
    pub unsafe fn epoll_ctl(
        &self,
        op: c_int,
        fd: c_int,
        events: u32,
        data: *mut c_void,
        cb: ffrt_poller_cb,
    ) -> Result<(), c_int> {
        let ret = unsafe { ffrt_loop_epoll_ctl(self.inner, op, fd, events, data, cb) };
        if ret == 0 { Ok(()) } else { Err(ret) }
    }

    /// Starts a timer on the loop.
    pub fn timer_start<F>(&self, timeout_ms: u64, repeat: bool, func: F) -> LooperTimer
    where
        F: FnMut() + Send + 'static,
    {
        struct TimerData<F> {
            func: F,
            repeat: bool,
        }

        unsafe extern "C" fn timer_cb<F: FnMut()>(data: *mut c_void) {
            if data.is_null() {
                return;
            }

            let mut timer_data = unsafe { Box::from_raw(data.cast::<TimerData<F>>()) };
            (timer_data.func)();
            if timer_data.repeat {
                let _ = Box::into_raw(timer_data);
            }
        }

        let data = Box::into_raw(Box::new(TimerData { func, repeat }));

        let handle = unsafe {
            ffrt_loop_timer_start(
                self.inner,
                timeout_ms,
                data.cast::<c_void>(),
                Some(timer_cb::<F>),
                repeat,
            )
        };

        LooperTimer {
            looper: self.inner,
            inner: handle,
        }
    }
}

impl Drop for Looper {
    fn drop(&mut self) {
        unsafe { ffrt_loop_destroy(self.inner) };
    }
}

/// Handle to a timer running on a [`Looper`].
pub struct LooperTimer {
    looper: ffrt_loop_t,
    inner: ffrt_timer_t,
}

impl LooperTimer {
    /// Stops the timer.
    pub fn stop(&mut self) -> Result<(), c_int> {
        let ret = unsafe { ffrt_loop_timer_stop(self.looper, self.inner) };
        if ret == 0 { Ok(()) } else { Err(ret) }
    }
}

impl Drop for LooperTimer {
    fn drop(&mut self) {
        let _ = unsafe { ffrt_loop_timer_stop(self.looper, self.inner) };
    }
}

// Keep the callback types in scope for callers that use raw fds.
#[allow(dead_code)]
fn _ffrt_callback_types(_: ffrt_poller_cb, _: ffrt_timer_cb, _: *const c_char) {}

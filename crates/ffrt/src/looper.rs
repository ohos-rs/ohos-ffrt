//! Safe wrappers around the FFRT event loop.

use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
/// High-priority data may be read.
pub const EPOLLPRI: u32 = 0x002;
/// Data may be written.
pub const EPOLLOUT: u32 = 0x004;
/// Error condition.
pub const EPOLLERR: u32 = 0x008;
/// Hang up.
pub const EPOLLHUP: u32 = 0x010;

/// An owned FFRT event loop.
pub struct Looper {
    inner: Arc<LoopInner>,
}

struct LoopInner {
    handle: ffrt_loop_t,
    _queue: Queue,
    running: AtomicBool,
}

unsafe impl Send for LoopInner {}
unsafe impl Sync for LoopInner {}

unsafe impl Send for Looper {}
unsafe impl Sync for Looper {}

impl Looper {
    /// Creates a loop backed by `queue`.
    pub fn new(queue: &Queue) -> Self {
        let inner = unsafe { ffrt_loop_create(queue.as_raw()) };
        assert!(!inner.is_null(), "failed to create FFRT loop");
        Self {
            inner: Arc::new(LoopInner {
                handle: inner,
                _queue: queue.clone(),
                running: AtomicBool::new(false),
            }),
        }
    }

    /// Starts the loop. This call blocks until [`Looper::stop`] is called.
    pub fn run(&self) -> Result<(), c_int> {
        if self.inner.running.swap(true, Ordering::AcqRel) {
            return Err(-1);
        }
        let ret = unsafe { ffrt_loop_run(self.inner.handle) };
        self.inner.running.store(false, Ordering::Release);
        if ret == 0 { Ok(()) } else { Err(ret) }
    }

    /// Stops a running loop.
    pub fn stop(&self) {
        unsafe { ffrt_loop_stop(self.inner.handle) };
    }

    /// Registers, modifies, or removes an epoll fd on the loop.
    ///
    /// # Safety
    ///
    /// `data` must remain valid until every callback that could have been
    /// returned by the loop's current event batch has finished. `EPOLL_CTL_DEL`
    /// does not wait for those callbacks. The loop must outlive registrations.
    pub unsafe fn epoll_ctl(
        &self,
        op: c_int,
        fd: c_int,
        events: u32,
        data: *mut c_void,
        cb: ffrt_poller_cb,
    ) -> Result<(), c_int> {
        let ret = unsafe { ffrt_loop_epoll_ctl(self.inner.handle, op, fd, events, data, cb) };
        if ret == 0 { Ok(()) } else { Err(ret) }
    }

    /// Tries to start a timer on the loop.
    pub fn try_timer_start<F>(
        &self,
        timeout_ms: u64,
        repeat: bool,
        func: F,
    ) -> Result<LooperTimer, c_int>
    where
        F: FnMut() + Send + 'static,
    {
        let data = Arc::new(TimerData {
            func: Mutex::new(Box::new(func)),
            repeat,
            registration_owned: AtomicBool::new(true),
        });
        // FFRT owns one strong reference until a one-shot callback fires or the
        // registration is successfully stopped.
        let callback_data = Arc::into_raw(data.clone());

        let handle = unsafe {
            ffrt_loop_timer_start(
                self.inner.handle,
                timeout_ms,
                callback_data.cast_mut().cast::<c_void>(),
                Some(timer_callback),
                repeat,
            )
        };

        if handle < 0 {
            data.release_registration();
            return Err(-1);
        }

        Ok(LooperTimer {
            looper: self.inner.clone(),
            inner: handle,
            data,
            active: true,
        })
    }

    /// Starts a timer on the loop.
    ///
    /// Panics if FFRT cannot allocate the timer. Code that needs to surface the
    /// allocation failure should use [`Looper::try_timer_start`].
    pub fn timer_start<F>(&self, timeout_ms: u64, repeat: bool, func: F) -> LooperTimer
    where
        F: FnMut() + Send + 'static,
    {
        self.try_timer_start(timeout_ms, repeat, func)
            .expect("failed to start FFRT loop timer")
    }
}

impl Drop for LoopInner {
    fn drop(&mut self) {
        unsafe { ffrt_loop_destroy(self.handle) };
    }
}

/// Handle to a timer running on a [`Looper`].
pub struct LooperTimer {
    looper: Arc<LoopInner>,
    inner: ffrt_timer_t,
    data: Arc<TimerData>,
    active: bool,
}

impl LooperTimer {
    /// Stops the timer.
    pub fn stop(&mut self) -> Result<(), c_int> {
        if !self.active || !self.data.registration_owned.load(Ordering::Acquire) {
            self.active = false;
            return Ok(());
        }

        let ret = unsafe { ffrt_loop_timer_stop(self.looper.handle, self.inner) };
        if ret == 0 {
            self.active = false;
            self.data.release_registration();
            Ok(())
        } else if !self.data.registration_owned.load(Ordering::Acquire) {
            self.active = false;
            Ok(())
        } else {
            Err(ret)
        }
    }
}

impl Drop for LooperTimer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

unsafe impl Send for LooperTimer {}
unsafe impl Sync for LooperTimer {}

struct TimerData {
    func: Mutex<Box<dyn FnMut() + Send + 'static>>,
    repeat: bool,
    registration_owned: AtomicBool,
}

impl TimerData {
    fn release_registration(&self) {
        if self.registration_owned.swap(false, Ordering::AcqRel) {
            // This balances the Arc::into_raw in try_timer_start. The caller
            // holds another Arc while executing this method.
            unsafe { Arc::decrement_strong_count(self as *const Self) };
        }
    }
}

unsafe extern "C" fn timer_callback(data: *mut c_void) {
    if data.is_null() {
        return;
    }

    let pointer = data.cast::<TimerData>();
    // A temporary strong reference protects the callback against a concurrent
    // successful timer_stop releasing FFRT's registration reference.
    unsafe { Arc::increment_strong_count(pointer) };
    let timer = unsafe { Arc::from_raw(pointer) };
    if let Ok(mut func) = timer.func.lock() {
        (func)();
    }
    if !timer.repeat {
        timer.release_registration();
    }
}

// Keep the callback types in scope for callers that use raw fds.
#[allow(dead_code)]
fn _ffrt_callback_types(_: ffrt_poller_cb, _: ffrt_timer_cb, _: *const c_char) {}

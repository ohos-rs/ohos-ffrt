use std::{
    cell::UnsafeCell,
    ptr::NonNull,
    sync::Arc,
    task::{RawWaker, RawWakerVTable, Waker},
};

use ohos_ffrt_sys::{
    ffrt_cond_destroy, ffrt_cond_init, ffrt_cond_signal, ffrt_cond_t, ffrt_cond_timedwait,
    ffrt_cond_wait, ffrt_mutex_destroy, ffrt_mutex_init, ffrt_mutex_lock, ffrt_mutex_t,
    ffrt_mutex_unlock, timespec,
};

/// Waker 状态，使用 FFRT 同步原语
pub struct WakerState {
    mutex: NonNull<ffrt_mutex_t>,
    cond: NonNull<ffrt_cond_t>,
    woken: UnsafeCell<bool>,
}

impl WakerState {
    pub fn new() -> Self {
        use std::mem::MaybeUninit;

        let mut uninit_mutex = Box::new(MaybeUninit::<ffrt_mutex_t>::uninit());
        let mut uninit_cond = Box::new(MaybeUninit::<ffrt_cond_t>::uninit());

        unsafe {
            ffrt_mutex_init(uninit_mutex.as_mut_ptr(), std::ptr::null());
            ffrt_cond_init(uninit_cond.as_mut_ptr(), std::ptr::null());
        }

        let mutex = unsafe { uninit_mutex.assume_init() };
        let cond = unsafe { uninit_cond.assume_init() };

        Self {
            mutex: unsafe { NonNull::new_unchecked(Box::into_raw(mutex)) },
            cond: unsafe { NonNull::new_unchecked(Box::into_raw(cond)) },
            woken: UnsafeCell::new(false),
        }
    }

    pub fn wake(&self) {
        unsafe {
            ffrt_mutex_lock(self.mutex.as_ptr());
            *self.woken.get() = true;
            ffrt_cond_signal(self.cond.as_ptr());
            ffrt_mutex_unlock(self.mutex.as_ptr());
        }
    }

    pub fn is_woken(&self) -> bool {
        unsafe {
            ffrt_mutex_lock(self.mutex.as_ptr());
            let woken = *self.woken.get();
            ffrt_mutex_unlock(self.mutex.as_ptr());
            woken
        }
    }

    pub fn reset_woken(&self) {
        unsafe {
            ffrt_mutex_lock(self.mutex.as_ptr());
            *self.woken.get() = false;
            ffrt_mutex_unlock(self.mutex.as_ptr());
        }
    }

    pub fn wait_timeout(&self, duration: std::time::Duration) {
        unsafe {
            ffrt_mutex_lock(self.mutex.as_ptr());

            // 计算绝对超时时间点
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("SystemTime before UNIX EPOCH");

            let timeout = now + duration;
            let timeout_secs = timeout.as_secs() as _;
            let timeout_nsecs = timeout.subsec_nanos() as _;

            let ts = timespec {
                tv_sec: timeout_secs,
                tv_nsec: timeout_nsecs,
            };

            ffrt_cond_timedwait(
                self.cond.as_ptr(),
                self.mutex.as_ptr(),
                &ts as *const timespec,
            );

            ffrt_mutex_unlock(self.mutex.as_ptr());
        }
    }

    pub fn wait(&self) {
        unsafe {
            ffrt_mutex_lock(self.mutex.as_ptr());
            ffrt_cond_wait(self.cond.as_ptr(), self.mutex.as_ptr());
            ffrt_mutex_unlock(self.mutex.as_ptr());
        }
    }
}

impl Drop for WakerState {
    fn drop(&mut self) {
        unsafe {
            ffrt_cond_destroy(self.cond.as_ptr());
            ffrt_mutex_destroy(self.mutex.as_ptr());
            let _ = Box::from_raw(self.mutex.as_ptr());
            let _ = Box::from_raw(self.cond.as_ptr());
        }
    }
}

unsafe impl Send for WakerState {}
unsafe impl Sync for WakerState {}

/// 创建一个基于 WakerState 的 waker
pub(crate) fn create_waker(state: Arc<WakerState>) -> Waker {
    unsafe fn clone_waker(data: *const ()) -> RawWaker {
        // SAFETY: data 由 create_waker 创建，保证是有效的 WakerState Arc 指针
        let state = unsafe { Arc::from_raw(data as *const WakerState) };
        let cloned = state.clone();
        std::mem::forget(state);
        RawWaker::new(Arc::into_raw(cloned) as *const (), &WAKER_VTABLE)
    }

    unsafe fn wake(data: *const ()) {
        // SAFETY: data 由 create_waker 创建，保证是有效的 WakerState Arc 指针
        let state = unsafe { Arc::from_raw(data as *const WakerState) };
        state.wake();
        // state 会在这里被 drop，这是正确的
    }

    unsafe fn wake_by_ref(data: *const ()) {
        // SAFETY: data 由 create_waker 创建，保证是有效的 WakerState Arc 指针
        let state = unsafe { Arc::from_raw(data as *const WakerState) };
        state.wake();
        std::mem::forget(state); // 不要 drop，因为只是引用
    }

    unsafe fn drop_waker(data: *const ()) {
        // SAFETY: data 由 create_waker 创建，保证是有效的 WakerState Arc 指针
        let _ = unsafe { Arc::from_raw(data as *const WakerState) };
        // Arc 会在这里被 drop
    }

    static WAKER_VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);

    let raw_waker = RawWaker::new(Arc::into_raw(state) as *const (), &WAKER_VTABLE);

    unsafe { Waker::from_raw(raw_waker) }
}

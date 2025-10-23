//! FFRT FFI绑定

use libc::{c_int, c_void, c_char};

// QoS级别
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum FfrtQos {
    Background = 0,
    Utility = 1,
    Default = 2,
    UserInitiated = 3,
    UserInteractive = 4,
}

// 任务属性
#[repr(C)]
pub struct FfrtTaskAttr {
    _private: [u8; 128],
}

// 互斥锁
#[repr(C)]
pub struct FfrtMutex {
    _private: [u8; 40],
}

// 条件变量
#[repr(C)]
pub struct FfrtCond {
    _private: [u8; 48],
}

// 读写锁
#[repr(C)]
pub struct FfrtRwlock {
    _private: [u8; 56],
}

// 任务句柄
pub type FfrtTask = *mut c_void;


#[link(name="ffrt.z")]
unsafe extern "C" {
    // 任务管理
    pub fn ffrt_submit_base(
        func: extern "C" fn(*mut c_void),
        after_func: extern "C" fn(*mut c_void),
        arg: *mut c_void,
        in_deps: *const c_void,
        out_deps: *const c_void,
        attr: *const FfrtTaskAttr,
    ) -> FfrtTask;
    
    pub fn ffrt_task_attr_init(attr: *mut FfrtTaskAttr);
    pub fn ffrt_task_attr_destroy(attr: *mut FfrtTaskAttr);
    pub fn ffrt_task_attr_set_qos(attr: *mut FfrtTaskAttr, qos: FfrtQos);
    pub fn ffrt_task_attr_set_name(attr: *mut FfrtTaskAttr, name: *const c_char);
    
    pub fn ffrt_wait();
    pub fn ffrt_wait_deps(deps: *const c_void);
    pub fn ffrt_yield();
    pub fn ffrt_usleep(usec: u64);
    
    // 互斥锁
    pub fn ffrt_mutex_init(mutex: *mut FfrtMutex, attr: *const c_void) -> c_int;
    pub fn ffrt_mutex_destroy(mutex: *mut FfrtMutex) -> c_int;
    pub fn ffrt_mutex_lock(mutex: *mut FfrtMutex) -> c_int;
    pub fn ffrt_mutex_unlock(mutex: *mut FfrtMutex) -> c_int;
    pub fn ffrt_mutex_trylock(mutex: *mut FfrtMutex) -> c_int;
    
    // 条件变量
    pub fn ffrt_cond_init(cond: *mut FfrtCond, attr: *const c_void) -> c_int;
    pub fn ffrt_cond_destroy(cond: *mut FfrtCond) -> c_int;
    pub fn ffrt_cond_signal(cond: *mut FfrtCond) -> c_int;
    pub fn ffrt_cond_broadcast(cond: *mut FfrtCond) -> c_int;
    pub fn ffrt_cond_wait(cond: *mut FfrtCond, mutex: *mut FfrtMutex) -> c_int;
    pub fn ffrt_cond_timewait(cond: *mut FfrtCond, mutex: *mut FfrtMutex, timestamp: u64) -> c_int;
    
    // 读写锁
    pub fn ffrt_rwlock_init(rwlock: *mut FfrtRwlock, attr: *const c_void) -> c_int;
    pub fn ffrt_rwlock_destroy(rwlock: *mut FfrtRwlock) -> c_int;
    pub fn ffrt_rwlock_rdlock(rwlock: *mut FfrtRwlock) -> c_int;
    pub fn ffrt_rwlock_wrlock(rwlock: *mut FfrtRwlock) -> c_int;
    pub fn ffrt_rwlock_unlock(rwlock: *mut FfrtRwlock) -> c_int;
    pub fn ffrt_rwlock_tryrdlock(rwlock: *mut FfrtRwlock) -> c_int;
    pub fn ffrt_rwlock_trywrlock(rwlock: *mut FfrtRwlock) -> c_int;
}
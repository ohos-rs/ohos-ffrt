use ohos_ffrt_sys::{
    ffrt_mutex_type, ffrt_mutex_type_ffrt_mutex_normal, ffrt_mutex_type_ffrt_mutex_recursive,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LockMode {
    Normal,
    Recursive,
}

impl From<ffrt_mutex_type> for LockMode {
    fn from(value: ffrt_mutex_type) -> Self {
        match value {
            ffrt_mutex_type_ffrt_mutex_normal => LockMode::Normal,
            ffrt_mutex_type_ffrt_mutex_recursive => LockMode::Recursive,
            _ => panic!("Invalid mutex type: {}", value),
        }
    }
}

impl From<LockMode> for ffrt_mutex_type {
    fn from(value: LockMode) -> Self {
        match value {
            LockMode::Normal => ffrt_mutex_type_ffrt_mutex_normal,
            LockMode::Recursive => ffrt_mutex_type_ffrt_mutex_recursive,
        }
    }
}

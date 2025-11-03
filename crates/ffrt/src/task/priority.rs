#![allow(warnings)]

use ohos_ffrt_sys::{
    ffrt_queue_priority_t, ffrt_queue_priority_t_ffrt_queue_priority_high,
    ffrt_queue_priority_t_ffrt_queue_priority_idle,
    ffrt_queue_priority_t_ffrt_queue_priority_immediate,
    ffrt_queue_priority_t_ffrt_queue_priority_low,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    Immediate,
    High,
    Low,
    Idle,
}

impl From<TaskPriority> for ffrt_queue_priority_t {
    fn from(priority: TaskPriority) -> Self {
        match priority {
            TaskPriority::Immediate => ffrt_queue_priority_t_ffrt_queue_priority_immediate,
            TaskPriority::High => ffrt_queue_priority_t_ffrt_queue_priority_high,
            TaskPriority::Low => ffrt_queue_priority_t_ffrt_queue_priority_low,
            TaskPriority::Idle => ffrt_queue_priority_t_ffrt_queue_priority_idle,
        }
    }
}

impl From<ffrt_queue_priority_t> for TaskPriority {
    fn from(priority: ffrt_queue_priority_t) -> Self {
        match priority {
            ffrt_queue_priority_t_ffrt_queue_priority_immediate => TaskPriority::Immediate,
            ffrt_queue_priority_t_ffrt_queue_priority_high => TaskPriority::High,
            ffrt_queue_priority_t_ffrt_queue_priority_low => TaskPriority::Low,
            ffrt_queue_priority_t_ffrt_queue_priority_idle => TaskPriority::Idle,
            _ => unreachable!(),
        }
    }
}

use ffrt::{Task, TaskAttr, TaskPriority};
use napi_derive_ohos::napi;

#[napi]
pub fn run_ffrt() {
    let task = Task::new(Default::default());

    task.submit(|| {
        ohos_hilog_binding::hilog_info!("Hello, FFRT!");
    });
}

#[napi]
pub fn run_ffrt_with_attr() {
    let attr = TaskAttr::new();
    attr.set_priority(TaskPriority::High);

    let task = Task::new(attr);

    task.submit(|| {
        ohos_hilog_binding::hilog_info!("Hello, FFRT!");
    });
}

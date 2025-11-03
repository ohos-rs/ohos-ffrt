use napi_derive_ohos::napi;
use ohos_ffrt::Task;

#[napi]
pub fn run_ffrt() -> () {
    let task = Task::new(Default::default());

    task.submit(|| {
        ohos_hilog_binding::hilog_info!("Hello, FFRT!");
    });
}

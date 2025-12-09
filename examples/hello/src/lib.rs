use std::time::Duration;
use std::time::Instant;

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::FnArgs;
use napi_ohos::bindgen_prelude::Function;
use napi_ohos::bindgen_prelude::PromiseRaw;
use napi_ohos::*;
use ohos_ext::*;

#[napi(ts_return_type = "Promise<void>")]
pub fn example_a<'env>(
    env: &'env Env,
    callback: Function<FnArgs<(u32, u32, u32)>, String>,
) -> napi_ohos::Result<PromiseRaw<'env, ()>> {
    let tsfn = callback.build_threadsafe_function().build()?;
    env.spawn_local(async move {
        let msg = tsfn.call_local((1, 2, 3).into()).await?;
        ohos_hilog_binding::hilog_info!("msg: {}", msg);
        Ok(())
    })
}

#[napi]
pub fn example_b<'env>(env: &'env Env, hello: String) -> napi_ohos::Result<PromiseRaw<'env, ()>> {
    env.spawn_local(async move {
        ohos_hilog_binding::hilog_info!("Hello, {}!", hello);
        Ok(())
    })
}

#[ffrt]
pub async fn example_c(hello: String) -> () {
    ohos_hilog_binding::hilog_info!("Hello, {}!", hello);
}

#[ffrt]
pub async fn example_d(hello: String) -> napi_ohos::Result<()> {
    ohos_hilog_binding::hilog_info!("Hello, {}!", hello);
    Ok(())
}

#[ffrt]
pub async fn example_e() -> napi_ohos::Result<String> {
    Ok("Hello, World!".to_string())
}

#[ffrt]
pub async fn example_f() -> String {
    String::from("hello")
}

// Test case: This should compile fine with napi_ohos::Result
#[ffrt]
pub async fn example_g(value: u32) -> napi_ohos::Result<u32> {
    Ok(value * 2)
}

// Test case: Empty function returning ()
#[ffrt]
pub async fn example_h() {
    ohos_hilog_binding::hilog_info!("Example H called");
}

#[ffrt]
pub async fn sleep_example() {
    ohos_hilog_binding::hilog_info!(format!("Start time: {:?}", Instant::now()));
    ohos_ext::timer::r#async::sleep(Duration::from_secs(1)).await;
    ohos_hilog_binding::hilog_info!(format!("End time: {:?}", Instant::now()));
}

#[ffrt(qos = "Inherit")]
pub async fn example_i() {
    ohos_hilog_binding::hilog_info!("Example I called");
}

#[ffrt(priority = "High")]
pub async fn example_j() {
    ohos_hilog_binding::hilog_info!("Example J called");
}

#[ffrt(name = "example_k")]
pub async fn example_k() {
    ohos_hilog_binding::hilog_info!("Example K called");
}

#[ffrt(delay = 1000)]
pub async fn example_l() {
    ohos_hilog_binding::hilog_info!("Example L called");
}

#[ffrt(stack_size = 1024)]
pub async fn example_m() {
    ohos_hilog_binding::hilog_info!("Example M called");
}

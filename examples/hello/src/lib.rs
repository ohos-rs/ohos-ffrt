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

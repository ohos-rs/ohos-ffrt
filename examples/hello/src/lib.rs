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
        let msg = tsfn.call_async((1, 2, 3).into()).await?;
        ohos_hilog_binding::hilog_info!("msg: {}", msg);
        Ok(())
    })
}

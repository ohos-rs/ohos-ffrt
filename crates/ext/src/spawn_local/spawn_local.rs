use futures::Future;
use napi_ohos::Env;
use napi_ohos::JsObject;
use napi_ohos::NapiValue;

use crate::runtime;
use crate::utils::UtilsExt;

pub fn spawn_local<Fut>(
  env: &Env,
  future: Fut,
) -> napi_ohos::Result<()>
where
  Fut: Future<Output = napi_ohos::Result<()>> + 'static,
{
  runtime::spawn_local_fut(*env, async move {
    if let Err(error) = future.await {
      eprintln!("Uncaught Napi Error: {}", error);
    };
  })?;

  Ok(())
}

pub fn spawn_local_promise<R, Fut>(
  env: &Env,
  future: Fut,
) -> napi_ohos::Result<JsObject>
where
  R: NapiValue + 'static,
  Fut: Future<Output = napi_ohos::Result<R>> + 'static,
{
  env.create_promise(Box::new(move |env, resolve_func, reject_func| {
    runtime::spawn_local_fut(env, async move {
      match future.await {
        Ok(result) => resolve_func(result),
        Err(error) => reject_func(error),
      };
    })
  }))
}

pub fn spawn_local_promise2<R, F, Fut>(
  env: &Env,
  future: Fut,
) -> napi_ohos::Result<JsObject>
where
  R: NapiValue + 'static,
  Fut: Future<Output = napi_ohos::Result<R>> + 'static,
{
  env.create_promise(Box::new(move |env, resolve_func, reject_func| {
    runtime::spawn_local_fut(env, async move {
      match future.await {
        Ok(result) => resolve_func(result),
        Err(error) => reject_func(error),
      };
    })
  }))
}

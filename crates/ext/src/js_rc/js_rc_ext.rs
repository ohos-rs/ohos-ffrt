use napi_ohos::Env;
use napi_ohos::NapiRaw;
use napi_ohos::NapiValue;

use super::JsRc;

pub trait JsRcExt<T: NapiRaw> {
  /// Wraps the JavaScript value in a reference counted container
  /// that prevents Nodejs's GC from dropping it
  fn into_rc(
    self,
    env: &Env,
  ) -> napi_ohos::Result<JsRc<T>>;
}

impl<T: NapiValue> JsRcExt<T> for T {
  fn into_rc(
    self,
    env: &Env,
  ) -> napi_ohos::Result<JsRc<T>> {
    JsRc::new(env, self)
  }
}

use napi_ohos::bindgen_prelude::ToNapiValue;
use napi_ohos::Env;
use napi_ohos::JsObject;
use napi_ohos::NapiValue;

use super::console_log;
use super::create_promise;
use super::spawn_thread;
use super::PromiseExecutor;

pub trait UtilsExt {
  /// Runs console.log() in the JavaScript context.
  /// useful for debugging [`NapiValue`] types
  fn console_log<V: NapiValue>(
    &self,
    args: &[V],
  ) -> napi_ohos::Result<()>;

  fn create_promise<Res>(
    &self,
    executor: PromiseExecutor<Res>,
  ) -> napi_ohos::Result<JsObject>
  where
    Res: NapiValue + 'static;

  fn spawn_thread<ThreadFunc, NapiFunc, NapiRet>(
    &self,
    func: ThreadFunc,
  ) -> napi_ohos::Result<JsObject>
  where
    ThreadFunc: FnOnce() -> napi_ohos::Result<NapiFunc> + Send + 'static,
    NapiFunc: FnOnce(Env) -> napi_ohos::Result<NapiRet> + Send + 'static,
    NapiRet: ToNapiValue;
}

impl UtilsExt for Env {
  fn console_log<V: NapiValue>(
    &self,
    args: &[V],
  ) -> napi_ohos::Result<()> {
    console_log(self, args)
  }

  fn create_promise<Res>(
    &self,
    executor: PromiseExecutor<Res>,
  ) -> napi_ohos::Result<JsObject>
  where
    Res: NapiValue + 'static,
  {
    create_promise(self, executor)
  }

  fn spawn_thread<ThreadFunc, NapiFunc, NapiRet>(
    &self,
    func: ThreadFunc,
  ) -> napi_ohos::Result<JsObject>
  where
    ThreadFunc: FnOnce() -> napi_ohos::Result<NapiFunc> + Send + 'static,
    NapiFunc: FnOnce(Env) -> napi_ohos::Result<NapiRet> + Send + 'static,
    NapiRet: ToNapiValue,
  {
    spawn_thread(self, func)
  }
}

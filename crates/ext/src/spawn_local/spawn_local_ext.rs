use futures::Future;
use napi_ohos::{
    Env, Result,
    bindgen_prelude::{PromiseRaw, ToNapiValue},
};

use crate::spawn_local;

pub trait SpawnLocalExt {
    fn spawn_local<
        T: 'static + Send + ToNapiValue,
        F: 'static + Send + Future<Output = Result<T>>,
    >(
        &self,
        fut: F,
    ) -> Result<PromiseRaw<'_, T>>;

    fn spawn_local_with_callback<
        'env,
        T: 'static + Send,
        V: ToNapiValue,
        F: 'static + Send + Future<Output = Result<T>>,
        R: 'static + FnOnce(&'env Env, T) -> Result<V>,
    >(
        &'env self,
        fut: F,
        callback: R,
    ) -> Result<PromiseRaw<'env, V>>;
}

impl SpawnLocalExt for Env {
    fn spawn_local<
        T: 'static + Send + ToNapiValue,
        F: 'static + Send + Future<Output = Result<T>>,
    >(
        &self,
        fut: F,
    ) -> Result<PromiseRaw<'_, T>> {
        let promise = spawn_local(self.raw(), fut, |env, val| unsafe {
            ToNapiValue::to_napi_value(env, val)
        })?;

        Ok(PromiseRaw::new(self.raw(), promise))
    }

    fn spawn_local_with_callback<
        'env,
        T: 'static + Send,
        V: ToNapiValue,
        F: 'static + Send + Future<Output = Result<T>>,
        R: 'static + FnOnce(&'env Env, T) -> Result<V>,
    >(
        &'env self,
        fut: F,
        callback: R,
    ) -> Result<PromiseRaw<'env, V>> {
        let promise = spawn_local(self.raw(), fut, move |env, val| unsafe {
            let env = Env::from_raw(env);
            let static_env = core::mem::transmute::<&Env, &'env Env>(&env);
            let val = callback(static_env, val)?;
            ToNapiValue::to_napi_value(env.raw(), val)
        })?;

        Ok(PromiseRaw::new(self.raw(), promise))
    }
}

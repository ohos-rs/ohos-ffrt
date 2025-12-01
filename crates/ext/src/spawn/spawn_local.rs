use futures::Future;
use napi_ohos::{Env, Error, JsValue, Result, SendableResolver, Status, Unknown, sys};
use ohos_ffrt::Runtime;

pub fn spawn_local<
    Data: 'static + Send,
    Fut: 'static + Send + Future<Output = std::result::Result<Data, impl Into<Error>>>,
    Resolver: 'static + FnOnce(sys::napi_env, Data) -> Result<sys::napi_value>,
>(
    env: sys::napi_env,
    fut: Fut,
    resolver: Resolver,
) -> Result<sys::napi_value> {
    let env = Env::from(env);

    let (deferred, promise) = env.create_deferred::<Unknown<'_>, _>()?;

    let deferred_for_panic = deferred.clone();
    let sendable_resolver = SendableResolver::new(resolver);

    let inner = async move {
        match fut.await {
            Ok(v) => deferred.resolve(move |env| {
                sendable_resolver
                    .resolve(env.raw(), v)
                    .map(|v| unsafe { Unknown::from_raw_unchecked(env.raw(), v) })
            }),
            Err(e) => deferred.reject(e.into()),
        }
    };

    let runtime = Runtime::new();

    let jh = runtime.spawn(inner);

    runtime.spawn(async move {
        if let Err(err) = jh.await {
            deferred_for_panic.reject(Error::new(Status::GenericFailure, err.to_string()));
        }
    });

    Ok(promise.raw())
}

# ohos-ffrt

![Crates.io Version](https://img.shields.io/crates/v/ohos-ffrt) ![Platform](https://img.shields.io/badge/platform-arm64/arm/x86__64-blue) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

This project provide a ffrt-binding and napi-ext for ffrt.

**Note: Don't use it to replace `tokio` directly, it only works for some simple scenarios now.**

## Install

```bash
cargo add ohos-ffrt
# or
cargo add ohos-ext
```

## Basic Usage

We can use it as another thread.

```rs
use napi_derive_ohos::napi;
use ohos_ffrt::Task;

#[napi]
pub fn run_ffrt() -> () {
    let task = Task::new(Default::default());

    task.submit(|| {
        ohos_hilog_binding::hilog_info!("Hello, FFRT!");
    });
}
```

## Napi-Ext

We can also define async function for napi with `ohos-ext`.

### Execute with env

```rs
use ohos_ext::*;

#[napi(ts_return_type = "Promise<void>")]
pub fn example_a<'env>(
    env: &'env Env,
    callback: Function<FnArgs<(u32, u32, u32)>, String>,
) -> napi_ohos::Result<PromiseRaw<'env, ()>> {
    let tsfn = callback.build_threadsafe_function().build()?;

    // Use new method
    env.spawn_local(async move {
        let msg = tsfn.call_local((1, 2, 3).into()).await?;
        ohos_hilog_binding::hilog_info!("msg: {}", msg);
        Ok(())
    })
}
```

### ffrt macro

```rs
use ohos_ext::*;

#[ffrt]
pub async fn example_e() -> napi_ohos::Result<String> {
    Ok("Hello, World!".to_string())
}
```

## What is FFRT and Why we need it?

You can see it as a built-in ThreadPool or async runtime. See detail with [ffrt-kit](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/ffrt-kit).

We typically rely on tokio as the runtime for asynchronous tasks, but it adds extra overhead in terms of package size and startup thread load. Switching to `ffrt` helps address these challenges.

## License

[MIT](./LICENSE)

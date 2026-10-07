# ffrt

![Crates.io Version](https://img.shields.io/crates/v/ffrt) ![Platform](https://img.shields.io/badge/platform-arm64/arm/x86__64-blue) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

This project provides FFRT bindings and a napi extension for FFRT.

The `ffrt` crate is an OHOS execution backend for Tokio's stable production
surface. An OHOS target can depend on the package as `tokio` and retain Tokio
imports; see [the compatibility contract](./TOKIO_COMPATIBILITY.md) for the
supported surface and deliberate FFRT scheduler/test-tool differences.

The I/O layer uses an FFRT loop readiness reactor. It includes `AsyncFd`,
non-blocking TCP/UDP types, `AsyncRead`/`AsyncWrite` extensions, `copy`,
`split`, `BufReader`, and `BufWriter`. Synchronization includes owned mutex,
rwlock, and semaphore guards; task-local values, Unix signals, and fair
`select!` with up to 64 branches are also available. Unix modules are compiled
only for `target_env = "ohos"`. Enabling the `tracing` feature emits
Tokio-schema task spawn/poll and waker lifecycle diagnostics and exposes the
named `tokio::task::Builder` API for tracing ecosystem integration.

## Install

```bash
cargo add ffrt
# raw sys bindings
cargo add ffrt-sys
# or the napi extension
cargo add ohos-ext
```

## Basic Usage

We can use it as another thread.

```rs
use napi_derive_ohos::napi;
use ffrt::Task;

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

## Regression tests

On Linux, install Rust, CMake, Git, and a C/C++ compiler, then run:

```bash
scripts/test-linux.sh
```

This builds pinned upstream FFRT and securec revisions, runs unit, integration,
macro, and documentation tests, checks the no-default-features configuration,
and executes `qemu_smoke` and the `tokio` package-alias example. The PR workflow
runs the same script. Linux testing supplements the OHOS compatibility contract.
`FFRT_TEST_SOURCE` and `FFRT_TEST_SECUREC` can point to existing source checkouts
for offline runs; `FFRT_TEST_CACHE` selects the build cache directory.

To execute tests on an already connected arm64 OpenHarmony device or QEMU guest:

```bash
export HDC=/path/to/toolchains/hdc
export HDC_TARGET=127.0.0.1:5555
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER=/path/to/native/llvm/bin/aarch64-unknown-linux-ohos-clang
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_RUNNER="python3 $(pwd)/scripts/ohos-runner.py"
cargo test --locked -p ffrt --all-features --target aarch64-unknown-linux-ohos --lib --tests
cargo run --locked -p ffrt --all-features --target aarch64-unknown-linux-ohos --example qemu_smoke
cargo run --locked -p tokio-compat --target aarch64-unknown-linux-ohos
```

The runner uploads each binary to a unique temporary path, propagates its exit
status, and removes it afterward. `OHOS_HDC_SERVER_PORT` selects an alternate
HDC server. Static checks cover arm64, armv7, and x86_64 OHOS targets.

`TaskAttr` owns its native allocation and implements deep `Clone`; it is no
longer `Copy`. `get_name()` returns an owned `String`, which remains valid after
renaming or dropping the attribute.

## What is FFRT and Why we need it?

You can see it as a built-in ThreadPool or async runtime. See detail with [ffrt-kit](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/ffrt-kit).

We typically rely on tokio as the runtime for asynchronous tasks, but it adds extra overhead in terms of package size and startup thread load. Switching to `ffrt` helps address these challenges.

## License

[MIT](./LICENSE)

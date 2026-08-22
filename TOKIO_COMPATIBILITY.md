# Tokio compatibility on OpenHarmony

`ffrt` is an OpenHarmony-only execution backend for applications that use
Tokio's stable production APIs. An OHOS target can replace the package without
renaming Rust imports:

```toml
[target.'cfg(target_env = "ohos")'.dependencies]
tokio = { package = "ffrt", path = "crates/ffrt", features = ["full"] }
```

The compatibility contract is intentionally scoped to OHOS. Unix sockets,
Unix signals, pipes, raw file descriptors, and Unix process extensions are
compiled only for `target_env = "ohos"`; no non-OHOS runtime backend is
promised.

## Supported production surface

- FFRT task spawning, blocking work, cancellation, task IDs, `JoinHandle`,
  `JoinSet`, `LocalSet`, cooperative yield, and task-local values.
- FFRT-loop timers and readiness I/O, including `AsyncFd` backed by
  `ffrt_loop_epoll_ctl`.
- Non-blocking TCP, UDP, Unix-domain sockets, anonymous/FIFO pipes, socket
  readiness, split halves, and socket options exposed by OHOS.
- Tokio-style async I/O traits, integer helpers, `read_exact`, `read_to_end`,
  `copy`, bidirectional copy, `split`, `join`, duplex/simplex streams,
  `BufReader`, `BufWriter`, and standard streams.
- Async filesystem and process APIs, with blocking filesystem/process work
  moved off the polling path.
- Mutex, RwLock, semaphore, barrier, notify, once-cell, mpsc, oneshot,
  broadcast, and watch primitives, including owned and mapped guards.
- `join!`, `try_join!`, `select!` (fair randomized or `biased;`, up to 64
  branches), `pin!`, `task_local!`, `#[tokio::main]`, and `#[tokio::test]`.

## Deliberate FFRT differences

- FFRT owns one process-wide worker pool. Builder thread-count, thread-name,
  queue-interval, and current-thread settings are accepted for source
  compatibility but cannot reconfigure or isolate that system pool.
- Runtime metrics expose stable counts that FFRT can determine. FFRT does not
  expose Tokio's scheduler internals, so global queue depth is reported as
  zero.
- Tokio's `tokio_unstable` APIs, runtime tracing/task dumps, and scheduler
  instrumentation hooks are outside the replacement contract.
- `test-util` virtual time (`pause`, `resume`, `advance`, and `start_paused`)
  is not emulated. Timers always use the OHOS FFRT loop clock.

The `examples/tokio-compat` package is compiled with the dependency named
`tokio`, and `qemu_smoke` validates the runtime path on arm64, armv7, and
x86_64 OpenHarmony images.

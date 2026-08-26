# Tokio compatibility on OpenHarmony

`ffrt` is an OpenHarmony-only execution backend for applications that use
Tokio's stable production APIs. An OHOS target can replace the package without
renaming Rust imports:

```toml
[target.'cfg(target_env = "ohos")'.dependencies]
tokio = { package = "ffrt", path = "crates/ffrt", features = ["full"] }
```

Applications that consume Tokio runtime diagnostics can additionally enable
`tracing`:

```toml
[target.'cfg(target_env = "ohos")'.dependencies]
tokio = { package = "ffrt", path = "crates/ffrt", features = ["full", "tracing"] }
```

The compatibility contract is intentionally scoped to OHOS. Unix sockets,
Unix signals, pipes, raw file descriptors, and Unix process extensions are
compiled only for `target_env = "ohos"`; no non-OHOS runtime backend is
promised.

## Supported production surface

- FFRT task spawning, blocking work, cancellation, task IDs, `JoinHandle`,
  `JoinSet`, `LocalSet`, cooperative yield, task-local values, and the tracing
  task `Builder`.
- With `tracing`, Tokio-compatible `runtime.spawn` spans are emitted for async,
  local, and blocking tasks. Every future poll enters its task span, and waker
  clone, wake, wake-by-ref, and drop operations emit `tokio::task::waker`
  events carrying `task.id`. Task names, future/function sizes, and spawn source
  locations use Tokio's field names so standard `tracing` layers can consume
  them.
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
- Tokio's `tokio_unstable` scheduler-internal APIs, resource-state protocol,
  runtime task dumps, and scheduler instrumentation hooks are outside the
  replacement contract. Stable `tracing` subscriber integration and Tokio's
  task lifecycle schema are supported as described above.
- `test-util` virtual time (`pause`, `resume`, `advance`, and `start_paused`)
  is not emulated. Timers always use the OHOS FFRT loop clock.

The `examples/tokio-compat` package is compiled with the dependency named
`tokio`; it installs a `tracing-subscriber` layer and validates task lifecycle
events. `qemu_smoke` validates the runtime path on arm64, armv7, and x86_64
OpenHarmony images.

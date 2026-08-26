pub mod mpsc;
pub mod oneshot;

#[cfg(target_env = "ohos")]
pub mod unix;

/// Completes when the process receives Ctrl-C (`SIGINT`).
#[cfg(target_env = "ohos")]
pub async fn ctrl_c() -> std::io::Result<()> {
    let mut signal = unix::signal(unix::SignalKind::interrupt())?;
    signal
        .recv()
        .await
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "signal stream closed"))
}

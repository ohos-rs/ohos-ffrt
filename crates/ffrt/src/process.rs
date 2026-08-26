//! OpenHarmony child-process support with reactor-backed standard I/O.

use std::ffi::{OsStr, OsString};
use std::io::{self, IoSlice, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::task::{Context, Poll};

pub use std::process::Output;

use crate::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use crate::lock::Mutex;
use crate::reactor::{AsyncFd, Interest};

fn runtime_error(error: crate::JoinError) -> io::Error {
    io::Error::other(error.to_string())
}

async fn run_blocking<F, R>(func: F) -> io::Result<R>
where
    F: FnOnce() -> io::Result<R> + Send + 'static,
    R: Send + 'static,
{
    crate::spawn_blocking(func).await.map_err(runtime_error)?
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let status = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if status < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, status | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A reusable child-process command builder.
#[derive(Debug)]
pub struct Command {
    inner: std::process::Command,
    kill_on_drop: bool,
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            inner: std::process::Command::new(program),
            kill_on_drop: false,
        }
    }

    pub fn from_std(command: std::process::Command) -> Self {
        Self {
            inner: command,
            kill_on_drop: false,
        }
    }

    pub fn as_std(&self) -> &std::process::Command {
        &self.inner
    }

    pub fn as_std_mut(&mut self) -> &mut std::process::Command {
        &mut self.inner
    }

    pub fn into_std(self) -> std::process::Command {
        self.inner
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.env(key, value);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.inner.env_remove(key);
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.inner.env_clear();
        self
    }

    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    pub fn stdin<T: Into<Stdio>>(&mut self, config: T) -> &mut Self {
        self.inner.stdin(config);
        self
    }

    pub fn stdout<T: Into<Stdio>>(&mut self, config: T) -> &mut Self {
        self.inner.stdout(config);
        self
    }

    pub fn stderr<T: Into<Stdio>>(&mut self, config: T) -> &mut Self {
        self.inner.stderr(config);
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.kill_on_drop = kill_on_drop;
        self
    }

    pub fn get_kill_on_drop(&self) -> bool {
        self.kill_on_drop
    }

    pub fn uid(&mut self, id: u32) -> &mut Self {
        self.inner.uid(id);
        self
    }

    pub fn gid(&mut self, id: u32) -> &mut Self {
        self.inner.gid(id);
        self
    }

    pub fn process_group(&mut self, group: i32) -> &mut Self {
        self.inner.process_group(group);
        self
    }

    pub fn arg0<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.inner.arg0(arg);
        self
    }

    /// Installs a callback immediately before `exec` in the child.
    ///
    /// # Safety
    ///
    /// The callback runs after `fork`; it may only call async-signal-safe
    /// operations.
    pub unsafe fn pre_exec<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static,
    {
        unsafe { self.inner.pre_exec(callback) };
        self
    }

    pub fn get_program(&self) -> &OsStr {
        self.inner.get_program()
    }

    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.inner.get_args()
    }

    pub fn get_envs(&self) -> impl Iterator<Item = (&OsStr, Option<&OsStr>)> {
        self.inner.get_envs()
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.inner.get_current_dir()
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        Child::from_std(self.inner.spawn()?, self.kill_on_drop)
    }

    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.wait().await
    }

    pub async fn output(&mut self) -> io::Result<Output> {
        self.inner.stdout(Stdio::piped()).stderr(Stdio::piped());
        self.spawn()?.wait_with_output().await
    }
}

impl From<std::process::Command> for Command {
    fn from(command: std::process::Command) -> Self {
        Self::from_std(command)
    }
}

/// A running child process.
pub struct Child {
    inner: Arc<Mutex<std::process::Child>>,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    kill_on_drop: bool,
}

impl Child {
    fn from_std(mut child: std::process::Child, kill_on_drop: bool) -> io::Result<Self> {
        let stdin = child.stdin.take().map(ChildStdin::from_std).transpose()?;
        let stdout = child.stdout.take().map(ChildStdout::from_std).transpose()?;
        let stderr = child.stderr.take().map(ChildStderr::from_std).transpose()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(child)),
            stdin,
            stdout,
            stderr,
            kill_on_drop,
        })
    }

    pub fn id(&self) -> Option<u32> {
        Some(self.inner.lock().unwrap().id())
    }

    pub fn start_kill(&mut self) -> io::Result<()> {
        self.inner.lock().unwrap().kill()
    }

    pub async fn kill(&mut self) -> io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(drop)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.lock().unwrap().try_wait()
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        // Match Tokio: close stdin before waiting to avoid a child blocked on
        // input that can no longer be supplied by the caller.
        self.stdin.take();
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().wait()).await
    }

    pub async fn wait_with_output(mut self) -> io::Result<Output> {
        self.stdin.take();
        let mut stdout = self.stdout.take();
        let mut stderr = self.stderr.take();

        let read_stdout = async move {
            let mut bytes = Vec::new();
            if let Some(stdout) = &mut stdout {
                stdout.read_to_end(&mut bytes).await?;
            }
            Ok::<_, io::Error>(bytes)
        };
        let read_stderr = async move {
            let mut bytes = Vec::new();
            if let Some(stderr) = &mut stderr {
                stderr.read_to_end(&mut bytes).await?;
            }
            Ok::<_, io::Error>(bytes)
        };
        let wait = self.wait();
        let (status, stdout, stderr) = crate::join!(wait, read_stdout, read_stderr);
        Ok(Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop {
            let _ = self.inner.lock().unwrap().kill();
        }
    }
}

impl std::fmt::Debug for Child {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Child").field("id", &self.id()).finish()
    }
}

fn poll_pipe_read<T: Read + AsRawFd>(
    pipe: &mut AsyncFd<T>,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
) -> Poll<io::Result<()>> {
    loop {
        match pipe.get_mut().read(buf.initialize_unfilled()) {
            Ok(amount) => {
                buf.advance(amount);
                return Poll::Ready(Ok(()));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut guard = match pipe.poll_read_ready(cx) {
                    Poll::Ready(Ok(guard)) => guard,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => return Poll::Pending,
                };
                guard.clear_ready();
            }
            Err(error) => return Poll::Ready(Err(error)),
        }
    }
}

/// Writable handle to a child's standard input.
#[derive(Debug)]
pub struct ChildStdin {
    inner: AsyncFd<std::process::ChildStdin>,
}

impl ChildStdin {
    pub fn from_std(pipe: std::process::ChildStdin) -> io::Result<Self> {
        set_nonblocking(pipe.as_raw_fd())?;
        Ok(Self {
            inner: AsyncFd::with_interest(pipe, Interest::WRITABLE)?,
        })
    }
}

impl AsyncWrite for ChildStdin {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            match this.inner.get_mut().write(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = match this.inner.poll_write_ready(cx) {
                        Poll::Ready(Ok(guard)) => guard,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Pending => return Poll::Pending,
                    };
                    guard.clear_ready();
                }
                result => return Poll::Ready(result),
            }
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            match this.inner.get_mut().write_vectored(bufs) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = match this.inner.poll_write_ready(cx) {
                        Poll::Ready(Ok(guard)) => guard,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Pending => return Poll::Pending,
                    };
                    guard.clear_ready();
                }
                result => return Poll::Ready(result),
            }
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.get_mut().inner.get_mut().flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl AsRawFd for ChildStdin {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for ChildStdin {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

macro_rules! child_reader {
    ($name:ident, $std:ty) => {
        #[derive(Debug)]
        pub struct $name {
            inner: AsyncFd<$std>,
        }

        impl $name {
            pub fn from_std(pipe: $std) -> io::Result<Self> {
                set_nonblocking(pipe.as_raw_fd())?;
                Ok(Self {
                    inner: AsyncFd::with_interest(pipe, Interest::READABLE)?,
                })
            }
        }

        impl AsyncRead for $name {
            fn poll_read(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                poll_pipe_read(&mut self.get_mut().inner, cx, buf)
            }
        }

        impl AsRawFd for $name {
            fn as_raw_fd(&self) -> RawFd {
                self.inner.as_raw_fd()
            }
        }

        impl AsFd for $name {
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.inner.get_ref().as_fd()
            }
        }
    };
}

child_reader!(ChildStdout, std::process::ChildStdout);
child_reader!(ChildStderr, std::process::ChildStderr);

impl From<Command> for std::process::Command {
    fn from(command: Command) -> Self {
        command.into_std()
    }
}

#[allow(dead_code)]
fn _owned_os_string(_: OsString) {}

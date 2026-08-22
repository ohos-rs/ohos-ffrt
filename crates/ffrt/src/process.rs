//! Process helpers executed on FFRT worker tasks.

use std::ffi::OsStr;
use std::io;
use std::process::Output;
use std::sync::Arc;

use crate::lock::Mutex;

async fn run_blocking<F, R>(func: F) -> io::Result<R>
where
    F: FnOnce() -> io::Result<R> + Send + 'static,
    R: Send + 'static,
{
    match crate::spawn_blocking(func).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

/// A tokio-style process command.
pub struct Command {
    inner: Option<std::process::Command>,
}

impl Command {
    /// Creates a command that runs `program`.
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            inner: Some(std::process::Command::new(program)),
        }
    }

    /// Adds an argument.
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.inner
            .as_mut()
            .expect("command already consumed")
            .arg(arg);
        self
    }

    /// Adds multiple arguments.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner
            .as_mut()
            .expect("command already consumed")
            .args(args);
        self
    }

    /// Sets an environment variable.
    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner
            .as_mut()
            .expect("command already consumed")
            .env(key, value);
        self
    }

    /// Sets the working directory.
    pub fn current_dir<P: AsRef<std::path::Path>>(&mut self, dir: P) -> &mut Self {
        self.inner
            .as_mut()
            .expect("command already consumed")
            .current_dir(dir);
        self
    }

    /// Runs the command and waits for its status.
    pub async fn status(&mut self) -> io::Result<std::process::ExitStatus> {
        let mut command = self
            .inner
            .take()
            .ok_or_else(|| io::Error::other("command already consumed"))?;
        run_blocking(move || command.status()).await
    }

    /// Runs the command and captures its output.
    pub async fn output(&mut self) -> io::Result<Output> {
        let mut command = self
            .inner
            .take()
            .ok_or_else(|| io::Error::other("command already consumed"))?;
        run_blocking(move || command.output()).await
    }

    /// Spawns the command without waiting.
    pub async fn spawn(&mut self) -> io::Result<Child> {
        let mut command = self
            .inner
            .take()
            .ok_or_else(|| io::Error::other("command already consumed"))?;
        let child = run_blocking(move || command.spawn()).await?;
        Ok(Child {
            inner: Arc::new(Mutex::new(Some(child))),
        })
    }
}

/// A running child process.
pub struct Child {
    inner: Arc<Mutex<Option<std::process::Child>>>,
}

impl Child {
    /// Returns the process id.
    pub fn id(&self) -> Option<u32> {
        let guard = self.inner.lock().unwrap();
        guard.as_ref().map(|child| child.id())
    }

    /// Kills the child process.
    pub async fn kill(&mut self) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            let child = guard
                .as_mut()
                .ok_or_else(|| io::Error::other("child already reaped"))?;
            child.kill()
        })
        .await
    }

    /// Waits for the child process to exit.
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            let child = guard
                .as_mut()
                .ok_or_else(|| io::Error::other("child already reaped"))?;
            child.wait()
        })
        .await
    }

    /// Waits for the child and captures its remaining output.
    pub async fn wait_with_output(self) -> io::Result<Output> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            let child = guard
                .take()
                .ok_or_else(|| io::Error::other("child already reaped"))?;
            child.wait_with_output()
        })
        .await
    }
}

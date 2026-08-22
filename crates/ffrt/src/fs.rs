//! Filesystem operations executed on FFRT worker tasks.

use std::io;
use std::path::Path;
use std::sync::Arc;

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

/// Reads the entire contents of a file.
pub async fn read<P>(path: P) -> io::Result<Vec<u8>>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::read(path.as_ref())).await
}

/// Reads a file to a string.
pub async fn read_to_string<P>(path: P) -> io::Result<String>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::read_to_string(path.as_ref())).await
}

/// Writes bytes to a file, creating or truncating it.
pub async fn write<P, C>(path: P, contents: C) -> io::Result<()>
where
    P: AsRef<Path> + Send + 'static,
    C: AsRef<[u8]> + Send + 'static,
{
    run_blocking(move || std::fs::write(path.as_ref(), contents.as_ref())).await
}

/// Renames a file or directory.
pub async fn rename<P, Q>(from: P, to: Q) -> io::Result<()>
where
    P: AsRef<Path> + Send + 'static,
    Q: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::rename(from.as_ref(), to.as_ref())).await
}

/// Removes a file.
pub async fn remove_file<P>(path: P) -> io::Result<()>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::remove_file(path.as_ref())).await
}

/// Creates a directory.
pub async fn create_dir<P>(path: P) -> io::Result<()>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::create_dir(path.as_ref())).await
}

/// Creates a directory and all of its parents.
pub async fn create_dir_all<P>(path: P) -> io::Result<()>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::create_dir_all(path.as_ref())).await
}

/// Returns file metadata.
pub async fn metadata<P>(path: P) -> io::Result<std::fs::Metadata>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::metadata(path.as_ref())).await
}

/// Returns the canonicalized path.
pub async fn canonicalize<P>(path: P) -> io::Result<std::path::PathBuf>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::canonicalize(path.as_ref())).await
}

/// Reads all directory entries.
pub async fn read_dir<P>(path: P) -> io::Result<Vec<std::fs::DirEntry>>
where
    P: AsRef<Path> + Send + 'static,
{
    run_blocking(move || {
        let entries = std::fs::read_dir(path.as_ref())?;
        entries.collect::<Result<Vec<_>, _>>()
    })
    .await
}

/// Copies a file.
pub async fn copy<P, Q>(from: P, to: Q) -> io::Result<u64>
where
    P: AsRef<Path> + Send + 'static,
    Q: AsRef<Path> + Send + 'static,
{
    run_blocking(move || std::fs::copy(from.as_ref(), to.as_ref())).await
}

/// An asynchronously accessed file handle.
pub struct File {
    inner: Arc<crate::lock::Mutex<std::fs::File>>,
}

impl File {
    /// Opens a file.
    pub async fn open<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path> + Send + 'static,
    {
        let file = run_blocking(move || std::fs::File::open(path.as_ref())).await?;
        Ok(Self {
            inner: Arc::new(crate::lock::Mutex::new(file)),
        })
    }

    /// Creates or truncates a file.
    pub async fn create<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path> + Send + 'static,
    {
        let file = run_blocking(move || std::fs::File::create(path.as_ref())).await?;
        Ok(Self {
            inner: Arc::new(crate::lock::Mutex::new(file)),
        })
    }

    /// Returns file metadata.
    pub async fn metadata(&self) -> io::Result<std::fs::Metadata> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let guard = inner.lock().unwrap();
            guard.metadata()
        })
        .await
    }

    /// Reads the entire file into a byte vector.
    pub async fn read_to_end(&self) -> io::Result<Vec<u8>> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            use std::io::Read;
            let mut buf = Vec::new();
            guard.read_to_end(&mut buf)?;
            Ok(buf)
        })
        .await
    }

    /// Reads up to `buf.len()` bytes and returns bytes read plus the buffer.
    pub async fn read(&self, buf: Vec<u8>) -> io::Result<(usize, Vec<u8>)> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            use std::io::Read;
            let mut buf = buf;
            let n = guard.read(&mut buf)?;
            Ok((n, buf))
        })
        .await
    }

    /// Writes all bytes.
    pub async fn write_all(&self, data: Vec<u8>) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            use std::io::Write;
            guard.write_all(&data)
        })
        .await
    }

    /// Flushes the file.
    pub async fn sync_all(&self) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let guard = inner.lock().unwrap();
            guard.sync_all()
        })
        .await
    }
}

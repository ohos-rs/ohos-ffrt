//! Filesystem operations executed on FFRT worker tasks.

use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use crate::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};
use crate::runtime::JoinHandle;

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
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::read(path)).await
}

/// Reads a file to a string.
pub async fn read_to_string<P>(path: P) -> io::Result<String>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::read_to_string(path)).await
}

/// Writes bytes to a file, creating or truncating it.
pub async fn write<P, C>(path: P, contents: C) -> io::Result<()>
where
    P: AsRef<Path>,
    C: AsRef<[u8]>,
{
    let path = path.as_ref().to_owned();
    let contents = contents.as_ref().to_owned();
    run_blocking(move || std::fs::write(path, contents)).await
}

/// Renames a file or directory.
pub async fn rename<P, Q>(from: P, to: Q) -> io::Result<()>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    run_blocking(move || std::fs::rename(from, to)).await
}

/// Removes a file.
pub async fn remove_file<P>(path: P) -> io::Result<()>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::remove_file(path)).await
}

/// Creates a directory.
pub async fn create_dir<P>(path: P) -> io::Result<()>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::create_dir(path)).await
}

/// Creates a directory and all of its parents.
pub async fn create_dir_all<P>(path: P) -> io::Result<()>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::create_dir_all(path)).await
}

/// Returns file metadata.
pub async fn metadata<P>(path: P) -> io::Result<std::fs::Metadata>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::metadata(path)).await
}

/// Returns the canonicalized path.
pub async fn canonicalize<P>(path: P) -> io::Result<std::path::PathBuf>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::canonicalize(path)).await
}

/// Returns whether a path exists, propagating errors other than `NotFound`.
pub async fn try_exists<P>(path: P) -> io::Result<bool>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    run_blocking(move || match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    })
    .await
}

/// Reads all directory entries.
pub async fn read_dir<P>(path: P) -> io::Result<ReadDir>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_owned();
    let entries = run_blocking(move || std::fs::read_dir(path)).await?;
    Ok(ReadDir {
        inner: Arc::new(crate::lock::Mutex::new(entries)),
        operation: None,
    })
}

/// Copies a file.
pub async fn copy<P, Q>(from: P, to: Q) -> io::Result<u64>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    run_blocking(move || std::fs::copy(from, to)).await
}

pub async fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::remove_dir(path)).await
}

pub async fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::remove_dir_all(path)).await
}

pub async fn symlink_metadata<P: AsRef<Path>>(path: P) -> io::Result<std::fs::Metadata> {
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::symlink_metadata(path)).await
}

pub async fn read_link<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::read_link(path)).await
}

pub async fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    run_blocking(move || std::fs::hard_link(src, dst)).await
}

pub async fn set_permissions<P: AsRef<Path>>(
    path: P,
    permissions: std::fs::Permissions,
) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    run_blocking(move || std::fs::set_permissions(path, permissions)).await
}

/// Creates a symbolic link on OpenHarmony.
#[cfg(target_env = "ohos")]
pub async fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    run_blocking(move || std::os::unix::fs::symlink(src, dst)).await
}

/// Tokio-compatible alias for creating a file symbolic link on OpenHarmony.
#[cfg(target_env = "ohos")]
pub async fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    symlink(src, dst).await
}

/// Tokio-compatible alias for creating a directory symbolic link on OpenHarmony.
#[cfg(target_env = "ohos")]
pub async fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    symlink(src, dst).await
}

/// Asynchronous directory builder.
#[derive(Debug, Default)]
pub struct DirBuilder {
    recursive: bool,
    #[cfg(target_env = "ohos")]
    mode: Option<u32>,
}

impl DirBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    /// Sets the permissions used when creating directories on OpenHarmony.
    #[cfg(target_env = "ohos")]
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        self.mode = Some(mode);
        self
    }

    pub async fn create<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let path = path.as_ref().to_owned();
        let recursive = self.recursive;
        #[cfg(target_env = "ohos")]
        let mode = self.mode;
        run_blocking(move || {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(recursive);
            #[cfg(target_env = "ohos")]
            if let Some(mode) = mode {
                std::os::unix::fs::DirBuilderExt::mode(&mut builder, mode);
            }
            builder.create(path)
        })
        .await
    }
}

/// Streaming asynchronous directory iterator.
pub struct ReadDir {
    inner: Arc<crate::lock::Mutex<std::fs::ReadDir>>,
    operation: Option<JoinHandle<io::Result<Option<DirEntry>>>>,
}

impl ReadDir {
    pub async fn next_entry(&mut self) -> io::Result<Option<DirEntry>> {
        std::future::poll_fn(|cx| self.poll_next_entry(cx)).await
    }

    pub fn poll_next_entry(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Option<DirEntry>>> {
        if self.operation.is_none() {
            let inner = self.inner.clone();
            self.operation = Some(crate::spawn_blocking(move || {
                inner
                    .lock()
                    .unwrap()
                    .next()
                    .transpose()
                    .map(|entry| entry.map(DirEntry::from_std))
            }));
        }
        let operation = self.operation.as_mut().expect("read-dir operation missing");
        match Pin::new(operation).poll(cx) {
            Poll::Ready(Ok(result)) => {
                self.operation = None;
                Poll::Ready(result)
            }
            Poll::Ready(Err(error)) => {
                self.operation = None;
                Poll::Ready(Err(io::Error::other(error.to_string())))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Directory entry returned by [`ReadDir`].
pub struct DirEntry {
    inner: std::fs::DirEntry,
}

impl DirEntry {
    fn from_std(inner: std::fs::DirEntry) -> Self {
        Self { inner }
    }

    pub fn path(&self) -> PathBuf {
        self.inner.path()
    }

    pub fn file_name(&self) -> std::ffi::OsString {
        self.inner.file_name()
    }

    /// Returns the underlying directory-entry inode on OpenHarmony.
    #[cfg(target_env = "ohos")]
    pub fn ino(&self) -> u64 {
        std::os::unix::fs::DirEntryExt::ino(&self.inner)
    }

    pub async fn metadata(&self) -> io::Result<std::fs::Metadata> {
        let path = self.path();
        run_blocking(move || std::fs::metadata(path)).await
    }

    pub async fn file_type(&self) -> io::Result<std::fs::FileType> {
        let path = self.path();
        run_blocking(move || std::fs::symlink_metadata(path).map(|metadata| metadata.file_type()))
            .await
    }
}

impl std::fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

/// Asynchronous file open-options builder.
#[derive(Clone, Debug)]
pub struct OpenOptions {
    inner: std::fs::OpenOptions,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self {
            inner: std::fs::OpenOptions::new(),
        }
    }

    pub fn read(&mut self, read: bool) -> &mut Self {
        self.inner.read(read);
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Self {
        self.inner.write(write);
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Self {
        self.inner.append(append);
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.inner.truncate(truncate);
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.inner.create(create);
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.inner.create_new(create_new);
        self
    }

    /// Sets the mode bits used when creating a file on OpenHarmony.
    #[cfg(target_env = "ohos")]
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::mode(&mut self.inner, mode);
        self
    }

    /// Adds platform-specific open flags on OpenHarmony.
    #[cfg(target_env = "ohos")]
    pub fn custom_flags(&mut self, flags: i32) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::custom_flags(&mut self.inner, flags);
        self
    }

    pub async fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        let path = path.as_ref().to_owned();
        let options = self.inner.clone();
        run_blocking(move || options.open(path))
            .await
            .map(File::from_inner)
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl From<std::fs::OpenOptions> for OpenOptions {
    fn from(inner: std::fs::OpenOptions) -> Self {
        Self { inner }
    }
}

/// An asynchronously accessed file handle.
pub struct File {
    inner: Arc<crate::lock::Mutex<std::fs::File>>,
    operation: Option<FileOperation>,
    pending_seek: Option<SeekFrom>,
    max_buf_size: usize,
}

enum FileOperation {
    Read(JoinHandle<io::Result<(usize, Vec<u8>)>>),
    Write(JoinHandle<io::Result<usize>>),
    Flush(JoinHandle<io::Result<()>>),
    Seek(JoinHandle<io::Result<u64>>),
}

enum CompletedOperation {
    Read(io::Result<(usize, Vec<u8>)>),
    Write(io::Result<usize>),
    Flush(io::Result<()>),
    Seek(io::Result<u64>),
}

impl File {
    fn from_inner(file: std::fs::File) -> Self {
        Self {
            inner: Arc::new(crate::lock::Mutex::new(file)),
            operation: None,
            pending_seek: None,
            max_buf_size: 2 * 1024 * 1024,
        }
    }

    /// Creates an asynchronous file from a standard file.
    pub fn from_std(file: std::fs::File) -> Self {
        Self::from_inner(file)
    }

    /// Returns a new open-options builder.
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Opens a file.
    pub async fn open<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref().to_owned();
        let file = run_blocking(move || std::fs::File::open(path)).await?;
        Ok(Self::from_inner(file))
    }

    /// Creates or truncates a file.
    pub async fn create<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref().to_owned();
        let file = run_blocking(move || std::fs::File::create(path)).await?;
        Ok(Self::from_inner(file))
    }

    /// Creates a new file and fails if the path already exists.
    pub async fn create_new<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref().to_owned();
        let file = run_blocking(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        })
        .await?;
        Ok(Self::from_inner(file))
    }

    /// Converts this asynchronous file into a standard file after pending work.
    pub async fn into_std(mut self) -> std::fs::File {
        if self.operation.is_some() {
            let operation = std::future::poll_fn(|cx| self.poll_operation(cx))
                .await
                .expect("file operation missing");
            let _ = discard_operation(operation);
        }
        self.try_into_std()
            .unwrap_or_else(|_| panic!("file still has outstanding references"))
    }

    /// Converts immediately when no asynchronous operation is outstanding.
    pub fn try_into_std(self) -> Result<std::fs::File, Self> {
        if self.operation.is_some() || Arc::strong_count(&self.inner) != 1 {
            return Err(self);
        }
        let File {
            inner,
            operation: _,
            pending_seek: _,
            max_buf_size: _,
        } = self;
        match Arc::try_unwrap(inner) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(_) => unreachable!("file Arc uniqueness changed"),
        }
    }

    /// Configures the maximum temporary buffer used by async file operations.
    pub fn set_max_buf_size(&mut self, max_buf_size: usize) {
        self.max_buf_size = max_buf_size;
    }

    /// Returns the maximum temporary buffer size used for file operations.
    pub fn max_buf_size(&self) -> usize {
        self.max_buf_size
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
    pub async fn read_to_end_owned(&self) -> io::Result<Vec<u8>> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            let mut buf = Vec::new();
            guard.read_to_end(&mut buf)?;
            Ok(buf)
        })
        .await
    }

    /// Reads up to `buf.len()` bytes and returns bytes read plus the buffer.
    pub async fn read_owned(&self, buf: Vec<u8>) -> io::Result<(usize, Vec<u8>)> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            let mut buf = buf;
            let n = guard.read(&mut buf)?;
            Ok((n, buf))
        })
        .await
    }

    /// Writes all bytes.
    pub async fn write_all_owned(&self, data: Vec<u8>) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
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

    pub async fn sync_data(&self) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().sync_data()).await
    }

    pub async fn set_len(&self, size: u64) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().set_len(size)).await
    }

    pub async fn set_permissions(&self, permissions: std::fs::Permissions) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().set_permissions(permissions)).await
    }

    pub async fn try_clone(&self) -> io::Result<Self> {
        let inner = self.inner.clone();
        let file = run_blocking(move || inner.lock().unwrap().try_clone()).await?;
        Ok(Self::from_inner(file))
    }

    fn poll_operation(&mut self, cx: &mut Context<'_>) -> Poll<Option<CompletedOperation>> {
        let Some(operation) = self.operation.as_mut() else {
            return Poll::Ready(None);
        };
        let completed = match operation {
            FileOperation::Read(handle) => match Pin::new(handle).poll(cx) {
                Poll::Ready(Ok(result)) => CompletedOperation::Read(result),
                Poll::Ready(Err(error)) => {
                    CompletedOperation::Read(Err(io::Error::other(error.to_string())))
                }
                Poll::Pending => return Poll::Pending,
            },
            FileOperation::Write(handle) => match Pin::new(handle).poll(cx) {
                Poll::Ready(Ok(result)) => CompletedOperation::Write(result),
                Poll::Ready(Err(error)) => {
                    CompletedOperation::Write(Err(io::Error::other(error.to_string())))
                }
                Poll::Pending => return Poll::Pending,
            },
            FileOperation::Flush(handle) => match Pin::new(handle).poll(cx) {
                Poll::Ready(Ok(result)) => CompletedOperation::Flush(result),
                Poll::Ready(Err(error)) => {
                    CompletedOperation::Flush(Err(io::Error::other(error.to_string())))
                }
                Poll::Pending => return Poll::Pending,
            },
            FileOperation::Seek(handle) => match Pin::new(handle).poll(cx) {
                Poll::Ready(Ok(result)) => CompletedOperation::Seek(result),
                Poll::Ready(Err(error)) => {
                    CompletedOperation::Seek(Err(io::Error::other(error.to_string())))
                }
                Poll::Pending => return Poll::Pending,
            },
        };
        self.operation = None;
        Poll::Ready(Some(completed))
    }
}

impl From<std::fs::File> for File {
    fn from(file: std::fs::File) -> Self {
        Self::from_std(file)
    }
}

fn discard_operation(operation: CompletedOperation) -> io::Result<()> {
    match operation {
        CompletedOperation::Read(result) => result.map(drop),
        CompletedOperation::Write(result) => result.map(drop),
        CompletedOperation::Flush(result) => result,
        CompletedOperation::Seek(result) => result.map(drop),
    }
}

impl AsyncRead for File {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.operation.is_none() {
                if buf.remaining() == 0 {
                    return Poll::Ready(Ok(()));
                }
                let inner = this.inner.clone();
                let capacity = buf.remaining().min(this.max_buf_size);
                this.operation = Some(FileOperation::Read(crate::spawn_blocking(move || {
                    let mut bytes = vec![0; capacity];
                    let amount = inner.lock().unwrap().read(&mut bytes)?;
                    Ok((amount, bytes))
                })));
            }
            match ready!(this.poll_operation(cx)).expect("file operation missing") {
                CompletedOperation::Read(Ok((amount, bytes))) => {
                    buf.put_slice(&bytes[..amount]);
                    return Poll::Ready(Ok(()));
                }
                CompletedOperation::Read(Err(error)) => return Poll::Ready(Err(error)),
                other => discard_operation(other)?,
            }
        }
    }
}

impl AsyncWrite for File {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            if this.operation.is_none() {
                if buf.is_empty() {
                    return Poll::Ready(Ok(0));
                }
                let inner = this.inner.clone();
                let bytes = buf[..buf.len().min(this.max_buf_size)].to_vec();
                this.operation = Some(FileOperation::Write(crate::spawn_blocking(move || {
                    inner.lock().unwrap().write(&bytes)
                })));
            }
            match ready!(this.poll_operation(cx)).expect("file operation missing") {
                CompletedOperation::Write(result) => return Poll::Ready(result),
                other => discard_operation(other)?,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.operation.is_none() {
                let inner = this.inner.clone();
                this.operation = Some(FileOperation::Flush(crate::spawn_blocking(move || {
                    inner.lock().unwrap().flush()
                })));
            }
            match ready!(this.poll_operation(cx)).expect("file operation missing") {
                CompletedOperation::Flush(result) => return Poll::Ready(result),
                other => discard_operation(other)?,
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl AsyncSeek for File {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        if this.pending_seek.is_some() {
            return Err(io::Error::other("another file seek is already in progress"));
        }
        this.pending_seek = Some(position);
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let this = self.get_mut();
        loop {
            if this.operation.is_none() {
                let position = this.pending_seek.take().unwrap_or(SeekFrom::Current(0));
                let inner = this.inner.clone();
                this.operation = Some(FileOperation::Seek(crate::spawn_blocking(move || {
                    inner.lock().unwrap().seek(position)
                })));
            }
            match ready!(this.poll_operation(cx)).expect("file operation missing") {
                CompletedOperation::Seek(result) => return Poll::Ready(result),
                other => discard_operation(other)?,
            }
        }
    }
}

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
    buffered_read: std::collections::VecDeque<u8>,
}

enum FileOperation {
    Read(JoinHandle<io::Result<(usize, Vec<u8>)>>),
    Write(JoinHandle<io::Result<usize>>),
    Seek(JoinHandle<io::Result<u64>>),
}

enum CompletedOperation {
    Read(io::Result<(usize, Vec<u8>)>),
    Write(io::Result<usize>),
    Seek(io::Result<u64>),
}

impl File {
    fn from_inner(file: std::fs::File) -> Self {
        Self {
            inner: Arc::new(crate::lock::Mutex::new(file)),
            operation: None,
            pending_seek: None,
            max_buf_size: 2 * 1024 * 1024,
            buffered_read: std::collections::VecDeque::new(),
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
            let _ = self.finish_operation(operation);
        }
        if !self.buffered_read.is_empty() {
            let unread = self.buffered_read.len() as i64;
            let inner = self.inner.clone();
            run_blocking(move || inner.lock().unwrap().seek(SeekFrom::Current(-unread)))
                .await
                .expect("failed to restore buffered file position");
            self.buffered_read.clear();
        }
        self.try_into_std()
            .unwrap_or_else(|_| panic!("file still has outstanding references"))
    }

    /// Converts immediately when no asynchronous operation is outstanding.
    pub fn try_into_std(self) -> Result<std::fs::File, Self> {
        if self.operation.is_some()
            || !self.buffered_read.is_empty()
            || Arc::strong_count(&self.inner) != 1
        {
            return Err(self);
        }
        let File {
            inner,
            operation: _,
            pending_seek: _,
            max_buf_size: _,
            buffered_read: _,
        } = self;
        match Arc::try_unwrap(inner) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(_) => unreachable!("file Arc uniqueness changed"),
        }
    }

    /// Configures the maximum temporary buffer used by async file operations.
    pub fn set_max_buf_size(&mut self, max_buf_size: usize) {
        self.max_buf_size = max_buf_size.max(1);
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
        self.wait_for_operation().await;
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
        self.wait_for_operation().await;
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
        self.wait_for_operation().await;
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            guard.write_all(&data)
        })
        .await
    }

    /// Flushes the file.
    pub async fn sync_all(&self) -> io::Result<()> {
        self.wait_for_operation().await;
        let inner = self.inner.clone();
        run_blocking(move || {
            let guard = inner.lock().unwrap();
            guard.sync_all()
        })
        .await
    }

    pub async fn sync_data(&self) -> io::Result<()> {
        self.wait_for_operation().await;
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().sync_data()).await
    }

    pub async fn set_len(&self, size: u64) -> io::Result<()> {
        self.wait_for_operation().await;
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().set_len(size)).await
    }

    pub async fn set_permissions(&self, permissions: std::fs::Permissions) -> io::Result<()> {
        self.wait_for_operation().await;
        let inner = self.inner.clone();
        run_blocking(move || inner.lock().unwrap().set_permissions(permissions)).await
    }

    pub async fn try_clone(&self) -> io::Result<Self> {
        self.wait_for_operation().await;
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

impl File {
    fn finish_operation(&mut self, operation: CompletedOperation) -> io::Result<()> {
        match operation {
            CompletedOperation::Read(result) => {
                let (amount, bytes) = result?;
                self.buffered_read.extend(&bytes[..amount]);
                Ok(())
            }
            CompletedOperation::Write(result) => result.map(drop),
            CompletedOperation::Seek(result) => result.map(drop),
        }
    }

    async fn wait_for_operation(&self) {
        if let Some(operation) = &self.operation {
            loop {
                let done = match operation {
                    FileOperation::Read(handle) => handle.is_finished(),
                    FileOperation::Write(handle) => handle.is_finished(),
                    FileOperation::Seek(handle) => handle.is_finished(),
                };
                if done {
                    break;
                }
                crate::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }
    }
}

impl AsyncRead for File {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if !this.buffered_read.is_empty() {
                let amount = buf.remaining().min(this.buffered_read.len());
                for byte in this.buffered_read.drain(..amount) {
                    buf.put_slice(&[byte]);
                }
                return Poll::Ready(Ok(()));
            }
            if this.operation.is_none() {
                let inner = this.inner.clone();
                let capacity = buf.remaining().min(this.max_buf_size);
                this.operation = Some(FileOperation::Read(crate::spawn_blocking(move || {
                    let mut bytes = vec![0; capacity];
                    let amount = inner.lock().unwrap().read(&mut bytes)?;
                    Ok((amount, bytes))
                })));
            }
            let operation = ready!(this.poll_operation(cx)).expect("file operation missing");
            let eof = matches!(&operation, CompletedOperation::Read(Ok((0, _))));
            this.finish_operation(operation)?;
            if eof {
                return Poll::Ready(Ok(()));
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
        while let Some(operation) = ready!(this.poll_operation(cx)) {
            this.finish_operation(operation)?;
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let amount = buf.len().min(this.max_buf_size);
        let bytes = buf[..amount].to_vec();
        let inner = this.inner.clone();
        let unread = this.buffered_read.len() as i64;
        this.buffered_read.clear();
        this.operation = Some(FileOperation::Write(crate::spawn_blocking(move || {
            let mut file = inner.lock().unwrap();
            if unread != 0 {
                file.seek(SeekFrom::Current(-unread))?;
            }
            file.write_all(&bytes)?;
            Ok(amount)
        })));
        // Ownership of exactly these bytes has been accepted. Pending never
        // accepts a new buffer; flush waits for the actual blocking write.
        Poll::Ready(Ok(amount))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while let Some(operation) = ready!(this.poll_operation(cx)) {
            this.finish_operation(operation)?;
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl AsyncSeek for File {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        if this.pending_seek.is_some() || matches!(this.operation, Some(FileOperation::Seek(_))) {
            return Err(io::Error::other("another file seek is already in progress"));
        }
        this.pending_seek = Some(position);
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let this = self.get_mut();
        loop {
            if this.operation.is_none() {
                let mut position = this.pending_seek.take().unwrap_or(SeekFrom::Current(0));
                if let SeekFrom::Current(offset) = position {
                    let Some(offset) = offset.checked_sub(this.buffered_read.len() as i64) else {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "seek offset overflow",
                        )));
                    };
                    position = SeekFrom::Current(offset);
                }
                this.buffered_read.clear();
                let inner = this.inner.clone();
                this.operation = Some(FileOperation::Seek(crate::spawn_blocking(move || {
                    inner.lock().unwrap().seek(position)
                })));
            }
            match ready!(this.poll_operation(cx)).expect("file operation missing") {
                CompletedOperation::Seek(result) => return Poll::Ready(result),
                other => this.finish_operation(other)?,
            }
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    use std::task::Waker;

    fn file() -> (std::path::PathBuf, File) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let name = format!(
            "ffrt-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        (path, File::from_std(file))
    }

    #[test]
    fn cancelled_read_accepts_a_smaller_destination_without_losing_bytes() {
        let (path, mut file) = file();
        let (tx, handle) = crate::runtime::local_join_channel();
        file.operation = Some(FileOperation::Read(handle));
        let mut cx = Context::from_waker(Waker::noop());
        let mut original = [0; 8];
        assert!(
            Pin::new(&mut file)
                .poll_read(&mut cx, &mut ReadBuf::new(&mut original))
                .is_pending()
        );
        // Complete the read only after its original borrowing future is gone.
        tx.send(Ok(Ok((8, b"abcdefgh".to_vec())))).unwrap();
        let mut output = Vec::new();
        for _ in 0..4 {
            let mut small = [0; 2];
            let mut buf = ReadBuf::new(&mut small);
            assert!(matches!(
                Pin::new(&mut file).poll_read(&mut cx, &mut buf),
                Poll::Ready(Ok(()))
            ));
            output.extend_from_slice(buf.filled());
        }
        assert_eq!(output, b"abcdefgh");
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pending_write_does_not_accept_the_cancelled_callers_buffer() {
        let (path, mut file) = file();
        let (tx, handle) = crate::runtime::local_join_channel();
        file.operation = Some(FileOperation::Write(handle));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            Pin::new(&mut file)
                .poll_write(&mut cx, b"cancelled-long-buffer")
                .is_pending()
        );
        tx.send(Ok(Ok(100))).unwrap();
        assert!(matches!(
            Pin::new(&mut file).poll_write(&mut cx, b"new"),
            Poll::Ready(Ok(3))
        ));
        crate::Runtime::new()
            .unwrap()
            .block_on(file.flush())
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn buffered_reads_preserve_logical_seek_and_write_positions() {
        crate::Runtime::new().unwrap().block_on(async {
            let (path, mut file) = file();
            file.write_all(b"abcdef").await.unwrap();
            file.flush().await.unwrap();
            file.seek(SeekFrom::Start(0)).await.unwrap();
            // Model a completed six-byte read whose caller consumed only two.
            let mut bytes = vec![0; 6];
            file.inner.lock().unwrap().read_exact(&mut bytes).unwrap();
            file.buffered_read.extend(&bytes[2..]);
            assert_eq!(file.stream_position().await.unwrap(), 2);
            let mut next = [0; 2];
            file.read_exact(&mut next).await.unwrap();
            assert_eq!(&next, b"cd");
            file.seek(SeekFrom::Start(0)).await.unwrap();
            file.inner.lock().unwrap().read_exact(&mut bytes).unwrap();
            file.buffered_read.extend(&bytes[2..]);
            file.write_all(b"XY").await.unwrap();
            file.flush().await.unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"abXYef");
            drop(file);
            std::fs::remove_file(path).unwrap();
        });
    }

    #[test]
    fn sync_all_waits_for_an_accepted_write_and_flush_reports_errors() {
        crate::Runtime::new().unwrap().block_on(async {
            let (path, mut file) = file();
            file.set_max_buf_size(2);
            file.write_all(b"abcdef").await.unwrap();
            file.sync_all().await.unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
            file.flush().await.unwrap();
            drop(file);
            let mut readonly = File::open(&path).await.unwrap();
            assert_eq!(readonly.write(b"rejected").await.unwrap(), 8);
            assert!(readonly.flush().await.is_err());
            drop(readonly);
            std::fs::remove_file(path).unwrap();
        });
    }
}

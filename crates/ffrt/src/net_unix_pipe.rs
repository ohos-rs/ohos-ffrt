//! OpenHarmony Unix anonymous pipes and FIFO endpoints.

use std::fs::File;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::io::{AsyncRead, AsyncWrite, ReadBuf};
use crate::reactor::{AsyncFd, Interest, Ready};

fn file_flags(fd: RawFd) -> io::Result<i32> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    (flags >= 0)
        .then_some(flags)
        .ok_or_else(io::Error::last_os_error)
}

fn set_nonblocking(fd: RawFd, nonblocking: bool) -> io::Result<()> {
    let flags = file_flags(fd)?;
    let flags = if nonblocking {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    (result >= 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

fn set_close_on_exec(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    (result >= 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

fn is_pipe(file: &File) -> io::Result<bool> {
    Ok(file.metadata()?.file_type().is_fifo())
}

fn checked_file(fd: OwnedFd, write: bool) -> io::Result<File> {
    let file = File::from(fd);
    if !is_pipe(&file)? {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a pipe"));
    }
    let access = file_flags(file.as_raw_fd())? & libc::O_ACCMODE;
    let valid = if write {
        matches!(access, libc::O_WRONLY | libc::O_RDWR)
    } else {
        matches!(access, libc::O_RDONLY | libc::O_RDWR)
    };
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            if write {
                "pipe is not open for writing"
            } else {
                "pipe is not open for reading"
            },
        ));
    }
    set_nonblocking(file.as_raw_fd(), true)?;
    Ok(file)
}

/// Creates a non-blocking anonymous Unix pipe.
pub fn pipe() -> io::Result<(Sender, Receiver)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful pipe call returns two newly owned descriptors.
    let receiver = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let sender = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    set_close_on_exec(receiver.as_raw_fd())?;
    set_close_on_exec(sender.as_raw_fd())?;
    set_nonblocking(receiver.as_raw_fd(), true)?;
    set_nonblocking(sender.as_raw_fd(), true)?;
    Ok((
        Sender::from_owned_fd_unchecked(sender)?,
        Receiver::from_owned_fd_unchecked(receiver)?,
    ))
}

/// Options for opening a named FIFO endpoint.
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    read_write: bool,
    unchecked: bool,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the FIFO in read-write mode, using the Linux behavior provided by OHOS.
    pub fn read_write(&mut self, value: bool) -> &mut Self {
        self.read_write = value;
        self
    }

    /// Skips validation that the opened file is a FIFO.
    pub fn unchecked(&mut self, value: bool) -> &mut Self {
        self.unchecked = value;
        self
    }

    pub fn open_receiver<P: AsRef<Path>>(&self, path: P) -> io::Result<Receiver> {
        let file = self.open(path.as_ref(), false)?;
        Receiver::from_file_unchecked(file)
    }

    pub fn open_sender<P: AsRef<Path>>(&self, path: P) -> io::Result<Sender> {
        let file = self.open(path.as_ref(), true)?;
        Sender::from_file_unchecked(file)
    }

    fn open(&self, path: &Path, sender: bool) -> io::Result<File> {
        let mut options = std::fs::OpenOptions::new();
        options
            .read(!sender || self.read_write)
            .write(sender || self.read_write)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = options.open(path)?;
        if !self.unchecked && !is_pipe(&file)? {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a pipe"));
        }
        Ok(file)
    }
}

/// Writing end of a Unix pipe.
#[derive(Debug)]
pub struct Sender {
    inner: AsyncFd<File>,
}

impl Sender {
    pub fn from_file(file: File) -> io::Result<Self> {
        Self::from_owned_fd(file.into())
    }

    pub fn from_owned_fd(fd: OwnedFd) -> io::Result<Self> {
        Self::from_file_unchecked(checked_file(fd, true)?)
    }

    pub fn from_file_unchecked(file: File) -> io::Result<Self> {
        Ok(Self {
            inner: AsyncFd::with_interest(file, Interest::WRITABLE)?,
        })
    }

    pub fn from_owned_fd_unchecked(fd: OwnedFd) -> io::Result<Self> {
        Self::from_file_unchecked(File::from(fd))
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        Ok(self.inner.ready(interest).await?.ready())
    }

    pub async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await.map(drop)
    }

    pub fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_write_ready(cx).map_ok(drop)
    }

    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        self.try_io(|| self.inner.get_ref().write(buf))
    }

    pub fn try_write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.try_io(|| self.inner.get_ref().write_vectored(bufs))
    }

    pub fn try_io<R>(&self, operation: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
        self.inner.try_io(Interest::WRITABLE, |_| operation())
    }

    pub fn into_blocking_fd(self) -> io::Result<OwnedFd> {
        let file = self.inner.into_inner();
        set_nonblocking(file.as_raw_fd(), false)?;
        Ok(file.into())
    }

    pub fn into_nonblocking_fd(self) -> io::Result<OwnedFd> {
        let file = self.inner.into_inner();
        set_nonblocking(file.as_raw_fd(), true)?;
        Ok(file.into())
    }
}

impl AsyncWrite for Sender {
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

impl AsRawFd for Sender {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for Sender {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

/// Reading end of a Unix pipe.
#[derive(Debug)]
pub struct Receiver {
    inner: AsyncFd<File>,
}

impl Receiver {
    pub fn from_file(file: File) -> io::Result<Self> {
        Self::from_owned_fd(file.into())
    }

    pub fn from_owned_fd(fd: OwnedFd) -> io::Result<Self> {
        Self::from_file_unchecked(checked_file(fd, false)?)
    }

    pub fn from_file_unchecked(file: File) -> io::Result<Self> {
        Ok(Self {
            inner: AsyncFd::with_interest(file, Interest::READABLE)?,
        })
    }

    pub fn from_owned_fd_unchecked(fd: OwnedFd) -> io::Result<Self> {
        Self::from_file_unchecked(File::from(fd))
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        Ok(self.inner.ready(interest).await?.ready())
    }

    pub async fn readable(&self) -> io::Result<()> {
        self.inner.readable().await.map(drop)
    }

    pub fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_read_ready(cx).map_ok(drop)
    }

    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.try_io(|| self.inner.get_ref().read(buf))
    }

    pub fn try_read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.try_io(|| self.inner.get_ref().read_vectored(bufs))
    }

    pub fn try_io<R>(&self, operation: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
        self.inner.try_io(Interest::READABLE, |_| operation())
    }

    pub fn into_blocking_fd(self) -> io::Result<OwnedFd> {
        let file = self.inner.into_inner();
        set_nonblocking(file.as_raw_fd(), false)?;
        Ok(file.into())
    }

    pub fn into_nonblocking_fd(self) -> io::Result<OwnedFd> {
        let file = self.inner.into_inner();
        set_nonblocking(file.as_raw_fd(), true)?;
        Ok(file.into())
    }
}

impl AsyncRead for Receiver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            let target = buf.initialize_unfilled();
            match this.inner.get_mut().read(target) {
                Ok(amount) => {
                    buf.advance(amount);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = match this.inner.poll_read_ready(cx) {
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
}

impl AsRawFd for Receiver {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for Receiver {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

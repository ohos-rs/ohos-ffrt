//! OpenHarmony Unix-domain sockets driven by the FFRT reactor.

use std::future::poll_fn;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{
    SocketAddr, UnixDatagram as StdUnixDatagram, UnixListener as StdUnixListener,
    UnixStream as StdUnixStream,
};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::BufMut;

use crate::io::{AsyncRead, AsyncWrite, ReadBuf};
use crate::reactor::{AsyncFd, Interest, Ready};

fn try_read_buf<B: BufMut>(
    buf: &mut B,
    operation: impl FnOnce(&mut [u8]) -> io::Result<usize>,
) -> io::Result<usize> {
    let result = {
        let chunk = buf.chunk_mut();
        // SAFETY: `chunk` is the buffer's writable spare capacity.
        let slice = unsafe { std::slice::from_raw_parts_mut(chunk.as_mut_ptr(), chunk.len()) };
        operation(slice)
    };
    let amount = result?;
    // SAFETY: a successful `Read` initialized exactly `amount` bytes.
    unsafe { buf.advance_mut(amount) };
    Ok(amount)
}

/// Effective credentials of a Unix-domain socket peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UCred {
    pid: Option<libc::pid_t>,
    uid: libc::uid_t,
    gid: libc::gid_t,
}

impl UCred {
    pub fn pid(&self) -> Option<libc::pid_t> {
        self.pid
    }

    pub fn uid(&self) -> libc::uid_t {
        self.uid
    }

    pub fn gid(&self) -> libc::gid_t {
        self.gid
    }
}

fn peer_credentials(fd: RawFd) -> io::Result<UCred> {
    let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(UCred {
        pid: Some(credentials.pid),
        uid: credentials.uid,
        gid: credentials.gid,
    })
}

fn socket_addr(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    if bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket path contains a NUL byte",
        ));
    }
    if bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket path is too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    Ok((address, length as libc::socklen_t))
}

fn new_socket(kind: libc::c_int) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, kind, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let status = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if status < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, status | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    if descriptor < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, descriptor | libc::FD_CLOEXEC) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn bind_fd(fd: RawFd, path: &Path) -> io::Result<()> {
    let (address, length) = socket_addr(path)?;
    let result = unsafe {
        libc::bind(
            fd,
            (&address as *const libc::sockaddr_un).cast::<libc::sockaddr>(),
            length,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

async fn connect_fd(fd: OwnedFd, path: &Path) -> io::Result<UnixStream> {
    let (address, length) = socket_addr(path)?;
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast::<libc::sockaddr>(),
            length,
        )
    };
    let stream = unsafe { StdUnixStream::from_raw_fd(fd.into_raw_fd()) };
    if result == 0 {
        return UnixStream::from_std(stream);
    }

    let error = io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::EINPROGRESS) | Some(libc::EALREADY)
    ) && error.kind() != io::ErrorKind::WouldBlock
    {
        return Err(error);
    }

    let stream = UnixStream::from_std(stream)?;
    stream.writable().await?;
    if let Some(error) = stream.take_error()? {
        return Err(error);
    }
    Ok(stream)
}

/// A socket that has not yet been converted into a Unix stream, listener, or
/// datagram.
pub struct UnixSocket {
    fd: OwnedFd,
}

impl UnixSocket {
    pub fn new_stream() -> io::Result<Self> {
        Ok(Self {
            fd: new_socket(libc::SOCK_STREAM)?,
        })
    }

    pub fn new_datagram() -> io::Result<Self> {
        Ok(Self {
            fd: new_socket(libc::SOCK_DGRAM)?,
        })
    }

    pub fn bind(&self, path: impl AsRef<Path>) -> io::Result<()> {
        bind_fd(self.fd.as_raw_fd(), path.as_ref())
    }

    pub fn listen(self, backlog: u32) -> io::Result<UnixListener> {
        let result =
            unsafe { libc::listen(self.fd.as_raw_fd(), backlog.min(i32::MAX as u32) as i32) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let listener = unsafe { StdUnixListener::from_raw_fd(self.fd.into_raw_fd()) };
        UnixListener::from_std(listener)
    }

    pub async fn connect(self, path: impl AsRef<Path>) -> io::Result<UnixStream> {
        connect_fd(self.fd, path.as_ref()).await
    }

    pub fn datagram(self) -> io::Result<UnixDatagram> {
        let datagram = unsafe { StdUnixDatagram::from_raw_fd(self.fd.into_raw_fd()) };
        UnixDatagram::from_std(datagram)
    }
}

impl AsRawFd for UnixSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// A non-blocking Unix-domain stream.
pub struct UnixStream {
    inner: Arc<AsyncFd<StdUnixStream>>,
}

impl UnixStream {
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        UnixSocket::new_stream()?.connect(path).await
    }

    pub async fn connect_addr(address: &SocketAddr) -> io::Result<Self> {
        let path = address
            .as_pathname()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unnamed Unix address"))?;
        Self::connect(path).await
    }

    pub fn from_std(stream: StdUnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            inner: Arc::new(AsyncFd::new(stream)?),
        })
    }

    pub fn into_std(self) -> io::Result<StdUnixStream> {
        Arc::try_unwrap(self.inner)
            .map(AsyncFd::into_inner)
            .map_err(|_| io::Error::other("Unix stream is still shared"))
    }

    pub fn pair() -> io::Result<(Self, Self)> {
        let (left, right) = StdUnixStream::pair()?;
        Ok((Self::from_std(left)?, Self::from_std(right)?))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub fn peer_cred(&self) -> io::Result<UCred> {
        peer_credentials(self.as_raw_fd())
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.get_ref().take_error()
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        Ok(self.inner.ready(interest).await?.ready())
    }

    pub async fn readable(&self) -> io::Result<()> {
        let _ = self.inner.readable().await?;
        Ok(())
    }

    pub async fn writable(&self) -> io::Result<()> {
        let _ = self.inner.writable().await?;
        Ok(())
    }

    pub fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner
            .poll_read_ready(cx)
            .map(|result| result.map(drop))
    }

    pub fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner
            .poll_write_ready(cx)
            .map(|result| result.map(drop))
    }

    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read(buf)
    }

    pub fn try_read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read_vectored(bufs)
    }

    pub fn try_read_buf<B: BufMut>(&self, buf: &mut B) -> io::Result<usize> {
        try_read_buf(buf, |slice| self.try_read(slice))
    }

    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.write(buf)
    }

    pub fn try_write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.write_vectored(bufs)
    }

    pub fn try_io<R>(
        &self,
        interest: Interest,
        operation: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        self.inner.try_io(interest, |_| operation())
    }

    pub async fn async_io<R>(
        &self,
        interest: Interest,
        mut operation: impl FnMut() -> io::Result<R>,
    ) -> io::Result<R> {
        loop {
            let mut guard = self.inner.ready(interest).await?;
            match guard.try_io(|_| operation()) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }

    pub fn split(&mut self) -> (UnixReadHalf<'_>, UnixWriteHalf<'_>) {
        (UnixReadHalf { inner: self }, UnixWriteHalf { inner: self })
    }

    pub fn into_split(self) -> (UnixOwnedReadHalf, UnixOwnedWriteHalf) {
        let inner = self.inner;
        (
            UnixOwnedReadHalf {
                inner: inner.clone(),
            },
            UnixOwnedWriteHalf { inner: Some(inner) },
        )
    }

    fn poll_read_slice(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        poll_stream_io(&self.inner, cx, Interest::READABLE, || {
            let mut stream = self.inner.get_ref();
            stream.read(buf)
        })
    }

    fn poll_write_slice(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        poll_stream_io(&self.inner, cx, Interest::WRITABLE, || {
            let mut stream = self.inner.get_ref();
            stream.write(buf)
        })
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.poll_read_slice(cx, buf.initialize_unfilled()) {
            Poll::Ready(Ok(amount)) => {
                buf.advance(amount);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_slice(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.shutdown(Shutdown::Write))
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for UnixStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

fn poll_stream_io<R>(
    inner: &AsyncFd<StdUnixStream>,
    cx: &mut Context<'_>,
    interest: Interest,
    mut operation: impl FnMut() -> io::Result<R>,
) -> Poll<io::Result<R>> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let readiness = if interest.is_readable() {
                    inner.poll_read_ready(cx)
                } else {
                    inner.poll_write_ready(cx)
                };
                let mut guard = match readiness {
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

pub struct UnixReadHalf<'a> {
    inner: &'a UnixStream,
}

impl UnixReadHalf<'_> {
    pub fn is_pair_of(&self, other: &UnixWriteHalf<'_>) -> bool {
        std::ptr::eq(self.inner, other.inner)
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        self.inner.ready(interest).await
    }

    pub async fn readable(&self) -> io::Result<()> {
        self.inner.readable().await
    }

    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.try_read(buf)
    }

    pub fn try_read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.try_read_vectored(bufs)
    }

    pub fn try_read_buf<B: BufMut>(&self, buf: &mut B) -> io::Result<usize> {
        try_read_buf(buf, |slice| self.try_read(slice))
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

impl AsyncRead for UnixReadHalf<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.inner.poll_read_slice(cx, buf.initialize_unfilled()) {
            Poll::Ready(Ok(amount)) => {
                buf.advance(amount);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct UnixWriteHalf<'a> {
    inner: &'a UnixStream,
}

impl UnixWriteHalf<'_> {
    pub fn is_pair_of(&self, other: &UnixReadHalf<'_>) -> bool {
        other.is_pair_of(self)
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        self.inner.ready(interest).await
    }

    pub async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await
    }

    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.try_write(buf)
    }

    pub fn try_write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.try_write_vectored(bufs)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

impl AsyncWrite for UnixWriteHalf<'_> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_write_slice(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.inner.shutdown(Shutdown::Write))
    }
}

pub struct UnixOwnedReadHalf {
    inner: Arc<AsyncFd<StdUnixStream>>,
}

impl UnixOwnedReadHalf {
    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        let guard = self.inner.ready(interest).await?;
        Ok(guard.ready())
    }

    pub async fn readable(&self) -> io::Result<()> {
        let _ = self.inner.readable().await?;
        Ok(())
    }

    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read(buf)
    }

    pub fn try_read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read_vectored(bufs)
    }

    pub fn try_read_buf<B: BufMut>(&self, buf: &mut B) -> io::Result<usize> {
        try_read_buf(buf, |slice| self.try_read(slice))
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn reunite(self, other: UnixOwnedWriteHalf) -> Result<UnixStream, UnixReuniteError> {
        if other
            .inner
            .as_ref()
            .is_some_and(|write| Arc::ptr_eq(&self.inner, write))
        {
            let mut other = other;
            let _ = other.inner.take();
            Ok(UnixStream { inner: self.inner })
        } else {
            Err(UnixReuniteError(self, other))
        }
    }
}

impl AsyncRead for UnixOwnedReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match poll_stream_io(&self.inner, cx, Interest::READABLE, || {
            let mut stream = self.inner.get_ref();
            stream.read(buf.initialize_unfilled())
        }) {
            Poll::Ready(Ok(amount)) => {
                buf.advance(amount);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsRawFd for UnixOwnedReadHalf {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for UnixOwnedReadHalf {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

pub struct UnixOwnedWriteHalf {
    inner: Option<Arc<AsyncFd<StdUnixStream>>>,
}

impl UnixOwnedWriteHalf {
    fn inner(&self) -> &Arc<AsyncFd<StdUnixStream>> {
        self.inner.as_ref().expect("owned Unix write half missing")
    }

    pub fn forget(mut self) {
        self.inner.take();
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        let guard = self.inner().ready(interest).await?;
        Ok(guard.ready())
    }

    pub async fn writable(&self) -> io::Result<()> {
        let _ = self.inner().writable().await?;
        Ok(())
    }

    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        let mut stream = self.inner().get_ref();
        stream.write(buf)
    }

    pub fn try_write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut stream = self.inner().get_ref();
        stream.write_vectored(bufs)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner().get_ref().peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner().get_ref().local_addr()
    }
}

impl AsyncWrite for UnixOwnedWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        poll_stream_io(self.inner(), cx, Interest::WRITABLE, || {
            let mut stream = self.inner().get_ref();
            stream.write(buf)
        })
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.inner().get_ref().shutdown(Shutdown::Write))
    }
}

impl AsRawFd for UnixOwnedWriteHalf {
    fn as_raw_fd(&self) -> RawFd {
        self.inner().as_raw_fd()
    }
}

impl AsFd for UnixOwnedWriteHalf {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner().as_fd()
    }
}

impl Drop for UnixOwnedWriteHalf {
    fn drop(&mut self) {
        if let Some(inner) = &self.inner {
            let _ = inner.get_ref().shutdown(Shutdown::Write);
        }
    }
}

pub struct UnixReuniteError(pub UnixOwnedReadHalf, pub UnixOwnedWriteHalf);

impl std::fmt::Debug for UnixReuniteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReuniteError").finish_non_exhaustive()
    }
}

impl std::fmt::Display for UnixReuniteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tried to reunite Unix halves from different streams")
    }
}

impl std::error::Error for UnixReuniteError {}

/// A non-blocking Unix-domain listener.
pub struct UnixListener {
    inner: Arc<AsyncFd<StdUnixListener>>,
}

impl UnixListener {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let socket = UnixSocket::new_stream()?;
        socket.bind(path)?;
        socket.listen(1024)
    }

    pub fn bind_addr(address: &SocketAddr) -> io::Result<Self> {
        let path = address
            .as_pathname()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unnamed Unix address"))?;
        Self::bind(path)
    }

    pub fn from_std(listener: StdUnixListener) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        Ok(Self {
            inner: Arc::new(AsyncFd::with_interest(listener, Interest::READABLE)?),
        })
    }

    pub fn into_std(self) -> io::Result<StdUnixListener> {
        Arc::try_unwrap(self.inner)
            .map(AsyncFd::into_inner)
            .map_err(|_| io::Error::other("Unix listener is still shared"))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.get_ref().take_error()
    }

    pub async fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        poll_fn(|cx| self.poll_accept(cx)).await
    }

    pub fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<io::Result<(UnixStream, SocketAddr)>> {
        loop {
            match self.inner.get_ref().accept() {
                Ok((stream, address)) => {
                    return Poll::Ready(
                        UnixStream::from_std(stream).map(|stream| (stream, address)),
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = match self.inner.poll_read_ready(cx) {
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

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

/// A non-blocking Unix-domain datagram socket.
pub struct UnixDatagram {
    inner: Arc<AsyncFd<StdUnixDatagram>>,
}

impl UnixDatagram {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let socket = UnixSocket::new_datagram()?;
        socket.bind(path)?;
        socket.datagram()
    }

    pub fn unbound() -> io::Result<Self> {
        UnixSocket::new_datagram()?.datagram()
    }

    pub fn pair() -> io::Result<(Self, Self)> {
        let (left, right) = StdUnixDatagram::pair()?;
        Ok((Self::from_std(left)?, Self::from_std(right)?))
    }

    pub fn from_std(datagram: StdUnixDatagram) -> io::Result<Self> {
        datagram.set_nonblocking(true)?;
        Ok(Self {
            inner: Arc::new(AsyncFd::new(datagram)?),
        })
    }

    pub fn into_std(self) -> io::Result<StdUnixDatagram> {
        Arc::try_unwrap(self.inner)
            .map(AsyncFd::into_inner)
            .map_err(|_| io::Error::other("Unix datagram is still shared"))
    }

    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.inner.get_ref().connect(path)
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        Ok(self.inner.ready(interest).await?.ready())
    }

    pub async fn readable(&self) -> io::Result<()> {
        let _ = self.inner.readable().await?;
        Ok(())
    }

    pub async fn writable(&self) -> io::Result<()> {
        let _ = self.inner.writable().await?;
        Ok(())
    }

    pub fn poll_recv_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner
            .poll_read_ready(cx)
            .map(|result| result.map(drop))
    }

    pub fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner
            .poll_write_ready(cx)
            .map(|result| result.map(drop))
    }

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_send(cx, buf)).await
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| {
            poll_datagram_io(&self.inner, cx, Interest::READABLE, || {
                self.inner.get_ref().recv(buf)
            })
        })
        .await
    }

    pub async fn send_to<P: AsRef<Path>>(&self, buf: &[u8], target: P) -> io::Result<usize> {
        poll_fn(|cx| self.poll_send_to(cx, buf, &target)).await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        poll_fn(|cx| self.poll_recv_from(cx, buf)).await
    }

    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.get_ref().send(buf)
    }

    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.get_ref().recv(buf)
    }

    pub fn try_send_to<P: AsRef<Path>>(&self, buf: &[u8], target: P) -> io::Result<usize> {
        self.inner.get_ref().send_to(buf, target)
    }

    pub fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.get_ref().recv_from(buf)
    }

    pub fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        poll_datagram_io(&self.inner, cx, Interest::WRITABLE, || {
            self.inner.get_ref().send(buf)
        })
    }

    pub fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match poll_datagram_io(&self.inner, cx, Interest::READABLE, || {
            self.inner.get_ref().recv(buf.initialize_unfilled())
        }) {
            Poll::Ready(Ok(amount)) => {
                buf.advance(amount);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn poll_send_to<P: AsRef<Path>>(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
        target: P,
    ) -> Poll<io::Result<usize>> {
        poll_datagram_io(&self.inner, cx, Interest::WRITABLE, || {
            self.inner.get_ref().send_to(buf, target.as_ref())
        })
    }

    pub fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, SocketAddr)>> {
        poll_datagram_io(&self.inner, cx, Interest::READABLE, || {
            self.inner.get_ref().recv_from(buf)
        })
    }

    pub fn try_io<R>(
        &self,
        interest: Interest,
        operation: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        self.inner.try_io(interest, |_| operation())
    }

    pub async fn async_io<R>(
        &self,
        interest: Interest,
        mut operation: impl FnMut() -> io::Result<R>,
    ) -> io::Result<R> {
        loop {
            let mut guard = self.inner.ready(interest).await?;
            match guard.try_io(|_| operation()) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.get_ref().take_error()
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }
}

fn poll_datagram_io<R>(
    inner: &AsyncFd<StdUnixDatagram>,
    cx: &mut Context<'_>,
    interest: Interest,
    mut operation: impl FnMut() -> io::Result<R>,
) -> Poll<io::Result<R>> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let readiness = if interest.is_readable() {
                    inner.poll_read_ready(cx)
                } else {
                    inner.poll_write_ready(cx)
                };
                let mut guard = match readiness {
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

impl AsRawFd for UnixDatagram {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for UnixDatagram {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

impl std::fmt::Debug for UnixSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixSocket")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

impl std::fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixStream")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

impl std::fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixListener")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

impl std::fmt::Debug for UnixDatagram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixDatagram")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

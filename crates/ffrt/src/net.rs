//! Non-blocking networking driven by the FFRT reactor.

use std::future::poll_fn;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::io::{AsyncRead, AsyncWrite, ReadBuf};
use crate::reactor::{AsyncFd, Interest};

async fn resolve<A>(addr: A) -> io::Result<Vec<SocketAddr>>
where
    A: ToSocketAddrs + Send + 'static,
{
    match crate::spawn_blocking(move || addr.to_socket_addrs().map(Iterator::collect)).await {
        Ok(result) => result,
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

fn no_addresses() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "address resolved to no socket addresses",
    )
}

/// A non-blocking TCP stream.
pub struct TcpStream {
    inner: Arc<AsyncFd<std::net::TcpStream>>,
}

impl TcpStream {
    /// Connects to a remote address without blocking an FFRT worker while the
    /// connection is in progress.
    pub async fn connect<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let addresses = resolve(addr).await?;
        let mut last_error = None;
        for address in addresses {
            match connect_addr(address).await {
                Ok(stream) => return Self::from_std(stream),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(no_addresses))
    }

    /// Creates a reactor-backed stream from a non-blocking standard stream.
    pub fn from_std(stream: std::net::TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            inner: Arc::new(AsyncFd::new(stream)?),
        })
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    /// Returns the peer address.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    /// Waits until the stream may be read without blocking.
    pub async fn readable(&self) -> io::Result<()> {
        let _ = self.inner.readable().await?;
        Ok(())
    }

    /// Waits until the stream may be written without blocking.
    pub async fn writable(&self) -> io::Result<()> {
        let _ = self.inner.writable().await?;
        Ok(())
    }

    /// Attempts an immediate non-blocking read.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read(buf)
    }

    /// Attempts an immediate non-blocking write.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.write(buf)
    }

    /// Reads up to `buf.len()` bytes and returns the supplied buffer.
    pub async fn read(&self, mut buf: Vec<u8>) -> io::Result<(usize, Vec<u8>)> {
        let n = poll_fn(|cx| self.poll_read_slice(cx, &mut buf)).await?;
        Ok((n, buf))
    }

    /// Reads bytes until EOF.
    pub async fn read_to_end(&self) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut chunk = vec![0; 8 * 1024];
        loop {
            let n = poll_fn(|cx| self.poll_read_slice(cx, &mut chunk)).await?;
            if n == 0 {
                return Ok(output);
            }
            output.extend_from_slice(&chunk[..n]);
        }
    }

    /// Writes some bytes from an owned buffer.
    pub async fn write(&self, data: Vec<u8>) -> io::Result<usize> {
        poll_fn(|cx| self.poll_write_slice(cx, &data)).await
    }

    /// Writes an entire owned buffer.
    pub async fn write_all(&self, data: Vec<u8>) -> io::Result<()> {
        let mut written = 0;
        while written < data.len() {
            let n = poll_fn(|cx| self.poll_write_slice(cx, &data[written..])).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ));
            }
            written += n;
        }
        Ok(())
    }

    /// Shuts down one or both halves of the connection.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }

    /// Returns a separately registered clone of the stream handle.
    pub async fn try_clone(&self) -> io::Result<Self> {
        Self::from_std(self.inner.get_ref().try_clone()?)
    }

    fn poll_read_slice(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        loop {
            match self.try_read(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let guard = match self.inner.poll_read_ready(cx) {
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

    fn poll_write_slice(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            match self.try_write(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let guard = match self.inner.poll_write_ready(cx) {
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
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.poll_read_slice(cx, buf.unfilled_mut()) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for TcpStream {
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
        Poll::Ready(self.inner.get_ref().shutdown(Shutdown::Write))
    }
}

/// A non-blocking TCP listener.
pub struct TcpListener {
    inner: Arc<AsyncFd<std::net::TcpListener>>,
}

impl TcpListener {
    /// Binds a listener to `addr`.
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let addresses = resolve(addr).await?;
        let mut last_error = None;
        for address in addresses {
            match std::net::TcpListener::bind(address) {
                Ok(listener) => return Self::from_std(listener),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(no_addresses))
    }

    /// Creates a reactor-backed listener from a standard listener.
    pub fn from_std(listener: std::net::TcpListener) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        Ok(Self {
            inner: Arc::new(AsyncFd::with_interest(listener, Interest::READABLE)?),
        })
    }

    /// Accepts one connection without blocking an FFRT worker.
    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        poll_fn(|cx| self.poll_accept(cx)).await
    }

    /// Attempts an immediate non-blocking accept.
    pub fn try_accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (stream, address) = self.inner.get_ref().accept()?;
        Ok((TcpStream::from_std(stream)?, address))
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        loop {
            match self.try_accept() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let guard = match self.inner.poll_read_ready(cx) {
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
}

/// A non-blocking UDP socket.
pub struct UdpSocket {
    inner: Arc<AsyncFd<std::net::UdpSocket>>,
}

impl UdpSocket {
    /// Binds a UDP socket to `addr`.
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let addresses = resolve(addr).await?;
        let mut last_error = None;
        for address in addresses {
            match std::net::UdpSocket::bind(address) {
                Ok(socket) => return Self::from_std(socket),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(no_addresses))
    }

    /// Creates a reactor-backed socket from a standard UDP socket.
    pub fn from_std(socket: std::net::UdpSocket) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            inner: Arc::new(AsyncFd::new(socket)?),
        })
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    /// Connects this UDP socket to a remote address.
    pub async fn connect<A>(&self, addr: A) -> io::Result<()>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let address = resolve(addr)
            .await?
            .into_iter()
            .next()
            .ok_or_else(no_addresses)?;
        self.inner.get_ref().connect(address)
    }

    /// Receives one datagram.
    pub async fn recv_from(&self, mut buf: Vec<u8>) -> io::Result<(usize, SocketAddr, Vec<u8>)> {
        let (n, address) = poll_fn(|cx| self.poll_recv_from(cx, &mut buf)).await?;
        Ok((n, address, buf))
    }

    /// Sends one datagram.
    pub async fn send_to<A>(&self, data: Vec<u8>, addr: A) -> io::Result<usize>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let address = resolve(addr)
            .await?
            .into_iter()
            .next()
            .ok_or_else(no_addresses)?;
        poll_fn(|cx| self.poll_send_to(cx, &data, address)).await
    }

    /// Receives bytes from a connected peer.
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_recv(cx, buf)).await
    }

    /// Sends bytes to a connected peer.
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_send(cx, buf)).await
    }

    fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, SocketAddr)>> {
        self.poll_io(cx, Interest::READABLE, || {
            self.inner.get_ref().recv_from(buf)
        })
    }

    fn poll_send_to(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
        addr: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        self.poll_io(cx, Interest::WRITABLE, || {
            self.inner.get_ref().send_to(buf, addr)
        })
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.poll_io(cx, Interest::READABLE, || self.inner.get_ref().recv(buf))
    }

    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.poll_io(cx, Interest::WRITABLE, || self.inner.get_ref().send(buf))
    }

    fn poll_io<R>(
        &self,
        cx: &mut Context<'_>,
        interest: Interest,
        mut operation: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        loop {
            match operation() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let ready = if interest.is_readable() {
                        self.inner.poll_read_ready(cx)
                    } else {
                        self.inner.poll_write_ready(cx)
                    };
                    let guard = match ready {
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
}

#[cfg(any(target_env = "ohos", target_os = "linux"))]
async fn connect_addr(address: SocketAddr) -> io::Result<std::net::TcpStream> {
    use std::os::fd::FromRawFd;
    use std::os::raw::{c_int, c_void};

    const AF_INET: c_int = 2;
    const AF_INET6: c_int = 10;
    const SOCK_STREAM: c_int = 1;
    const SOCK_NONBLOCK: c_int = 0o00004000;
    const SOCK_CLOEXEC: c_int = 0o02000000;
    const EINPROGRESS: i32 = 115;
    const EALREADY: i32 = 114;

    unsafe extern "C" {
        fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
        fn connect(fd: c_int, address: *const c_void, len: u32) -> c_int;
    }

    #[repr(C)]
    struct SockAddrV4 {
        family: u16,
        port: u16,
        address: u32,
        zero: [u8; 8],
    }

    #[repr(C)]
    struct SockAddrV6 {
        family: u16,
        port: u16,
        flow_info: u32,
        address: [u8; 16],
        scope_id: u32,
    }

    let domain = if address.is_ipv4() { AF_INET } else { AF_INET6 };
    let fd = unsafe { socket(domain, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = match address {
        SocketAddr::V4(address) => {
            let raw = SockAddrV4 {
                family: AF_INET as u16,
                port: address.port().to_be(),
                address: u32::from_ne_bytes(address.ip().octets()),
                zero: [0; 8],
            };
            unsafe {
                connect(
                    fd,
                    (&raw as *const SockAddrV4).cast::<c_void>(),
                    size_of::<SockAddrV4>() as u32,
                )
            }
        }
        SocketAddr::V6(address) => {
            let raw = SockAddrV6 {
                family: AF_INET6 as u16,
                port: address.port().to_be(),
                flow_info: address.flowinfo().to_be(),
                address: address.ip().octets(),
                scope_id: address.scope_id(),
            };
            unsafe {
                connect(
                    fd,
                    (&raw as *const SockAddrV6).cast::<c_void>(),
                    size_of::<SockAddrV6>() as u32,
                )
            }
        }
    };

    let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
    if result == 0 {
        return Ok(stream);
    }

    let error = io::Error::last_os_error();
    if !matches!(error.raw_os_error(), Some(EINPROGRESS) | Some(EALREADY))
        && error.kind() != io::ErrorKind::WouldBlock
    {
        return Err(error);
    }

    let fd = AsyncFd::with_interest(stream, Interest::WRITABLE)?;
    let guard = fd.writable().await?;
    guard.clear_ready();
    if let Some(error) = fd.get_ref().take_error()? {
        return Err(error);
    }
    Ok(fd.into_inner())
}

#[cfg(not(any(target_env = "ohos", target_os = "linux")))]
async fn connect_addr(address: SocketAddr) -> io::Result<std::net::TcpStream> {
    let result = crate::spawn_blocking(move || std::net::TcpStream::connect(address)).await;
    match result {
        Ok(result) => result,
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

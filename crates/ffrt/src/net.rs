//! Non-blocking networking driven by the FFRT reactor.

#[path = "net_tcp_socket.rs"]
mod tcp_socket_impl;
#[cfg(target_env = "ohos")]
#[path = "net_unix.rs"]
mod unix_impl;
#[cfg(target_env = "ohos")]
#[path = "net_unix_pipe.rs"]
mod unix_pipe_impl;

pub use tcp_socket_impl::TcpSocket;
#[cfg(target_env = "ohos")]
pub use unix_impl::{UnixDatagram, UnixListener, UnixSocket, UnixStream};

/// OpenHarmony Unix-domain networking types.
#[cfg(target_env = "ohos")]
pub mod unix {
    pub use std::os::unix::net::SocketAddr;

    #[allow(non_camel_case_types)]
    pub type uid_t = u32;
    #[allow(non_camel_case_types)]
    pub type gid_t = u32;
    #[allow(non_camel_case_types)]
    pub type pid_t = i32;

    /// Unix anonymous pipe and FIFO support.
    pub mod pipe {
        pub use super::super::unix_pipe_impl::{OpenOptions, Receiver, Sender, pipe};
    }

    pub use super::unix_impl::{
        UCred, UnixDatagram, UnixListener, UnixOwnedReadHalf as OwnedReadHalf,
        UnixOwnedWriteHalf as OwnedWriteHalf, UnixReadHalf as ReadHalf,
        UnixReuniteError as ReuniteError, UnixSocket, UnixStream, UnixWriteHalf as WriteHalf,
    };
}

use std::future::poll_fn;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
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
        // SAFETY: `chunk` describes writable spare capacity. The operation may
        // initialize at most its reported length, which is advanced below.
        let slice = unsafe { std::slice::from_raw_parts_mut(chunk.as_mut_ptr(), chunk.len()) };
        operation(slice)
    };
    let amount = result?;
    // SAFETY: a successful `Read` initialized exactly `amount` bytes.
    unsafe { buf.advance_mut(amount) };
    Ok(amount)
}

fn poll_tcp_peek(
    inner: &AsyncFd<std::net::TcpStream>,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
) -> Poll<io::Result<usize>> {
    loop {
        match inner.get_ref().peek(buf.initialize_unfilled()) {
            Ok(amount) => {
                buf.advance(amount);
                return Poll::Ready(Ok(amount));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut guard = match inner.poll_read_ready(cx) {
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

#[cfg(target_env = "ohos")]
fn get_socket_int(fd: RawFd, level: i32, option: i32) -> io::Result<i32> {
    let mut value = 0;
    let mut length = std::mem::size_of_val(&value) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            level,
            option,
            (&mut value as *mut i32).cast(),
            &mut length,
        )
    };
    (result == 0)
        .then_some(value)
        .ok_or_else(io::Error::last_os_error)
}

#[cfg(target_env = "ohos")]
fn set_socket_int(fd: RawFd, level: i32, option: i32, value: u32) -> io::Result<()> {
    let value = i32::try_from(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket value is too large"))?;
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            option,
            (&value as *const i32).cast(),
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    (result == 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

#[cfg(target_env = "ohos")]
fn socket_device(fd: RawFd) -> io::Result<Option<Vec<u8>>> {
    let mut device = vec![0u8; libc::IFNAMSIZ];
    let mut length = device.len() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            device.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    device.truncate(length as usize);
    if device.last() == Some(&0) {
        device.pop();
    }
    Ok((!device.is_empty()).then_some(device))
}

#[cfg(target_env = "ohos")]
fn bind_socket_device(fd: RawFd, interface: Option<&[u8]>) -> io::Result<()> {
    let mut interface = interface.unwrap_or_default().to_vec();
    if interface.contains(&0) || interface.len() >= libc::IFNAMSIZ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid network interface name",
        ));
    }
    if !interface.is_empty() {
        interface.push(0);
    }
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            interface.as_ptr().cast(),
            interface.len() as libc::socklen_t,
        )
    };
    (result == 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

#[doc(hidden)]
pub enum AddressQuery {
    Ready(Vec<SocketAddr>),
    Host(String),
    HostPort(String, u16),
}

/// Address types accepted by FFRT networking operations.
///
/// Borrowed host names are copied before DNS resolution is moved to an FFRT
/// blocking task, so callers do not need a `'static` address value.
pub trait ToSocketAddrs {
    #[doc(hidden)]
    fn into_address_query(self) -> AddressQuery;
}

macro_rules! ready_address {
    ($type:ty, $convert:expr) => {
        impl ToSocketAddrs for $type {
            fn into_address_query(self) -> AddressQuery {
                AddressQuery::Ready(vec![($convert)(self)])
            }
        }
    };
}

ready_address!(SocketAddr, |address| address);
ready_address!(SocketAddrV4, SocketAddr::V4);
ready_address!(SocketAddrV6, SocketAddr::V6);
ready_address!((IpAddr, u16), |(ip, port)| SocketAddr::new(ip, port));
ready_address!((Ipv4Addr, u16), |(ip, port)| SocketAddr::new(
    IpAddr::V4(ip),
    port
));
ready_address!((Ipv6Addr, u16), |(ip, port)| SocketAddr::new(
    IpAddr::V6(ip),
    port
));

impl ToSocketAddrs for &str {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::Host(self.to_owned())
    }
}

impl ToSocketAddrs for String {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::Host(self)
    }
}

impl ToSocketAddrs for &String {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::Host(self.clone())
    }
}

impl ToSocketAddrs for (&str, u16) {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::HostPort(self.0.to_owned(), self.1)
    }
}

impl ToSocketAddrs for (String, u16) {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::HostPort(self.0, self.1)
    }
}

impl ToSocketAddrs for (&String, u16) {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::HostPort(self.0.clone(), self.1)
    }
}

impl ToSocketAddrs for &[SocketAddr] {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::Ready(self.to_vec())
    }
}

impl<const N: usize> ToSocketAddrs for &[SocketAddr; N] {
    fn into_address_query(self) -> AddressQuery {
        AddressQuery::Ready(self.to_vec())
    }
}

async fn resolve<A: ToSocketAddrs>(addr: A) -> io::Result<Vec<SocketAddr>> {
    use std::net::ToSocketAddrs as _;

    let query = addr.into_address_query();
    match crate::spawn_blocking(move || match query {
        AddressQuery::Ready(addresses) => Ok(addresses),
        AddressQuery::Host(host) => host.to_socket_addrs().map(Iterator::collect),
        AddressQuery::HostPort(host, port) => (host, port).to_socket_addrs().map(Iterator::collect),
    })
    .await
    {
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

/// Performs asynchronous DNS/address resolution on an FFRT blocking task.
pub async fn lookup_host<T>(host: T) -> io::Result<impl Iterator<Item = SocketAddr>>
where
    T: ToSocketAddrs,
{
    Ok(resolve(host).await?.into_iter())
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
        A: ToSocketAddrs,
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

    /// Deregisters the stream and returns the standard non-blocking stream.
    pub fn into_std(self) -> io::Result<std::net::TcpStream> {
        match Arc::try_unwrap(self.inner) {
            Ok(inner) => Ok(inner.into_inner()),
            Err(_) => Err(io::Error::other(
                "TCP stream is still shared by an owned split half",
            )),
        }
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    /// Returns the peer address.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    /// Returns the pending socket error, if any.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.get_ref().take_error()
    }

    /// Waits for any of the requested readiness interests.
    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        Ok(self.inner.ready(interest).await?.ready())
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

    /// Polls for read readiness.
    pub fn poll_read_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner
            .poll_read_ready(cx)
            .map(|result| result.map(drop))
    }

    /// Polls for write readiness.
    pub fn poll_write_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner
            .poll_write_ready(cx)
            .map(|result| result.map(drop))
    }

    /// Attempts an immediate non-blocking read.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read(buf)
    }

    /// Attempts an immediate vectored read.
    pub fn try_read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.read_vectored(bufs)
    }

    pub fn try_read_buf<B: BufMut>(&self, buf: &mut B) -> io::Result<usize> {
        try_read_buf(buf, |slice| self.try_read(slice))
    }

    /// Attempts an immediate non-blocking write.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.write(buf)
    }

    /// Attempts an immediate vectored write.
    pub fn try_write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut stream = self.inner.get_ref();
        stream.write_vectored(bufs)
    }

    /// Reads into an owned buffer and returns it to the caller.
    pub async fn read_owned(&self, mut buf: Vec<u8>) -> io::Result<(usize, Vec<u8>)> {
        let n = poll_fn(|cx| self.poll_read_slice(cx, &mut buf)).await?;
        Ok((n, buf))
    }

    /// Reads all remaining bytes into a newly allocated buffer.
    pub async fn read_to_end_owned(&self) -> io::Result<Vec<u8>> {
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
    pub async fn write_owned(&self, data: Vec<u8>) -> io::Result<usize> {
        poll_fn(|cx| self.poll_write_slice(cx, &data)).await
    }

    /// Writes an entire owned buffer.
    pub async fn write_all_owned(&self, data: Vec<u8>) -> io::Result<()> {
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

    /// Peeks at incoming data without consuming it.
    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.async_io(Interest::READABLE, || self.inner.get_ref().peek(buf))
            .await
    }

    /// Polls a non-consuming read from the stream.
    pub fn poll_peek(
        &self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<usize>> {
        poll_tcp_peek(&self.inner, cx, buf)
    }

    /// Runs an immediate I/O operation if the requested direction is ready.
    pub fn try_io<R>(
        &self,
        interest: Interest,
        operation: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        self.inner.try_io(interest, |_| operation())
    }

    /// Repeats an I/O operation until it no longer returns `WouldBlock`.
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

    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.get_ref().nodelay()
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.get_ref().set_nodelay(nodelay)
    }

    #[cfg(target_env = "ohos")]
    pub fn quickack(&self) -> io::Result<bool> {
        get_socket_int(self.as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_QUICKACK)
            .map(|value| value != 0)
    }

    #[cfg(target_env = "ohos")]
    pub fn set_quickack(&self, quickack: bool) -> io::Result<()> {
        set_socket_int(
            self.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_QUICKACK,
            u32::from(quickack),
        )
    }

    pub fn linger(&self) -> io::Result<Option<std::time::Duration>> {
        let mut linger = libc::linger {
            l_onoff: 0,
            l_linger: 0,
        };
        let mut length = std::mem::size_of_val(&linger) as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&mut linger as *mut libc::linger).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((linger.l_onoff != 0)
            .then(|| std::time::Duration::from_secs(linger.l_linger.max(0) as u64)))
    }

    #[deprecated = "`SO_LINGER` may block the thread when a socket is dropped"]
    pub fn set_linger(&self, duration: Option<std::time::Duration>) -> io::Result<()> {
        let linger = libc::linger {
            l_onoff: i32::from(duration.is_some()),
            l_linger: duration
                .map(|duration| duration.as_secs().min(i32::MAX as u64) as i32)
                .unwrap_or(0),
        };
        let result = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&linger as *const libc::linger).cast(),
                std::mem::size_of_val(&linger) as libc::socklen_t,
            )
        };
        (result == 0)
            .then_some(())
            .ok_or_else(io::Error::last_os_error)
    }

    pub fn set_zero_linger(&self) -> io::Result<()> {
        #[allow(deprecated)]
        self.set_linger(Some(std::time::Duration::ZERO))
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.get_ref().ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_ttl(ttl)
    }

    /// Splits this stream into borrowed read and write halves.
    pub fn split(&mut self) -> (TcpReadHalf<'_>, TcpWriteHalf<'_>) {
        (TcpReadHalf { inner: self }, TcpWriteHalf { inner: self })
    }

    /// Splits this stream into independently owned read and write halves.
    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        let inner = self.inner;
        (
            OwnedReadHalf {
                inner: inner.clone(),
            },
            OwnedWriteHalf { inner: Some(inner) },
        )
    }

    fn poll_read_slice(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        loop {
            match self.try_read(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = match self.inner.poll_read_ready(cx) {
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
                    let mut guard = match self.inner.poll_write_ready(cx) {
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
        match self.poll_read_slice(cx, buf.initialize_unfilled()) {
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

impl AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for TcpStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

/// Borrowed read half of a [`TcpStream`].
pub struct TcpReadHalf<'a> {
    inner: &'a TcpStream,
}

impl TcpReadHalf<'_> {
    pub fn is_pair_of(&self, other: &TcpWriteHalf<'_>) -> bool {
        std::ptr::eq(self.inner, other.inner)
    }

    pub async fn readable(&self) -> io::Result<()> {
        self.inner.readable().await
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        self.inner.ready(interest).await
    }

    pub fn poll_peek(
        &self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_peek(cx, buf)
    }

    pub async fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf).await
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

impl AsyncRead for TcpReadHalf<'_> {
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

/// Borrowed write half of a [`TcpStream`].
pub struct TcpWriteHalf<'a> {
    inner: &'a TcpStream,
}

impl TcpWriteHalf<'_> {
    pub fn is_pair_of(&self, other: &TcpReadHalf<'_>) -> bool {
        other.is_pair_of(self)
    }

    pub async fn writable(&self) -> io::Result<()> {
        self.inner.writable().await
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        self.inner.ready(interest).await
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

impl AsyncWrite for TcpWriteHalf<'_> {
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
        Poll::Ready(self.inner.inner.get_ref().shutdown(Shutdown::Write))
    }
}

/// Owned read half of a [`TcpStream`].
pub struct OwnedReadHalf {
    inner: Arc<AsyncFd<std::net::TcpStream>>,
}

impl OwnedReadHalf {
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

    pub fn poll_peek(
        &self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<usize>> {
        poll_tcp_peek(&self.inner, cx, buf)
    }

    pub async fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.inner.get_ref().peek(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = self.inner.readable().await?;
                    guard.clear_ready();
                }
                result => return result,
            }
        }
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn reunite(self, other: OwnedWriteHalf) -> Result<TcpStream, ReuniteError> {
        if other
            .inner
            .as_ref()
            .is_some_and(|write| Arc::ptr_eq(&self.inner, write))
        {
            let mut other = other;
            let _ = other.inner.take();
            Ok(TcpStream { inner: self.inner })
        } else {
            Err(ReuniteError(self, other))
        }
    }
}

impl AsyncRead for OwnedReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        poll_tcp_read(&self.inner, cx, buf)
    }
}

impl AsRawFd for OwnedReadHalf {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for OwnedReadHalf {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

/// Owned write half of a [`TcpStream`].
pub struct OwnedWriteHalf {
    inner: Option<Arc<AsyncFd<std::net::TcpStream>>>,
}

impl OwnedWriteHalf {
    fn inner(&self) -> &Arc<AsyncFd<std::net::TcpStream>> {
        self.inner.as_ref().expect("owned TCP write half missing")
    }

    pub async fn writable(&self) -> io::Result<()> {
        let _ = self.inner().writable().await?;
        Ok(())
    }

    pub async fn ready(&self, interest: Interest) -> io::Result<Ready> {
        let guard = self.inner().ready(interest).await?;
        Ok(guard.ready())
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

    /// Drops the half without shutting down the write direction.
    pub fn forget(mut self) {
        self.inner.take();
    }
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        poll_tcp_write(self.inner(), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.inner().get_ref().shutdown(Shutdown::Write))
    }
}

impl AsRawFd for OwnedWriteHalf {
    fn as_raw_fd(&self) -> RawFd {
        self.inner().as_raw_fd()
    }
}

impl AsFd for OwnedWriteHalf {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner().as_fd()
    }
}

impl Drop for OwnedWriteHalf {
    fn drop(&mut self) {
        if let Some(inner) = &self.inner {
            let _ = inner.get_ref().shutdown(Shutdown::Write);
        }
    }
}

/// Error returned when owned halves from different TCP streams are reunited.
pub struct ReuniteError(pub OwnedReadHalf, pub OwnedWriteHalf);

impl std::fmt::Debug for ReuniteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReuniteError").finish_non_exhaustive()
    }
}

impl std::fmt::Display for ReuniteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tried to reunite TCP halves from different streams")
    }
}

impl std::error::Error for ReuniteError {}

fn poll_tcp_read(
    inner: &AsyncFd<std::net::TcpStream>,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
) -> Poll<io::Result<()>> {
    loop {
        let mut stream = inner.get_ref();
        match stream.read(buf.initialize_unfilled()) {
            Ok(amount) => {
                buf.advance(amount);
                return Poll::Ready(Ok(()));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut guard = match inner.poll_read_ready(cx) {
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

fn poll_tcp_write(
    inner: &AsyncFd<std::net::TcpStream>,
    cx: &mut Context<'_>,
    buf: &[u8],
) -> Poll<io::Result<usize>> {
    loop {
        let mut stream = inner.get_ref();
        match stream.write(buf) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut guard = match inner.poll_write_ready(cx) {
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

/// A non-blocking TCP listener.
pub struct TcpListener {
    inner: Arc<AsyncFd<std::net::TcpListener>>,
}

impl TcpListener {
    /// Binds a listener to `addr`.
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs,
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

    /// Deregisters and returns the standard non-blocking listener.
    pub fn into_std(self) -> io::Result<std::net::TcpListener> {
        Arc::try_unwrap(self.inner)
            .map(AsyncFd::into_inner)
            .map_err(|_| io::Error::other("TCP listener is still shared"))
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

    pub fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        loop {
            match self.try_accept() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut guard = match self.inner.poll_read_ready(cx) {
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

    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.get_ref().ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_ttl(ttl)
    }
}

impl AsRawFd for TcpListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for TcpListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
    }
}

/// TCP-specific split types, matching Tokio's module layout.
pub mod tcp {
    pub use super::{
        OwnedReadHalf, OwnedWriteHalf, ReuniteError, TcpListener, TcpReadHalf as ReadHalf,
        TcpSocket, TcpStream, TcpWriteHalf as WriteHalf,
    };
}

/// A non-blocking UDP socket.
pub struct UdpSocket {
    inner: Arc<AsyncFd<std::net::UdpSocket>>,
}

impl UdpSocket {
    /// Binds a UDP socket to `addr`.
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs,
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

    /// Deregisters and returns the standard non-blocking socket.
    pub fn into_std(self) -> io::Result<std::net::UdpSocket> {
        Arc::try_unwrap(self.inner)
            .map(AsyncFd::into_inner)
            .map_err(|_| io::Error::other("UDP socket is still shared"))
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
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

    /// Connects this UDP socket to a remote address.
    pub async fn connect<A>(&self, addr: A) -> io::Result<()>
    where
        A: ToSocketAddrs,
    {
        let address = resolve(addr)
            .await?
            .into_iter()
            .next()
            .ok_or_else(no_addresses)?;
        self.inner.get_ref().connect(address)
    }

    /// Receives one datagram.
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        poll_fn(|cx| self.poll_recv_from(cx, buf)).await
    }

    /// Sends one datagram.
    pub async fn send_to<A>(&self, data: &[u8], addr: A) -> io::Result<usize>
    where
        A: ToSocketAddrs,
    {
        let address = resolve(addr)
            .await?
            .into_iter()
            .next()
            .ok_or_else(no_addresses)?;
        poll_fn(|cx| self.poll_send_to(cx, data, address)).await
    }

    /// Receives bytes from a connected peer.
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_recv_slice(cx, buf)).await
    }

    /// Sends bytes to a connected peer.
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_send(cx, buf)).await
    }

    pub fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, SocketAddr)>> {
        self.poll_io(cx, Interest::READABLE, || {
            self.inner.get_ref().recv_from(buf)
        })
    }

    pub fn poll_send_to(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
        addr: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        self.poll_io(cx, Interest::WRITABLE, || {
            self.inner.get_ref().send_to(buf, addr)
        })
    }

    fn poll_recv_slice(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.poll_io(cx, Interest::READABLE, || self.inner.get_ref().recv(buf))
    }

    pub fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.poll_io(cx, Interest::WRITABLE, || self.inner.get_ref().send(buf))
    }

    pub fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.poll_recv_slice(cx, buf.initialize_unfilled()) {
            Poll::Ready(Ok(amount)) => {
                buf.advance(amount);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.get_ref().send(buf)
    }

    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.get_ref().recv(buf)
    }

    pub fn try_send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.inner.get_ref().send_to(buf, target)
    }

    pub fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.get_ref().recv_from(buf)
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.async_io(Interest::READABLE, || self.inner.get_ref().peek(buf))
            .await
    }

    pub fn poll_peek(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.poll_io(cx, Interest::READABLE, || {
            self.inner.get_ref().peek(buf.initialize_unfilled())
        }) {
            Poll::Ready(Ok(amount)) => {
                buf.advance(amount);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn try_peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.get_ref().peek(buf)
    }

    pub async fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.async_io(Interest::READABLE, || self.inner.get_ref().peek_from(buf))
            .await
    }

    pub fn poll_peek_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<SocketAddr>> {
        match self.poll_io(cx, Interest::READABLE, || {
            self.inner.get_ref().peek_from(buf.initialize_unfilled())
        }) {
            Poll::Ready(Ok((amount, address))) => {
                buf.advance(amount);
                Poll::Ready(Ok(address))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn try_peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.get_ref().peek_from(buf)
    }

    pub async fn peek_sender(&self) -> io::Result<SocketAddr> {
        poll_fn(|cx| self.poll_peek_sender(cx)).await
    }

    pub fn poll_peek_sender(&self, cx: &mut Context<'_>) -> Poll<io::Result<SocketAddr>> {
        self.poll_io(cx, Interest::READABLE, || {
            self.inner
                .get_ref()
                .peek_from(&mut [])
                .map(|(_, address)| address)
        })
    }

    pub fn try_peek_sender(&self) -> io::Result<SocketAddr> {
        self.inner
            .get_ref()
            .peek_from(&mut [])
            .map(|(_, address)| address)
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

    pub fn broadcast(&self) -> io::Result<bool> {
        self.inner.get_ref().broadcast()
    }

    pub fn set_broadcast(&self, on: bool) -> io::Result<()> {
        self.inner.get_ref().set_broadcast(on)
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.inner.get_ref().multicast_loop_v4()
    }

    pub fn set_multicast_loop_v4(&self, on: bool) -> io::Result<()> {
        self.inner.get_ref().set_multicast_loop_v4(on)
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.inner.get_ref().multicast_ttl_v4()
    }

    pub fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_multicast_ttl_v4(ttl)
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.inner.get_ref().multicast_loop_v6()
    }

    pub fn set_multicast_loop_v6(&self, on: bool) -> io::Result<()> {
        self.inner.get_ref().set_multicast_loop_v6(on)
    }

    #[cfg(target_env = "ohos")]
    pub fn tclass_v6(&self) -> io::Result<u32> {
        get_socket_int(self.as_raw_fd(), libc::IPPROTO_IPV6, libc::IPV6_TCLASS)
            .and_then(|value| u32::try_from(value).map_err(io::Error::other))
    }

    #[cfg(target_env = "ohos")]
    pub fn set_tclass_v6(&self, tclass: u32) -> io::Result<()> {
        set_socket_int(
            self.as_raw_fd(),
            libc::IPPROTO_IPV6,
            libc::IPV6_TCLASS,
            tclass,
        )
    }

    #[cfg(target_env = "ohos")]
    pub fn tos_v4(&self) -> io::Result<u32> {
        get_socket_int(self.as_raw_fd(), libc::IPPROTO_IP, libc::IP_TOS)
            .and_then(|value| u32::try_from(value).map_err(io::Error::other))
    }

    #[cfg(target_env = "ohos")]
    pub fn tos(&self) -> io::Result<u32> {
        self.tos_v4()
    }

    #[cfg(target_env = "ohos")]
    pub fn set_tos_v4(&self, tos: u32) -> io::Result<()> {
        set_socket_int(self.as_raw_fd(), libc::IPPROTO_IP, libc::IP_TOS, tos)
    }

    #[cfg(target_env = "ohos")]
    pub fn set_tos(&self, tos: u32) -> io::Result<()> {
        self.set_tos_v4(tos)
    }

    #[cfg(target_env = "ohos")]
    pub fn device(&self) -> io::Result<Option<Vec<u8>>> {
        socket_device(self.as_raw_fd())
    }

    #[cfg(target_env = "ohos")]
    pub fn bind_device(&self, interface: Option<&[u8]>) -> io::Result<()> {
        bind_socket_device(self.as_raw_fd(), interface)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.get_ref().ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_ttl(ttl)
    }

    pub fn join_multicast_v4(
        &self,
        multiaddr: std::net::Ipv4Addr,
        interface: std::net::Ipv4Addr,
    ) -> io::Result<()> {
        self.inner
            .get_ref()
            .join_multicast_v4(&multiaddr, &interface)
    }

    pub fn join_multicast_v6(
        &self,
        multiaddr: &std::net::Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        self.inner.get_ref().join_multicast_v6(multiaddr, interface)
    }

    pub fn leave_multicast_v4(
        &self,
        multiaddr: std::net::Ipv4Addr,
        interface: std::net::Ipv4Addr,
    ) -> io::Result<()> {
        self.inner
            .get_ref()
            .leave_multicast_v4(&multiaddr, &interface)
    }

    pub fn leave_multicast_v6(
        &self,
        multiaddr: &std::net::Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        self.inner
            .get_ref()
            .leave_multicast_v6(multiaddr, interface)
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
                    let mut guard = match ready {
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

impl AsRawFd for UdpSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl AsFd for UdpSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.get_ref().as_fd()
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
    let mut guard = fd.writable().await?;
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

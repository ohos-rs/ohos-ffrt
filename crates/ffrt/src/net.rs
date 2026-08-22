//! Basic networking helpers executed on FFRT worker tasks.
//!
//! These wrappers use the platform's blocking socket APIs. They are provided
//! for compatibility while the FFRT-loop-based reactor is being completed.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
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

/// A TCP stream.
pub struct TcpStream {
    inner: Arc<Mutex<std::net::TcpStream>>,
}

impl TcpStream {
    /// Connects to a remote address.
    pub async fn connect<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let stream = run_blocking(move || std::net::TcpStream::connect(addr)).await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(stream)),
        })
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let guard = self.inner.lock().unwrap();
        guard.local_addr()
    }

    /// Returns the peer address.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let guard = self.inner.lock().unwrap();
        guard.peer_addr()
    }

    /// Writes all bytes and returns the number written.
    pub async fn write(&self, data: Vec<u8>) -> io::Result<usize> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            use std::io::Write;
            guard.write(&data)
        })
        .await
    }

    /// Writes all bytes, returning an error on partial writes.
    pub async fn write_all(&self, data: Vec<u8>) -> io::Result<()> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut guard = inner.lock().unwrap();
            use std::io::Write;
            guard.write_all(&data)
        })
        .await
    }

    /// Reads up to `buf.len()` bytes into `buf` and returns bytes read plus the buffer.
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

    /// Reads all bytes until EOF.
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

    /// Returns a clone of the stream handle.
    pub async fn try_clone(&self) -> io::Result<Self> {
        let inner = self.inner.clone();
        let stream = run_blocking(move || {
            let guard = inner.lock().unwrap();
            guard.try_clone()
        })
        .await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(stream)),
        })
    }
}

/// A TCP listener.
pub struct TcpListener {
    inner: Arc<Mutex<std::net::TcpListener>>,
}

impl TcpListener {
    /// Binds a listener to `addr`.
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let listener = run_blocking(move || std::net::TcpListener::bind(addr)).await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(listener)),
        })
    }

    /// Accepts one connection.
    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let inner = self.inner.clone();
        let (stream, addr) = run_blocking(move || {
            let guard = inner.lock().unwrap();
            guard.accept()
        })
        .await?;
        Ok((
            TcpStream {
                inner: Arc::new(Mutex::new(stream)),
            },
            addr,
        ))
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let guard = self.inner.lock().unwrap();
        guard.local_addr()
    }
}

/// A UDP socket.
pub struct UdpSocket {
    inner: Arc<Mutex<std::net::UdpSocket>>,
}

impl UdpSocket {
    /// Binds a UDP socket to `addr`.
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let socket = run_blocking(move || std::net::UdpSocket::bind(addr)).await?;
        Ok(Self {
            inner: Arc::new(Mutex::new(socket)),
        })
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let guard = self.inner.lock().unwrap();
        guard.local_addr()
    }

    /// Receives one datagram.
    pub async fn recv_from(&self, buf: Vec<u8>) -> io::Result<(usize, SocketAddr, Vec<u8>)> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let guard = inner.lock().unwrap();
            let mut buf = buf;
            let (n, addr) = guard.recv_from(&mut buf)?;
            Ok((n, addr, buf))
        })
        .await
    }

    /// Sends one datagram.
    pub async fn send_to<A>(&self, data: Vec<u8>, addr: A) -> io::Result<usize>
    where
        A: ToSocketAddrs + Send + 'static,
    {
        let inner = self.inner.clone();
        run_blocking(move || {
            let guard = inner.lock().unwrap();
            guard.send_to(&data, addr)
        })
        .await
    }
}

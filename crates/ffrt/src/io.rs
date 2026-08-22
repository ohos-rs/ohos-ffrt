//! Core asynchronous I/O traits for FFRT-based transports.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A buffer passed to [`AsyncRead`] implementations.
pub struct ReadBuf<'a> {
    buf: &'a mut [u8],
    filled: usize,
}

impl<'a> ReadBuf<'a> {
    /// Creates a buffer from a byte slice.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, filled: 0 }
    }

    /// Returns the number of bytes that have been filled.
    pub fn filled(&self) -> &[u8] {
        &self.buf[..self.filled]
    }

    /// Returns the part of the buffer that is still empty.
    pub fn unfilled(&self) -> &[u8] {
        &self.buf[self.filled..]
    }

    /// Returns a mutable view of the unfilled region.
    pub fn unfilled_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.filled..]
    }

    /// Appends bytes to the filled region.
    pub fn put_slice(&mut self, bytes: &[u8]) {
        let dst = &mut self.buf[self.filled..];
        let len = dst.len().min(bytes.len());
        dst[..len].copy_from_slice(&bytes[..len]);
        self.filled += len;
    }

    /// Advances the filled pointer by `n` bytes.
    pub fn advance(&mut self, n: usize) {
        self.filled = self.filled.saturating_add(n);
    }

    /// Returns the number of bytes filled.
    pub fn len(&self) -> usize {
        self.filled
    }

    /// Returns `true` when the buffer is full.
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }
}

/// FFRT-compatible asynchronous reader.
pub trait AsyncRead {
    /// Polls for more data, appending it to `buf`.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>>;
}

/// FFRT-compatible asynchronous writer.
pub trait AsyncWrite {
    /// Polls to write bytes from `buf`.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>>;

    /// Polls to flush pending writes.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;

    /// Polls to shut down the writer.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

impl AsyncRead for &[u8] {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let reader = &mut *self;
        let src = *reader;
        let n = src.len().min(buf.unfilled().len());
        buf.put_slice(&src[..n]);
        *reader = &src[n..];
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Vec<u8> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Extension methods for [`AsyncRead`].
pub trait AsyncReadExt: AsyncRead {
    /// Reads bytes into `buf`.
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Read<'a, Self>
    where
        Self: Unpin + Sized,
    {
        Read { reader: self, buf }
    }
}

impl<T: AsyncRead> AsyncReadExt for T {}

/// Future returned by [`AsyncReadExt::read`].
#[must_use = "futures do nothing unless polled"]
pub struct Read<'a, R> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<R> Future for Read<'_, R>
where
    R: AsyncRead + Unpin,
{
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let mut read_buf = ReadBuf::new(this.buf);
        match Pin::new(&mut *this.reader).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.len())),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Extension methods for [`AsyncWrite`].
pub trait AsyncWriteExt: AsyncWrite {
    /// Writes bytes from `buf`.
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Write<'a, Self>
    where
        Self: Unpin + Sized,
    {
        Write { writer: self, buf }
    }

    /// Writes all bytes from `buf`.
    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> WriteAll<'a, Self>
    where
        Self: Unpin + Sized,
    {
        WriteAll { writer: self, buf }
    }

    /// Flushes the writer.
    fn flush(&mut self) -> Flush<'_, Self>
    where
        Self: Unpin + Sized,
    {
        Flush { writer: self }
    }

    /// Shuts down the writer.
    fn shutdown(&mut self) -> Shutdown<'_, Self>
    where
        Self: Unpin + Sized,
    {
        Shutdown { writer: self }
    }
}

impl<T: AsyncWrite> AsyncWriteExt for T {}

/// Future returned by [`AsyncWriteExt::write`].
#[must_use = "futures do nothing unless polled"]
pub struct Write<'a, W> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W> Future for Write<'_, W>
where
    W: AsyncWrite + Unpin,
{
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_write(cx, this.buf)
    }
}

/// Future returned by [`AsyncWriteExt::write_all`].
#[must_use = "futures do nothing unless polled"]
pub struct WriteAll<'a, W> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W> Future for WriteAll<'_, W>
where
    W: AsyncWrite + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while !this.buf.is_empty() {
            match Pin::new(&mut *this.writer).poll_write(cx, this.buf) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    )));
                }
                Poll::Ready(Ok(n)) => this.buf = &this.buf[n..],
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Ok(()))
    }
}

/// Future returned by [`AsyncWriteExt::flush`].
#[must_use = "futures do nothing unless polled"]
pub struct Flush<'a, W> {
    writer: &'a mut W,
}

impl<W> Future for Flush<'_, W>
where
    W: AsyncWrite + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_flush(cx)
    }
}

/// Future returned by [`AsyncWriteExt::shutdown`].
#[must_use = "futures do nothing unless polled"]
pub struct Shutdown<'a, W> {
    writer: &'a mut W,
}

impl<W> Future for Shutdown<'_, W>
where
    W: AsyncWrite + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_shutdown(cx)
    }
}

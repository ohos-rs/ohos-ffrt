//! Core asynchronous I/O traits, adapters, and utilities.

use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use crate::lock::Mutex as FfrtMutex;

/// Unix readiness types backed by the FFRT reactor.
#[cfg(unix)]
pub mod unix {
    pub use crate::reactor::{
        AsyncFd, AsyncFdReady, AsyncFdReadyGuard, Interest, Ready, TryIoError,
    };
}

/// A buffer passed to [`AsyncRead`] implementations.
pub struct ReadBuf<'a> {
    buf: &'a mut [u8],
    filled: usize,
}

impl<'a> ReadBuf<'a> {
    /// Creates a buffer from an initialized byte slice.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, filled: 0 }
    }

    /// Returns the filled portion.
    pub fn filled(&self) -> &[u8] {
        &self.buf[..self.filled]
    }

    /// Returns a mutable filled portion.
    pub fn filled_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.filled]
    }

    /// Returns the unfilled portion.
    pub fn unfilled(&self) -> &[u8] {
        &self.buf[self.filled..]
    }

    /// Returns a mutable unfilled portion.
    pub fn unfilled_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.filled..]
    }

    /// Returns the total capacity.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Returns the remaining capacity.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.filled
    }

    /// Appends bytes to the filled region.
    pub fn put_slice(&mut self, bytes: &[u8]) {
        assert!(
            bytes.len() <= self.remaining(),
            "source does not fit in ReadBuf"
        );
        let end = self.filled + bytes.len();
        self.buf[self.filled..end].copy_from_slice(bytes);
        self.filled = end;
    }

    /// Advances the filled pointer by `n` bytes.
    pub fn advance(&mut self, n: usize) {
        assert!(n <= self.remaining(), "ReadBuf advanced past capacity");
        self.filled += n;
    }

    /// Returns the number of filled bytes.
    pub fn len(&self) -> usize {
        self.filled
    }

    /// Returns whether no bytes have been filled.
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Clears the filled portion without modifying the bytes.
    pub fn clear(&mut self) {
        self.filled = 0;
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

/// FFRT-compatible asynchronous buffered reader.
pub trait AsyncBufRead: AsyncRead {
    /// Returns buffered data, filling the internal buffer if necessary.
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>>;

    /// Marks `amount` bytes as consumed.
    fn consume(self: Pin<&mut Self>, amount: usize);
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
        let n = reader.len().min(buf.remaining());
        buf.put_slice(&reader[..n]);
        *reader = &reader[n..];
        Poll::Ready(Ok(()))
    }
}

impl AsyncBufRead for &[u8] {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        Poll::Ready(Ok(*self.get_mut()))
    }

    fn consume(mut self: Pin<&mut Self>, amount: usize) {
        let amount = amount.min(self.len());
        *self = &self[amount..];
    }
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for &mut T {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for &mut T {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(cx)
    }
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for Box<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for Box<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(cx)
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

    /// Reads exactly enough bytes to fill `buf`.
    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadExact<'a, Self>
    where
        Self: Unpin + Sized,
    {
        ReadExact {
            reader: self,
            buf,
            read: 0,
        }
    }

    /// Reads all bytes until EOF and appends them to `buf`.
    fn read_to_end<'a>(&'a mut self, buf: &'a mut Vec<u8>) -> ReadToEnd<'a, Self>
    where
        Self: Unpin + Sized,
    {
        ReadToEnd {
            reader: self,
            buf,
            read: 0,
        }
    }

    /// Reads UTF-8 bytes until EOF and appends them to `string`.
    fn read_to_string<'a>(&'a mut self, string: &'a mut String) -> ReadToString<'a, Self>
    where
        Self: Unpin + Sized,
    {
        ReadToString {
            reader: self,
            string,
            bytes: Vec::new(),
            done: false,
        }
    }
}

impl<T: AsyncRead + ?Sized> AsyncReadExt for T {}

/// Future returned by [`AsyncReadExt::read`].
#[must_use = "futures do nothing unless polled"]
pub struct Read<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<R: AsyncRead + Unpin + ?Sized> Future for Read<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut read_buf = ReadBuf::new(this.buf);
        ready!(Pin::new(&mut *this.reader).poll_read(cx, &mut read_buf))?;
        Poll::Ready(Ok(read_buf.len()))
    }
}

/// Future returned by [`AsyncReadExt::read_exact`].
#[must_use = "futures do nothing unless polled"]
pub struct ReadExact<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
    read: usize,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadExact<'_, R> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while this.read < this.buf.len() {
            let mut read_buf = ReadBuf::new(&mut this.buf[this.read..]);
            ready!(Pin::new(&mut *this.reader).poll_read(cx, &mut read_buf))?;
            let n = read_buf.len();
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "early EOF while filling buffer",
                )));
            }
            this.read += n;
        }
        Poll::Ready(Ok(()))
    }
}

/// Future returned by [`AsyncReadExt::read_to_end`].
#[must_use = "futures do nothing unless polled"]
pub struct ReadToEnd<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut Vec<u8>,
    read: usize,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadToEnd<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            let mut chunk = [0; 8 * 1024];
            let mut read_buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut *this.reader).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    let n = read_buf.len();
                    if n == 0 {
                        return Poll::Ready(Ok(this.read));
                    }
                    this.buf.extend_from_slice(read_buf.filled());
                    this.read += n;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Future returned by [`AsyncReadExt::read_to_string`].
#[must_use = "futures do nothing unless polled"]
pub struct ReadToString<'a, R: ?Sized> {
    reader: &'a mut R,
    string: &'a mut String,
    bytes: Vec<u8>,
    done: bool,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadToString<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.done, "ReadToString polled after completion");
        loop {
            let mut chunk = [0; 8 * 1024];
            let mut read_buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut *this.reader).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) if read_buf.is_empty() => {
                    this.done = true;
                    let bytes = std::mem::take(&mut this.bytes);
                    let length = bytes.len();
                    let value = String::from_utf8(bytes).map_err(|error| {
                        io::Error::new(io::ErrorKind::InvalidData, error.utf8_error())
                    })?;
                    this.string.push_str(&value);
                    return Poll::Ready(Ok(length));
                }
                Poll::Ready(Ok(())) => this.bytes.extend_from_slice(read_buf.filled()),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Extension methods for [`AsyncBufRead`].
pub trait AsyncBufReadExt: AsyncBufRead {
    /// Reads bytes through and including `delimiter`.
    fn read_until<'a>(&'a mut self, delimiter: u8, buf: &'a mut Vec<u8>) -> ReadUntil<'a, Self>
    where
        Self: Unpin + Sized,
    {
        ReadUntil {
            reader: self,
            delimiter,
            buf,
            read: 0,
        }
    }

    /// Reads one UTF-8 line, retaining its newline.
    fn read_line<'a>(&'a mut self, line: &'a mut String) -> ReadLine<'a, Self>
    where
        Self: Unpin + Sized,
    {
        ReadLine {
            reader: self,
            line,
            bytes: Vec::new(),
            done: false,
        }
    }
}

impl<T: AsyncBufRead + ?Sized> AsyncBufReadExt for T {}

/// Future returned by [`AsyncBufReadExt::read_until`].
#[must_use = "futures do nothing unless polled"]
pub struct ReadUntil<'a, R: ?Sized> {
    reader: &'a mut R,
    delimiter: u8,
    buf: &'a mut Vec<u8>,
    read: usize,
}

impl<R: AsyncBufRead + Unpin + ?Sized> Future for ReadUntil<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            let (used, done) = {
                let available = ready!(Pin::new(&mut *this.reader).poll_fill_buf(cx))?;
                if available.is_empty() {
                    return Poll::Ready(Ok(this.read));
                }
                let used = available
                    .iter()
                    .position(|byte| *byte == this.delimiter)
                    .map_or(available.len(), |index| index + 1);
                this.buf.extend_from_slice(&available[..used]);
                (used, available[used - 1] == this.delimiter)
            };
            Pin::new(&mut *this.reader).consume(used);
            this.read += used;
            if done {
                return Poll::Ready(Ok(this.read));
            }
        }
    }
}

/// Future returned by [`AsyncBufReadExt::read_line`].
#[must_use = "futures do nothing unless polled"]
pub struct ReadLine<'a, R: ?Sized> {
    reader: &'a mut R,
    line: &'a mut String,
    bytes: Vec<u8>,
    done: bool,
}

impl<R: AsyncBufRead + Unpin + ?Sized> Future for ReadLine<'_, R> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.done, "ReadLine polled after completion");
        loop {
            let (used, done) = {
                let available = ready!(Pin::new(&mut *this.reader).poll_fill_buf(cx))?;
                if available.is_empty() {
                    (0, true)
                } else {
                    let used = available
                        .iter()
                        .position(|byte| *byte == b'\n')
                        .map_or(available.len(), |index| index + 1);
                    this.bytes.extend_from_slice(&available[..used]);
                    (used, available[used - 1] == b'\n')
                }
            };
            Pin::new(&mut *this.reader).consume(used);
            if done {
                this.done = true;
                let bytes = std::mem::take(&mut this.bytes);
                let length = bytes.len();
                let value = String::from_utf8(bytes).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.utf8_error())
                })?;
                this.line.push_str(&value);
                return Poll::Ready(Ok(length));
            }
        }
    }
}

/// Extension methods for [`AsyncWrite`].
pub trait AsyncWriteExt: AsyncWrite {
    /// Writes some bytes.
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Write<'a, Self>
    where
        Self: Unpin + Sized,
    {
        Write { writer: self, buf }
    }

    /// Writes all bytes.
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

impl<T: AsyncWrite + ?Sized> AsyncWriteExt for T {}

/// Future returned by [`AsyncWriteExt::write`].
#[must_use = "futures do nothing unless polled"]
pub struct Write<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for Write<'_, W> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_write(cx, this.buf)
    }
}

/// Future returned by [`AsyncWriteExt::write_all`].
#[must_use = "futures do nothing unless polled"]
pub struct WriteAll<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteAll<'_, W> {
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
pub struct Flush<'a, W: ?Sized> {
    writer: &'a mut W,
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for Flush<'_, W> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().writer).poll_flush(cx)
    }
}

/// Future returned by [`AsyncWriteExt::shutdown`].
#[must_use = "futures do nothing unless polled"]
pub struct Shutdown<'a, W: ?Sized> {
    writer: &'a mut W,
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for Shutdown<'_, W> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().writer).poll_shutdown(cx)
    }
}

/// A buffered asynchronous reader.
pub struct BufReader<R> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    cap: usize,
}

impl<R> BufReader<R> {
    /// Creates a reader with an 8 KiB buffer.
    pub fn new(inner: R) -> Self {
        Self::with_capacity(8 * 1024, inner)
    }

    /// Creates a reader with a specific buffer capacity.
    pub fn with_capacity(capacity: usize, inner: R) -> Self {
        Self {
            inner,
            buf: vec![0; capacity.max(1)],
            pos: 0,
            cap: 0,
        }
    }

    /// Returns a shared reference to the inner reader.
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Returns a mutable reference to the inner reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Returns currently buffered data.
    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.pos..self.cap]
    }

    /// Returns the buffer capacity.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Consumes the adapter and returns the inner reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: fmt::Debug> fmt::Debug for BufReader<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufReader")
            .field("inner", &self.inner)
            .field("buffered", &self.buffer().len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

impl<R: AsyncRead + Unpin> AsyncBufRead for BufReader<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.pos >= this.cap {
            let mut read_buf = ReadBuf::new(&mut this.buf);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut read_buf))?;
            this.pos = 0;
            this.cap = read_buf.len();
        }
        Poll::Ready(Ok(&this.buf[this.pos..this.cap]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        this.pos = (this.pos + amount).min(this.cap);
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for BufReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.pos == this.cap && output.remaining() >= this.buf.len() {
            return Pin::new(&mut this.inner).poll_read(cx, output);
        }

        if this.pos == this.cap {
            let mut internal = ReadBuf::new(&mut this.buf);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut internal))?;
            this.pos = 0;
            this.cap = internal.len();
        }

        let amount = output.remaining().min(this.cap - this.pos);
        output.put_slice(&this.buf[this.pos..this.pos + amount]);
        this.pos += amount;
        Poll::Ready(Ok(()))
    }
}

/// A buffered asynchronous writer.
pub struct BufWriter<W> {
    inner: W,
    buf: Vec<u8>,
    written: usize,
    capacity: usize,
}

impl<W> BufWriter<W> {
    /// Creates a writer with an 8 KiB buffer.
    pub fn new(inner: W) -> Self {
        Self::with_capacity(8 * 1024, inner)
    }

    /// Creates a writer with a specific buffer capacity.
    pub fn with_capacity(capacity: usize, inner: W) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner,
            buf: Vec::with_capacity(capacity),
            written: 0,
            capacity,
        }
    }

    /// Returns a shared reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Returns a mutable reference to the inner writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Returns the bytes waiting to be written.
    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.written..]
    }

    /// Returns the configured capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Consumes the adapter. Call `flush` first to avoid discarding data.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: fmt::Debug> fmt::Debug for BufWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufWriter")
            .field("inner", &self.inner)
            .field("buffered", &self.buffer().len())
            .field("capacity", &self.capacity)
            .finish()
    }
}

impl<W: AsyncWrite + Unpin> BufWriter<W> {
    fn poll_flush_buf(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.written < self.buf.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &self.buf[self.written..]) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to flush buffered writer",
                    )));
                }
                Poll::Ready(Ok(amount)) => self.written += amount,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.buf.clear();
        self.written = 0;
        Poll::Ready(Ok(()))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for BufWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if !this.buf.is_empty() && input.len() > this.capacity - this.buf.len() {
            ready!(this.poll_flush_buf(cx))?;
        }
        if this.buf.is_empty() && input.len() >= this.capacity {
            return Pin::new(&mut this.inner).poll_write(cx, input);
        }
        let amount = input.len().min(this.capacity - this.buf.len());
        this.buf.extend_from_slice(&input[..amount]);
        Poll::Ready(Ok(amount))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_flush_buf(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_flush_buf(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// Copies all bytes from `reader` to `writer`, returning the byte count.
pub fn copy<'a, R, W>(reader: &'a mut R, writer: &'a mut W) -> Copy<'a, R, W>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    Copy {
        reader,
        writer,
        buf: [0; 8 * 1024],
        pos: 0,
        cap: 0,
        amount: 0,
        eof: false,
    }
}

/// Future returned by [`copy`].
#[must_use = "futures do nothing unless polled"]
pub struct Copy<'a, R: ?Sized, W: ?Sized> {
    reader: &'a mut R,
    writer: &'a mut W,
    buf: [u8; 8 * 1024],
    pos: usize,
    cap: usize,
    amount: u64,
    eof: bool,
}

impl<R, W> Future for Copy<'_, R, W>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    type Output = io::Result<u64>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            if this.pos == this.cap && !this.eof {
                let mut read_buf = ReadBuf::new(&mut this.buf);
                ready!(Pin::new(&mut *this.reader).poll_read(cx, &mut read_buf))?;
                this.pos = 0;
                this.cap = read_buf.len();
                this.eof = this.cap == 0;
            }

            while this.pos < this.cap {
                let written = ready!(
                    Pin::new(&mut *this.writer).poll_write(cx, &this.buf[this.pos..this.cap])
                )?;
                if written == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to copy entire stream",
                    )));
                }
                this.pos += written;
                this.amount += written as u64;
            }

            if this.eof {
                ready!(Pin::new(&mut *this.writer).poll_flush(cx))?;
                return Poll::Ready(Ok(this.amount));
            }
        }
    }
}

struct Split<T> {
    io: FfrtMutex<T>,
}

/// The read half returned by [`split`].
pub struct ReadHalf<T> {
    inner: Arc<Split<T>>,
}

/// The write half returned by [`split`].
pub struct WriteHalf<T> {
    inner: Arc<Split<T>>,
}

/// Splits a duplex I/O object into independently owned read and write halves.
pub fn split<T>(io: T) -> (ReadHalf<T>, WriteHalf<T>)
where
    T: AsyncRead + AsyncWrite,
{
    let inner = Arc::new(Split {
        io: FfrtMutex::new(io),
    });
    (
        ReadHalf {
            inner: inner.clone(),
        },
        WriteHalf { inner },
    )
}

impl<T> ReadHalf<T> {
    /// Returns whether `write` is the matching half of this split object.
    pub fn is_pair_of(&self, write: &WriteHalf<T>) -> bool {
        Arc::ptr_eq(&self.inner, &write.inner)
    }

    /// Reunites matching halves and returns the original I/O object.
    ///
    /// # Panics
    ///
    /// Panics if the halves originated from different calls to [`split`].
    pub fn unsplit(self, write: WriteHalf<T>) -> T {
        assert!(self.is_pair_of(&write), "split halves are unrelated");
        drop(write);
        Arc::try_unwrap(self.inner)
            .ok()
            .expect("split object still has unexpected owners")
            .io
            .into_inner()
    }
}

impl<T> AsyncRead for ReadHalf<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut io = self.inner.io.lock().unwrap();
        Pin::new(&mut *io).poll_read(cx, buf)
    }
}

impl<T> AsyncWrite for WriteHalf<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut io = self.inner.io.lock().unwrap();
        Pin::new(&mut *io).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut io = self.inner.io.lock().unwrap();
        Pin::new(&mut *io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut io = self.inner.io.lock().unwrap();
        Pin::new(&mut *io).poll_shutdown(cx)
    }
}

impl<T> fmt::Debug for ReadHalf<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadHalf").finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for WriteHalf<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteHalf").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_combinators() {
        let result = crate::Runtime::new().block_on(async move {
            let mut data: &[u8] = b"hello";
            let mut exact = [0; 2];
            data.read_exact(&mut exact).await?;
            assert_eq!(&exact, b"he");

            let mut tail = Vec::new();
            assert_eq!(data.read_to_end(&mut tail).await?, 3);
            assert_eq!(&tail, b"llo");
            Ok::<(), io::Error>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn copy_and_buffers() {
        let result = crate::Runtime::new().block_on(async move {
            let mut reader = BufReader::with_capacity(2, &b"hello"[..]);
            let mut writer = BufWriter::with_capacity(2, Vec::new());
            assert_eq!(copy(&mut reader, &mut writer).await?, 5);
            writer.flush().await?;
            assert_eq!(writer.get_ref(), b"hello");
            Ok::<(), io::Error>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn buffered_lines() {
        let result = crate::Runtime::new().block_on(async move {
            let mut reader = BufReader::with_capacity(3, &b"one\ntwo"[..]);
            let mut line = String::new();
            assert_eq!(reader.read_line(&mut line).await?, 4);
            assert_eq!(line, "one\n");
            Ok::<(), io::Error>(())
        });
        assert!(result.is_ok());
    }
}

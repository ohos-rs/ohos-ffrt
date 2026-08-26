//! Core asynchronous I/O traits, adapters, and utilities.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::io::{self, IoSlice};
use std::mem::MaybeUninit;
use std::ops::DerefMut;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll, Waker, ready};

use bytes::{Buf, BufMut};

use crate::lock::Mutex as FfrtMutex;

pub use std::io::{Error, ErrorKind, Result, SeekFrom};

#[cfg(target_env = "ohos")]
pub use crate::reactor::{Interest, Ready};

macro_rules! read_number_method {
    ($name:ident, $ty:ty, $size:expr, $convert:expr) => {
        fn $name(&mut self) -> impl Future<Output = io::Result<$ty>> + '_
        where
            Self: Unpin + Sized,
        {
            async move {
                let mut bytes = [0u8; $size];
                self.read_exact(&mut bytes).await?;
                Ok(($convert)(bytes))
            }
        }
    };
}

macro_rules! write_number_method {
    ($name:ident, $ty:ty, $convert:expr) => {
        fn $name(&mut self, value: $ty) -> impl Future<Output = io::Result<()>> + '_
        where
            Self: Unpin + Sized,
        {
            async move { self.write_all(&($convert)(value)).await }
        }
    };
}

/// Unix readiness types backed by the FFRT reactor.
#[cfg(target_env = "ohos")]
pub mod unix {
    pub use crate::reactor::{
        AsyncFd, AsyncFdReady, AsyncFdReadyGuard, AsyncFdReadyMut, AsyncFdReadyMutGuard,
        AsyncFdTryNewError, Interest, Ready, TryIoError,
    };
}

/// A buffer passed to [`AsyncRead`] implementations.
pub struct ReadBuf<'a> {
    buf: &'a mut [MaybeUninit<u8>],
    filled: usize,
    initialized: usize,
}

impl<'a> ReadBuf<'a> {
    /// Creates a buffer from an initialized byte slice.
    pub fn new(buf: &'a mut [u8]) -> Self {
        let initialized = buf.len();
        let buf = unsafe {
            std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<MaybeUninit<u8>>(), buf.len())
        };
        Self {
            buf,
            filled: 0,
            initialized,
        }
    }

    /// Creates a buffer whose contents may be uninitialized.
    pub fn uninit(buf: &'a mut [MaybeUninit<u8>]) -> Self {
        Self {
            buf,
            filled: 0,
            initialized: 0,
        }
    }

    /// Returns the filled portion.
    pub fn filled(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), self.filled) }
    }

    /// Returns a mutable filled portion.
    pub fn filled_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.buf.as_mut_ptr().cast::<u8>(), self.filled) }
    }

    /// Returns a new buffer over at most `n` bytes of the unfilled region.
    pub fn take(&mut self, n: usize) -> ReadBuf<'_> {
        let amount = n.min(self.remaining());
        let mut taken = ReadBuf::uninit(&mut self.buf[self.filled..self.filled + amount]);
        unsafe {
            taken.assume_init(self.initialized.saturating_sub(self.filled).min(amount));
        }
        taken
    }

    /// Returns the initialized region, including filled bytes.
    pub fn initialized(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), self.initialized) }
    }

    /// Returns the mutable initialized region, including filled bytes.
    pub fn initialized_mut(&mut self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(self.buf.as_mut_ptr().cast::<u8>(), self.initialized)
        }
    }

    /// Returns the entire underlying buffer without initializing it.
    ///
    /// # Safety
    ///
    /// The caller must not de-initialize bytes in the region reported by
    /// [`ReadBuf::initialized`].
    pub unsafe fn inner_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        self.buf
    }

    /// Returns the unfilled region without initializing it.
    ///
    /// # Safety
    ///
    /// The caller must not de-initialize bytes in the region reported by
    /// [`ReadBuf::initialized`].
    pub unsafe fn unfilled_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        &mut self.buf[self.filled..]
    }

    /// Initializes and returns the entire unfilled region.
    pub fn initialize_unfilled(&mut self) -> &mut [u8] {
        self.initialize_unfilled_to(self.remaining())
    }

    /// Initializes and returns the first `n` unfilled bytes.
    pub fn initialize_unfilled_to(&mut self, n: usize) -> &mut [u8] {
        assert!(n <= self.remaining(), "n overflows remaining");
        let end = self.filled + n;
        if self.initialized < end {
            self.buf[self.initialized..end].fill(MaybeUninit::new(0));
            self.initialized = end;
        }
        unsafe {
            std::slice::from_raw_parts_mut(self.buf.as_mut_ptr().add(self.filled).cast::<u8>(), n)
        }
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
        self.initialize_unfilled_to(bytes.len())
            .copy_from_slice(bytes);
        self.filled += bytes.len();
    }

    /// Advances the filled pointer by `n` bytes.
    pub fn advance(&mut self, n: usize) {
        let filled = self.filled.checked_add(n).expect("filled overflow");
        self.set_filled(filled);
    }

    /// Sets the size of the filled region.
    pub fn set_filled(&mut self, n: usize) {
        assert!(
            n <= self.initialized,
            "filled must not become larger than initialized"
        );
        self.filled = n;
    }

    /// Marks the first `n` unfilled bytes as initialized.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the first `n` bytes after the filled region
    /// have been fully initialized.
    pub unsafe fn assume_init(&mut self, n: usize) {
        let initialized = self.filled.checked_add(n).expect("initialized overflow");
        assert!(initialized <= self.capacity(), "initialized past capacity");
        self.initialized = self.initialized.max(initialized);
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

    /// Polls to write multiple buffers.
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let buffer = bufs
            .iter()
            .find(|buffer| !buffer.is_empty())
            .map_or(&[][..], |buffer| &buffer[..]);
        self.poll_write(cx, buffer)
    }

    /// Returns whether vectored writes are implemented efficiently.
    fn is_write_vectored(&self) -> bool {
        false
    }

    /// Polls to flush pending writes.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;

    /// Polls to shut down the writer.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

/// FFRT-compatible asynchronous seeker.
pub trait AsyncSeek {
    /// Starts a seek operation.
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()>;

    /// Polls a previously started seek operation to completion.
    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>>;
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

impl<T: AsyncBufRead + Unpin + ?Sized> AsyncBufRead for &mut T {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        Pin::new(&mut **self.get_mut()).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        Pin::new(&mut **self.get_mut()).consume(amount)
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

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        (**self).is_write_vectored()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(cx)
    }
}

impl<T: AsyncSeek + Unpin + ?Sized> AsyncSeek for &mut T {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut **self.get_mut()).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut **self.get_mut()).poll_complete(cx)
    }
}

impl<P> AsyncRead for Pin<P>
where
    P: DerefMut,
    P::Target: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_read(cx, buf)
    }
}

impl<P> AsyncBufRead for Pin<P>
where
    P: DerefMut,
    P::Target: AsyncBufRead,
{
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        unsafe { self.get_unchecked_mut().as_mut() }.consume(amount)
    }
}

impl<P> AsyncWrite for Pin<P>
where
    P: DerefMut,
    P::Target: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        (**self).is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_shutdown(cx)
    }
}

impl<P> AsyncSeek for Pin<P>
where
    P: DerefMut,
    P::Target: AsyncSeek,
{
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        unsafe { self.get_unchecked_mut().as_mut() }.start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        unsafe { self.get_unchecked_mut().as_mut() }.poll_complete(cx)
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

impl<T: AsyncBufRead + Unpin + ?Sized> AsyncBufRead for Box<T> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        Pin::new(&mut **self.get_mut()).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        Pin::new(&mut **self.get_mut()).consume(amount)
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

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        (**self).is_write_vectored()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(cx)
    }
}

impl<T: AsyncSeek + Unpin + ?Sized> AsyncSeek for Box<T> {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut **self.get_mut()).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut **self.get_mut()).poll_complete(cx)
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

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(std::io::Write::write_vectored(&mut *self, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncRead for io::Cursor<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let position = self.position();
        let bytes = self.get_ref().as_ref();
        if position > bytes.len() as u64 {
            return Poll::Ready(Ok(()));
        }
        let start = position as usize;
        let amount = (bytes.len() - start).min(buf.remaining());
        buf.put_slice(&bytes[start..start + amount]);
        self.set_position((start + amount) as u64);
        Poll::Ready(Ok(()))
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncBufRead for io::Cursor<T> {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        Poll::Ready(std::io::BufRead::fill_buf(self.get_mut()))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        std::io::BufRead::consume(self.get_mut(), amount)
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncSeek for io::Cursor<T> {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        std::io::Seek::seek(&mut *self, position).map(drop)
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(self.position()))
    }
}

impl<T: Unpin> AsyncWrite for io::Cursor<T>
where
    io::Cursor<T>: std::io::Write,
{
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(std::io::Write::write(self.get_mut(), buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(std::io::Write::write_vectored(self.get_mut(), bufs))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(std::io::Write::flush(self.get_mut()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

/// Extension methods for [`AsyncSeek`].
pub trait AsyncSeekExt: AsyncSeek {
    fn seek(&mut self, position: SeekFrom) -> Seek<'_, Self>
    where
        Self: Unpin + Sized,
    {
        Seek {
            seeker: self,
            position: Some(position),
        }
    }

    fn rewind(&mut self) -> Seek<'_, Self>
    where
        Self: Unpin + Sized,
    {
        self.seek(SeekFrom::Start(0))
    }

    fn stream_position(&mut self) -> Seek<'_, Self>
    where
        Self: Unpin + Sized,
    {
        self.seek(SeekFrom::Current(0))
    }
}

impl<T: AsyncSeek + ?Sized> AsyncSeekExt for T {}

/// Future returned by [`AsyncSeekExt::seek`].
#[must_use = "futures do nothing unless polled"]
pub struct Seek<'a, S: ?Sized> {
    seeker: &'a mut S,
    position: Option<SeekFrom>,
}

impl<S: AsyncSeek + Unpin + ?Sized> Future for Seek<'_, S> {
    type Output = io::Result<u64>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(position) = this.position.take() {
            Pin::new(&mut *this.seeker).start_seek(position)?;
        }
        Pin::new(&mut *this.seeker).poll_complete(cx)
    }
}

/// Extension methods for [`AsyncRead`].
pub trait AsyncReadExt: AsyncRead {
    /// Concatenates this reader with another reader.
    fn chain<R>(self, next: R) -> Chain<Self, R>
    where
        Self: Sized,
        R: AsyncRead,
    {
        Chain {
            first: self,
            second: next,
            reading_first: true,
        }
    }

    /// Reads bytes into `buf`.
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Read<'a, Self>
    where
        Self: Unpin + Sized,
    {
        Read { reader: self, buf }
    }

    /// Reads bytes into a [`BufMut`].
    fn read_buf<'a, B>(&'a mut self, buf: &'a mut B) -> impl Future<Output = io::Result<usize>> + 'a
    where
        Self: Unpin + Sized,
        B: BufMut + ?Sized,
    {
        async move {
            let capacity = buf.remaining_mut().min(8 * 1024);
            if capacity == 0 {
                return Ok(0);
            }
            let mut bytes = vec![0; capacity];
            let amount = self.read(&mut bytes).await?;
            buf.put_slice(&bytes[..amount]);
            Ok(amount)
        }
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

    read_number_method!(read_u8, u8, 1, |bytes: [u8; 1]| bytes[0]);
    read_number_method!(read_i8, i8, 1, |bytes: [u8; 1]| bytes[0] as i8);
    read_number_method!(read_u16, u16, 2, u16::from_be_bytes);
    read_number_method!(read_i16, i16, 2, i16::from_be_bytes);
    read_number_method!(read_u32, u32, 4, u32::from_be_bytes);
    read_number_method!(read_i32, i32, 4, i32::from_be_bytes);
    read_number_method!(read_u64, u64, 8, u64::from_be_bytes);
    read_number_method!(read_i64, i64, 8, i64::from_be_bytes);
    read_number_method!(read_u128, u128, 16, u128::from_be_bytes);
    read_number_method!(read_i128, i128, 16, i128::from_be_bytes);
    read_number_method!(read_f32, f32, 4, f32::from_be_bytes);
    read_number_method!(read_f64, f64, 8, f64::from_be_bytes);
    read_number_method!(read_u16_le, u16, 2, u16::from_le_bytes);
    read_number_method!(read_i16_le, i16, 2, i16::from_le_bytes);
    read_number_method!(read_u32_le, u32, 4, u32::from_le_bytes);
    read_number_method!(read_i32_le, i32, 4, i32::from_le_bytes);
    read_number_method!(read_u64_le, u64, 8, u64::from_le_bytes);
    read_number_method!(read_i64_le, i64, 8, i64::from_le_bytes);
    read_number_method!(read_u128_le, u128, 16, u128::from_le_bytes);
    read_number_method!(read_i128_le, i128, 16, i128::from_le_bytes);
    read_number_method!(read_f32_le, f32, 4, f32::from_le_bytes);
    read_number_method!(read_f64_le, f64, 8, f64::from_le_bytes);

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

    /// Limits the number of bytes that may be read from this reader.
    fn take(self, limit: u64) -> Take<Self>
    where
        Self: Sized,
    {
        Take { inner: self, limit }
    }
}

impl<T: AsyncRead + ?Sized> AsyncReadExt for T {}

/// Reader returned by [`AsyncReadExt::chain`].
pub struct Chain<T, U> {
    first: T,
    second: U,
    reading_first: bool,
}

impl<T, U> Chain<T, U> {
    pub fn get_ref(&self) -> (&T, &U) {
        (&self.first, &self.second)
    }

    pub fn get_mut(&mut self) -> (&mut T, &mut U) {
        (&mut self.first, &mut self.second)
    }

    pub fn into_inner(self) -> (T, U) {
        (self.first, self.second)
    }
}

impl<T: AsyncRead + Unpin, U: AsyncRead + Unpin> AsyncRead for Chain<T, U> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.reading_first {
            let before = buf.len();
            match Pin::new(&mut this.first).poll_read(cx, buf) {
                Poll::Ready(Ok(())) if buf.len() == before => this.reading_first = false,
                result => return result,
            }
        }
        Pin::new(&mut this.second).poll_read(cx, buf)
    }
}

/// Reader returned by [`AsyncReadExt::take`].
pub struct Take<T> {
    inner: T,
    limit: u64,
}

impl<T> Take<T> {
    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Take<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.limit == 0 || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let maximum = usize::try_from(this.limit)
            .unwrap_or(usize::MAX)
            .min(buf.remaining());
        let mut limited = ReadBuf::new(&mut buf.initialize_unfilled()[..maximum]);
        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut limited))?;
        let amount = limited.len();
        buf.advance(amount);
        this.limit -= amount as u64;
        Poll::Ready(Ok(()))
    }
}

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
    type Output = io::Result<usize>;

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
        Poll::Ready(Ok(this.buf.len()))
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

    /// Returns the currently buffered bytes, filling the buffer if needed.
    fn fill_buf(&mut self) -> FillBuf<'_, Self>
    where
        Self: Unpin + Sized,
    {
        FillBuf { reader: self }
    }

    /// Consumes bytes from the buffered reader.
    fn consume(&mut self, amount: usize)
    where
        Self: Unpin + Sized,
    {
        Pin::new(self).consume(amount)
    }

    /// Returns a line-oriented adapter.
    fn lines(self) -> Lines<Self>
    where
        Self: Sized,
    {
        Lines { inner: self }
    }

    /// Returns a delimiter-oriented adapter.
    fn split(self, delimiter: u8) -> Split<Self>
    where
        Self: Sized,
    {
        Split {
            inner: self,
            delimiter,
        }
    }
}

impl<T: AsyncBufRead + ?Sized> AsyncBufReadExt for T {}

/// Future returned by [`AsyncBufReadExt::fill_buf`].
#[must_use = "futures do nothing unless polled"]
pub struct FillBuf<'a, R: ?Sized> {
    reader: &'a mut R,
}

impl<'a, R: AsyncBufRead + Unpin + ?Sized> Future for FillBuf<'a, R> {
    type Output = io::Result<&'a [u8]>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let reader = &mut *self.get_mut().reader;
        match Pin::new(reader).poll_fill_buf(cx) {
            Poll::Ready(Ok(buffer)) => {
                // The future holds the exclusive reader borrow for `'a`, so
                // the returned buffer cannot outlive or alias a later poll.
                let buffer = unsafe { std::mem::transmute::<&[u8], &'a [u8]>(buffer) };
                Poll::Ready(Ok(buffer))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Line-oriented buffered reader adapter.
pub struct Lines<R> {
    inner: R,
}

impl<R> Lines<R> {
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncBufRead + Unpin> Lines<R> {
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        if self.inner.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        Ok(Some(line))
    }
}

/// Delimiter-oriented buffered reader adapter.
pub struct Split<R> {
    inner: R,
    delimiter: u8,
}

impl<R> Split<R> {
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncBufRead + Unpin> Split<R> {
    pub async fn next_segment(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut segment = Vec::new();
        if self.inner.read_until(self.delimiter, &mut segment).await? == 0 {
            return Ok(None);
        }
        if segment.last() == Some(&self.delimiter) {
            segment.pop();
        }
        Ok(Some(segment))
    }
}

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

    /// Writes from a slice of buffers.
    fn write_vectored<'a, 'b>(&'a mut self, bufs: &'a [IoSlice<'b>]) -> WriteVectored<'a, 'b, Self>
    where
        Self: Unpin + Sized,
    {
        WriteVectored { writer: self, bufs }
    }

    /// Writes bytes from a [`Buf`].
    fn write_buf<'a, B>(
        &'a mut self,
        src: &'a mut B,
    ) -> impl Future<Output = io::Result<usize>> + 'a
    where
        Self: Unpin + Sized,
        B: Buf + ?Sized,
    {
        async move {
            if !src.has_remaining() {
                return Ok(0);
            }
            let amount = self.write(src.chunk()).await?;
            src.advance(amount);
            Ok(amount)
        }
    }

    /// Writes every remaining byte from a [`Buf`].
    fn write_all_buf<'a, B>(
        &'a mut self,
        src: &'a mut B,
    ) -> impl Future<Output = io::Result<()>> + 'a
    where
        Self: Unpin + Sized,
        B: Buf + ?Sized,
    {
        async move {
            while src.has_remaining() {
                let amount = self.write(src.chunk()).await?;
                if amount == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ));
                }
                src.advance(amount);
            }
            Ok(())
        }
    }

    /// Writes all bytes.
    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> WriteAll<'a, Self>
    where
        Self: Unpin + Sized,
    {
        WriteAll { writer: self, buf }
    }

    write_number_method!(write_u8, u8, |value: u8| [value]);
    write_number_method!(write_i8, i8, |value: i8| [value as u8]);
    write_number_method!(write_u16, u16, u16::to_be_bytes);
    write_number_method!(write_i16, i16, i16::to_be_bytes);
    write_number_method!(write_u32, u32, u32::to_be_bytes);
    write_number_method!(write_i32, i32, i32::to_be_bytes);
    write_number_method!(write_u64, u64, u64::to_be_bytes);
    write_number_method!(write_i64, i64, i64::to_be_bytes);
    write_number_method!(write_u128, u128, u128::to_be_bytes);
    write_number_method!(write_i128, i128, i128::to_be_bytes);
    write_number_method!(write_f32, f32, f32::to_be_bytes);
    write_number_method!(write_f64, f64, f64::to_be_bytes);
    write_number_method!(write_u16_le, u16, u16::to_le_bytes);
    write_number_method!(write_i16_le, i16, i16::to_le_bytes);
    write_number_method!(write_u32_le, u32, u32::to_le_bytes);
    write_number_method!(write_i32_le, i32, i32::to_le_bytes);
    write_number_method!(write_u64_le, u64, u64::to_le_bytes);
    write_number_method!(write_i64_le, i64, i64::to_le_bytes);
    write_number_method!(write_u128_le, u128, u128::to_le_bytes);
    write_number_method!(write_i128_le, i128, i128::to_le_bytes);
    write_number_method!(write_f32_le, f32, f32::to_le_bytes);
    write_number_method!(write_f64_le, f64, f64::to_le_bytes);

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

/// Future returned by [`AsyncWriteExt::write_vectored`].
#[must_use = "futures do nothing unless polled"]
pub struct WriteVectored<'a, 'b, W: ?Sized> {
    writer: &'a mut W,
    bufs: &'a [IoSlice<'b>],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteVectored<'_, '_, W> {
    type Output = io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut *this.writer).poll_write_vectored(cx, this.bufs)
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
            buf: vec![0; capacity],
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

    /// Returns a pinned mutable reference to the inner reader.
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut R> {
        unsafe { self.map_unchecked_mut(|this| &mut this.inner) }
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

impl<R: AsyncRead + AsyncWrite + Unpin> AsyncWrite for BufReader<R> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin> AsyncSeek for BufReader<R> {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        let buffered = (this.cap - this.pos) as i64;
        let position = match position {
            SeekFrom::Current(offset) => {
                SeekFrom::Current(offset.checked_sub(buffered).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflow")
                })?)
            }
            position => position,
        };
        this.pos = 0;
        this.cap = 0;
        Pin::new(&mut this.inner).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.get_mut().inner).poll_complete(cx)
    }
}

/// A buffered asynchronous writer.
pub struct BufWriter<W> {
    inner: W,
    buf: Vec<u8>,
    written: usize,
    capacity: usize,
    seek: Option<SeekFrom>,
}

impl<W> BufWriter<W> {
    /// Creates a writer with an 8 KiB buffer.
    pub fn new(inner: W) -> Self {
        Self::with_capacity(8 * 1024, inner)
    }

    /// Creates a writer with a specific buffer capacity.
    pub fn with_capacity(capacity: usize, inner: W) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(capacity),
            written: 0,
            capacity,
            seek: None,
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

    /// Returns a pinned mutable reference to the inner writer.
    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut W> {
        unsafe { self.map_unchecked_mut(|this| &mut this.inner) }
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

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.inner.is_write_vectored() {
            let total = bufs
                .iter()
                .fold(0usize, |sum, buf| sum.saturating_add(buf.len()));
            if total > this.capacity - this.buf.len() {
                ready!(this.poll_flush_buf(cx))?;
            }
            if total >= this.capacity {
                return Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
            }
            for buf in bufs {
                this.buf.extend_from_slice(buf);
            }
            return Poll::Ready(Ok(total));
        }

        while bufs.first().is_some_and(|buf| buf.is_empty()) {
            bufs = &bufs[1..];
        }
        let Some(first) = bufs.first() else {
            return Poll::Ready(Ok(0));
        };
        if first.len() > this.capacity - this.buf.len() {
            ready!(this.poll_flush_buf(cx))?;
        }
        if first.len() >= this.capacity {
            return Pin::new(&mut this.inner).poll_write(cx, first);
        }
        this.buf.extend_from_slice(first);
        let mut written = first.len();
        for buf in &bufs[1..] {
            if buf.len() > this.capacity - this.buf.len() {
                break;
            }
            this.buf.extend_from_slice(buf);
            written += buf.len();
        }
        Poll::Ready(Ok(written))
    }

    fn is_write_vectored(&self) -> bool {
        true
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

impl<W: AsyncWrite + AsyncRead + Unpin> AsyncRead for BufWriter<W> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_flush_buf(cx))?;
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<W: AsyncWrite + AsyncBufRead + Unpin> AsyncBufRead for BufWriter<W> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        ready!(this.poll_flush_buf(cx))?;
        Pin::new(&mut this.inner).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        Pin::new(&mut self.get_mut().inner).consume(amount)
    }
}

impl<W: AsyncWrite + AsyncSeek + Unpin> AsyncSeek for BufWriter<W> {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        if this.seek.is_some() {
            return Err(io::Error::other("another seek is already in progress"));
        }
        this.seek = Some(position);
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let this = self.get_mut();
        ready!(this.poll_flush_buf(cx))?;
        if let Some(position) = this.seek.take() {
            Pin::new(&mut this.inner).start_seek(position)?;
        }
        Pin::new(&mut this.inner).poll_complete(cx)
    }
}

/// A bidirectional stream with independent read and write buffers.
pub struct BufStream<RW> {
    inner: BufReader<BufWriter<RW>>,
}

impl<RW> BufStream<RW> {
    pub fn new(stream: RW) -> Self {
        Self::with_capacity(8 * 1024, 8 * 1024, stream)
    }

    pub fn with_capacity(reader_capacity: usize, writer_capacity: usize, stream: RW) -> Self {
        Self {
            inner: BufReader::with_capacity(
                reader_capacity,
                BufWriter::with_capacity(writer_capacity, stream),
            ),
        }
    }

    pub fn get_ref(&self) -> &RW {
        self.inner.get_ref().get_ref()
    }

    pub fn get_mut(&mut self) -> &mut RW {
        self.inner.get_mut().get_mut()
    }

    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut RW> {
        unsafe { self.map_unchecked_mut(|this| &mut this.inner.inner.inner) }
    }

    pub fn into_inner(self) -> RW {
        self.inner.into_inner().into_inner()
    }
}

impl<RW: AsyncRead + AsyncWrite + Unpin> AsyncRead for BufStream<RW> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<RW: AsyncRead + AsyncWrite + Unpin> AsyncBufRead for BufStream<RW> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        Pin::new(&mut self.get_mut().inner).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        Pin::new(&mut self.get_mut().inner).consume(amount)
    }
}

impl<RW: AsyncRead + AsyncWrite + Unpin> AsyncWrite for BufStream<RW> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

impl<RW: AsyncRead + AsyncWrite + AsyncSeek + Unpin> AsyncSeek for BufStream<RW> {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.get_mut().inner).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.get_mut().inner).poll_complete(cx)
    }
}

impl<RW: fmt::Debug> fmt::Debug for BufStream<RW> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufStream")
            .field("inner", self.get_ref())
            .finish()
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

/// Copies buffered data from `reader` to `writer` until EOF.
pub async fn copy_buf<R, W>(reader: &mut R, writer: &mut W) -> io::Result<u64>
where
    R: AsyncBufRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut amount = 0u64;
    std::future::poll_fn(|cx| {
        loop {
            let available = ready!(Pin::new(&mut *reader).poll_fill_buf(cx))?;
            if available.is_empty() {
                ready!(Pin::new(&mut *writer).poll_flush(cx))?;
                return Poll::Ready(Ok(amount));
            }
            let written = ready!(Pin::new(&mut *writer).poll_write(cx, available))?;
            if written == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to copy buffered stream",
                )));
            }
            Pin::new(&mut *reader).consume(written);
            amount += written as u64;
        }
    })
    .await
}

struct CopyDirection {
    buf: Vec<u8>,
    pos: usize,
    cap: usize,
    amount: u64,
    eof: bool,
    done: bool,
}

impl CopyDirection {
    fn new(size: usize) -> Self {
        Self {
            buf: vec![0; size.max(1)],
            pos: 0,
            cap: 0,
            amount: 0,
            eof: false,
            done: false,
        }
    }
}

fn poll_copy_direction<R, W>(
    reader: &mut R,
    writer: &mut W,
    state: &mut CopyDirection,
    cx: &mut Context<'_>,
) -> Poll<io::Result<u64>>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    if state.done {
        return Poll::Ready(Ok(state.amount));
    }
    loop {
        if state.pos == state.cap && !state.eof {
            let mut read_buf = ReadBuf::new(&mut state.buf);
            match Pin::new(&mut *reader).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    state.pos = 0;
                    state.cap = read_buf.len();
                    state.eof = state.cap == 0;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }

        while state.pos < state.cap {
            match Pin::new(&mut *writer).poll_write(cx, &state.buf[state.pos..state.cap]) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to copy bidirectional stream",
                    )));
                }
                Poll::Ready(Ok(written)) => {
                    state.pos += written;
                    state.amount += written as u64;
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }

        if state.eof {
            ready!(Pin::new(&mut *writer).poll_shutdown(cx))?;
            state.done = true;
            return Poll::Ready(Ok(state.amount));
        }
    }
}

/// Copies data in both directions between two duplex streams.
pub async fn copy_bidirectional<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    copy_bidirectional_with_sizes(a, b, 8 * 1024, 8 * 1024).await
}

/// Copies data in both directions using independently sized buffers.
pub async fn copy_bidirectional_with_sizes<A, B>(
    a: &mut A,
    b: &mut B,
    a_to_b_buffer_size: usize,
    b_to_a_buffer_size: usize,
) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let mut a_to_b = CopyDirection::new(a_to_b_buffer_size);
    let mut b_to_a = CopyDirection::new(b_to_a_buffer_size);
    std::future::poll_fn(|cx| {
        let first = poll_copy_direction(&mut *a, &mut *b, &mut a_to_b, cx);
        let second = poll_copy_direction(&mut *b, &mut *a, &mut b_to_a, cx);

        let first = match first {
            Poll::Ready(Ok(amount)) => Some(amount),
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Pending => None,
        };
        let second = match second {
            Poll::Ready(Ok(amount)) => Some(amount),
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Pending => None,
        };
        match (first, second) {
            (Some(first), Some(second)) => Poll::Ready(Ok((first, second))),
            _ => Poll::Pending,
        }
    })
    .await
}

/// Reader that always returns EOF.
#[derive(Clone, Copy, Debug, Default)]
pub struct Empty;

pub fn empty() -> Empty {
    Empty
}

impl AsyncRead for Empty {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncBufRead for Empty {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        Poll::Ready(Ok(&[]))
    }

    fn consume(self: Pin<&mut Self>, _amount: usize) {}
}

impl AsyncWrite for Empty {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bufs.iter().map(|buf| buf.len()).sum()))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncSeek for Empty {
    fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> io::Result<()> {
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(0))
    }
}

/// Reader that repeats one byte indefinitely.
#[derive(Clone, Copy, Debug)]
pub struct Repeat {
    byte: u8,
}

pub fn repeat(byte: u8) -> Repeat {
    Repeat { byte }
}

impl AsyncRead for Repeat {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        buf.initialize_unfilled().fill(self.byte);
        let amount = buf.remaining();
        buf.advance(amount);
        Poll::Ready(Ok(()))
    }
}

/// Writer that discards all bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sink;

pub fn sink() -> Sink {
    Sink
}

impl AsyncWrite for Sink {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Returns an asynchronous handle to the process standard input.
pub fn stdin() -> Stdin {
    Stdin { operation: None }
}

/// Returns an asynchronous handle to the process standard output.
pub fn stdout() -> Stdout {
    Stdout { operation: None }
}

/// Returns an asynchronous handle to the process standard error stream.
pub fn stderr() -> Stderr {
    Stderr { operation: None }
}

/// Asynchronous standard input backed by FFRT blocking work.
pub struct Stdin {
    operation: Option<crate::task::JoinHandle<io::Result<Vec<u8>>>>,
}

impl Stdin {
    /// Reads one UTF-8 line, including its trailing newline when present.
    pub async fn read_line(&mut self, destination: &mut String) -> io::Result<usize> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            let read = self.read(&mut byte).await?;
            if read == 0 {
                break;
            }
            bytes.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        destination.push_str(text);
        Ok(bytes.len())
    }
}

impl AsyncRead for Stdin {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        use std::io::Read as _;

        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if self.operation.is_none() {
            let capacity = buf.remaining().min(8 * 1024);
            self.operation = Some(crate::task::spawn_blocking(move || {
                let mut bytes = vec![0; capacity];
                let amount = std::io::stdin().read(&mut bytes)?;
                bytes.truncate(amount);
                Ok(bytes)
            }));
        }

        let operation = self.operation.as_mut().expect("stdin operation missing");
        match Pin::new(operation).poll(cx) {
            Poll::Ready(Ok(Ok(bytes))) => {
                self.operation = None;
                buf.put_slice(&bytes);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Ok(Err(error))) => {
                self.operation = None;
                Poll::Ready(Err(error))
            }
            Poll::Ready(Err(error)) => {
                self.operation = None;
                Poll::Ready(Err(join_error(error)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl fmt::Debug for Stdin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stdin").finish_non_exhaustive()
    }
}

enum StandardWriteOperation {
    Write(crate::task::JoinHandle<io::Result<usize>>),
    Flush(crate::task::JoinHandle<io::Result<()>>),
}

fn poll_standard_write(
    operation: &mut Option<StandardWriteOperation>,
    cx: &mut Context<'_>,
    bytes: &[u8],
    stderr: bool,
) -> Poll<io::Result<usize>> {
    use std::io::Write as _;

    if operation.is_none() {
        let bytes = bytes.to_vec();
        *operation = Some(StandardWriteOperation::Write(crate::task::spawn_blocking(
            move || {
                if stderr {
                    std::io::stderr().write(&bytes)
                } else {
                    std::io::stdout().write(&bytes)
                }
            },
        )));
    }
    let StandardWriteOperation::Write(handle) = operation.as_mut().expect("write missing") else {
        return Poll::Ready(Err(io::Error::other(
            "standard stream flush is already in progress",
        )));
    };
    match Pin::new(handle).poll(cx) {
        Poll::Ready(Ok(result)) => {
            *operation = None;
            Poll::Ready(result)
        }
        Poll::Ready(Err(error)) => {
            *operation = None;
            Poll::Ready(Err(join_error(error)))
        }
        Poll::Pending => Poll::Pending,
    }
}

fn poll_standard_flush(
    operation: &mut Option<StandardWriteOperation>,
    cx: &mut Context<'_>,
    stderr: bool,
) -> Poll<io::Result<()>> {
    use std::io::Write as _;

    if operation.is_none() {
        *operation = Some(StandardWriteOperation::Flush(crate::task::spawn_blocking(
            move || {
                if stderr {
                    std::io::stderr().flush()
                } else {
                    std::io::stdout().flush()
                }
            },
        )));
    }
    let StandardWriteOperation::Flush(handle) = operation.as_mut().expect("flush missing") else {
        return Poll::Ready(Err(io::Error::other(
            "standard stream write is already in progress",
        )));
    };
    match Pin::new(handle).poll(cx) {
        Poll::Ready(Ok(result)) => {
            *operation = None;
            Poll::Ready(result)
        }
        Poll::Ready(Err(error)) => {
            *operation = None;
            Poll::Ready(Err(join_error(error)))
        }
        Poll::Pending => Poll::Pending,
    }
}

fn join_error(error: crate::JoinError) -> io::Error {
    io::Error::other(error.to_string())
}

macro_rules! standard_writer {
    ($name:ident, $stderr:literal) => {
        /// Asynchronous process standard stream backed by FFRT blocking work.
        pub struct $name {
            operation: Option<StandardWriteOperation>,
        }

        impl AsyncWrite for $name {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                poll_standard_write(&mut self.operation, cx, buf, $stderr)
            }

            fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                poll_standard_flush(&mut self.operation, cx, $stderr)
            }

            fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                self.poll_flush(cx)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}

standard_writer!(Stdout, false);
standard_writer!(Stderr, true);

struct MemoryPipe {
    buffer: VecDeque<u8>,
    capacity: usize,
    writer_closed: bool,
    reader_closed: bool,
    reader_waker: Option<Waker>,
    writer_waker: Option<Waker>,
}

impl MemoryPipe {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            writer_closed: false,
            reader_closed: false,
            reader_waker: None,
            writer_waker: None,
        }
    }

    fn register(slot: &mut Option<Waker>, waker: &Waker) {
        if slot
            .as_ref()
            .is_none_or(|registered| !registered.will_wake(waker))
        {
            *slot = Some(waker.clone());
        }
    }
}

/// In-memory bidirectional stream with bounded backpressure.
pub struct DuplexStream {
    incoming: Arc<StdMutex<MemoryPipe>>,
    outgoing: Arc<StdMutex<MemoryPipe>>,
}

/// Creates a pair of connected in-memory streams.
pub fn duplex(max_buf_size: usize) -> (DuplexStream, DuplexStream) {
    assert!(max_buf_size > 0, "duplex capacity must be non-zero");
    let left_to_right = Arc::new(StdMutex::new(MemoryPipe::new(max_buf_size)));
    let right_to_left = Arc::new(StdMutex::new(MemoryPipe::new(max_buf_size)));
    (
        DuplexStream {
            incoming: right_to_left.clone(),
            outgoing: left_to_right.clone(),
        },
        DuplexStream {
            incoming: left_to_right,
            outgoing: right_to_left,
        },
    )
}

impl AsyncRead for DuplexStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut pipe = self.incoming.lock().unwrap();
        if !pipe.buffer.is_empty() {
            let amount = pipe.buffer.len().min(buf.remaining());
            for _ in 0..amount {
                buf.put_slice(&[pipe.buffer.pop_front().unwrap()]);
            }
            if let Some(waker) = pipe.writer_waker.take() {
                waker.wake();
            }
            return Poll::Ready(Ok(()));
        }
        if pipe.writer_closed {
            return Poll::Ready(Ok(()));
        }
        MemoryPipe::register(&mut pipe.reader_waker, cx.waker());
        Poll::Pending
    }
}

impl AsyncWrite for DuplexStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut pipe = self.outgoing.lock().unwrap();
        if pipe.reader_closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "duplex peer dropped its reader",
            )));
        }
        let available = pipe.capacity - pipe.buffer.len();
        if available == 0 {
            MemoryPipe::register(&mut pipe.writer_waker, cx.waker());
            return Poll::Pending;
        }
        let amount = available.min(buf.len());
        pipe.buffer.extend(&buf[..amount]);
        if let Some(waker) = pipe.reader_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(amount))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut pipe = self.outgoing.lock().unwrap();
        pipe.writer_closed = true;
        if let Some(waker) = pipe.reader_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        {
            let mut outgoing = self.outgoing.lock().unwrap();
            outgoing.writer_closed = true;
            if let Some(waker) = outgoing.reader_waker.take() {
                waker.wake();
            }
        }
        let mut incoming = self.incoming.lock().unwrap();
        incoming.reader_closed = true;
        if let Some(waker) = incoming.writer_waker.take() {
            waker.wake();
        }
    }
}

impl fmt::Debug for DuplexStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplexStream").finish_non_exhaustive()
    }
}

/// Stream type used by [`simplex`].
pub struct SimplexStream(DuplexStream);

impl AsyncRead for SimplexStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl AsyncWrite for SimplexStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

/// Creates a bounded in-memory one-way stream.
pub fn simplex(max_buf_size: usize) -> (ReadHalf<SimplexStream>, WriteHalf<SimplexStream>) {
    let (writer_end, reader_end) = duplex(max_buf_size);
    let (_unused_reader, writer) = split(SimplexStream(writer_end));
    let (reader, _unused_writer) = split(SimplexStream(reader_end));
    (reader, writer)
}

/// Joins a reader and writer into one bidirectional I/O handle.
pub fn join<R, W>(reader: R, writer: W) -> Join<R, W>
where
    R: AsyncRead,
    W: AsyncWrite,
{
    Join { reader, writer }
}

/// A bidirectional handle composed from separate reader and writer values.
#[derive(Debug)]
pub struct Join<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> Join<R, W> {
    pub fn into_inner(self) -> (R, W) {
        (self.reader, self.writer)
    }

    pub fn reader(&self) -> &R {
        &self.reader
    }

    pub fn writer(&self) -> &W {
        &self.writer
    }

    pub fn reader_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    pub fn reader_pin_mut(self: Pin<&mut Self>) -> Pin<&mut R> {
        unsafe { self.map_unchecked_mut(|joined| &mut joined.reader) }
    }

    pub fn writer_pin_mut(self: Pin<&mut Self>) -> Pin<&mut W> {
        unsafe { self.map_unchecked_mut(|joined| &mut joined.writer) }
    }
}

impl<R: AsyncRead, W> AsyncRead for Join<R, W> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.reader_pin_mut().poll_read(cx, buf)
    }
}

impl<R, W: AsyncWrite> AsyncWrite for Join<R, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.writer_pin_mut().poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.writer_pin_mut().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.writer.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.writer_pin_mut().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.writer_pin_mut().poll_shutdown(cx)
    }
}

impl<R: AsyncBufRead, W> AsyncBufRead for Join<R, W> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        self.reader_pin_mut().poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        self.reader_pin_mut().consume(amount)
    }
}

struct SplitInner<T> {
    io: FfrtMutex<T>,
}

/// The read half returned by [`split`].
pub struct ReadHalf<T> {
    inner: Arc<SplitInner<T>>,
}

/// The write half returned by [`split`].
pub struct WriteHalf<T> {
    inner: Arc<SplitInner<T>>,
}

/// Splits a duplex I/O object into independently owned read and write halves.
pub fn split<T>(io: T) -> (ReadHalf<T>, WriteHalf<T>)
where
    T: AsyncRead + AsyncWrite,
{
    let inner = Arc::new(SplitInner {
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
        let result = crate::Runtime::new().unwrap().block_on(async move {
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
        let result = crate::Runtime::new().unwrap().block_on(async move {
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
        let result = crate::Runtime::new().unwrap().block_on(async move {
            let mut reader = BufReader::with_capacity(3, &b"one\ntwo"[..]);
            let mut line = String::new();
            assert_eq!(reader.read_line(&mut line).await?, 4);
            assert_eq!(line, "one\n");
            Ok::<(), io::Error>(())
        });
        assert!(result.is_ok());
    }
}

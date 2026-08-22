//! Readiness based I/O built on the FFRT loop.
//!
//! [`AsyncFd`] is the lowest-level non-blocking I/O primitive in this crate.
//! File descriptors registered here must already be in non-blocking mode.

use std::fmt;
use std::future::Future;
use std::io;
use std::ops::{BitOr, BitOrAssign};
use std::os::fd::{AsRawFd, RawFd};
use std::os::raw::c_void;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};

use crate::lock::Mutex;
use crate::looper::{EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, Looper};
use crate::queue::{Queue, QueueType};

const ERROR_EVENTS: u32 = EPOLLERR | EPOLLHUP;

/// An I/O readiness interest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interest(u32);

impl Interest {
    /// Read readiness.
    pub const READABLE: Self = Self(EPOLLIN);
    /// Write readiness.
    pub const WRITABLE: Self = Self(EPOLLOUT);

    /// Returns whether this interest includes read readiness.
    pub const fn is_readable(self) -> bool {
        self.0 & EPOLLIN != 0
    }

    /// Returns whether this interest includes write readiness.
    pub const fn is_writable(self) -> bool {
        self.0 & EPOLLOUT != 0
    }
}

impl BitOr for Interest {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Interest {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Readiness returned by [`AsyncFd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ready(u32);

impl Ready {
    /// No readiness was observed.
    pub const EMPTY: Self = Self(0);
    /// The descriptor may be read without blocking.
    pub const READABLE: Self = Self(EPOLLIN);
    /// The descriptor may be written without blocking.
    pub const WRITABLE: Self = Self(EPOLLOUT);
    /// The descriptor reported an error.
    pub const ERROR: Self = Self(EPOLLERR);
    /// The descriptor reported a hangup.
    pub const READ_CLOSED: Self = Self(EPOLLHUP);
    /// The descriptor reported a hangup.
    pub const WRITE_CLOSED: Self = Self(EPOLLHUP);

    /// Returns whether read readiness was observed.
    pub const fn is_readable(self) -> bool {
        self.0 & (EPOLLIN | EPOLLHUP) != 0
    }

    /// Returns whether write readiness was observed.
    pub const fn is_writable(self) -> bool {
        self.0 & (EPOLLOUT | EPOLLHUP) != 0
    }

    /// Returns whether an error was observed.
    pub const fn is_error(self) -> bool {
        self.0 & EPOLLERR != 0
    }

    /// Returns whether a read-side close was observed.
    pub const fn is_read_closed(self) -> bool {
        self.0 & EPOLLHUP != 0
    }

    /// Returns whether a write-side close was observed.
    pub const fn is_write_closed(self) -> bool {
        self.0 & EPOLLHUP != 0
    }
}

impl BitOr for Ready {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Default)]
struct Waiters {
    readers: Vec<Waker>,
    writers: Vec<Waker>,
}

impl Waiters {
    fn register(waiters: &mut Vec<Waker>, waker: &Waker) {
        if let Some(existing) = waiters.iter_mut().find(|old| old.will_wake(waker)) {
            *existing = waker.clone();
        } else {
            waiters.push(waker.clone());
        }
    }
}

struct ReactorInner {
    // Field order is significant if a non-static reactor is introduced later:
    // the loop must be destroyed before its backing queue.
    looper: Looper,
    _queue: Queue,
}

impl ReactorInner {
    fn start() -> Arc<Self> {
        let queue = Queue::new(QueueType::Concurrent, "ffrt-rs-reactor", None);
        let looper = Looper::new(&queue);
        let reactor = Arc::new(Self {
            looper,
            _queue: queue,
        });
        let runner = reactor.clone();
        std::thread::Builder::new()
            .name("ffrt-rs-reactor".into())
            .spawn(move || {
                let _ = runner.looper.run();
            })
            .expect("failed to start FFRT reactor thread");
        reactor
    }
}

fn reactor() -> &'static Arc<ReactorInner> {
    static REACTOR: OnceLock<Arc<ReactorInner>> = OnceLock::new();
    REACTOR.get_or_init(ReactorInner::start)
}

struct ScheduledIo {
    fd: RawFd,
    interest: Interest,
    readiness: AtomicU32,
    registered: AtomicBool,
    armed: AtomicBool,
    waiters: Mutex<Waiters>,
    reactor: Arc<ReactorInner>,
}

unsafe extern "C" fn readiness_callback(data: *mut c_void, events: u32) {
    if data.is_null() {
        return;
    }

    // The registration owns one strong Arc reference for exactly as long as
    // FFRT may invoke this callback. Take a temporary reference so a concurrent
    // AsyncFd drop cannot release the callback state while this call is active.
    let pointer = data.cast::<ScheduledIo>();
    unsafe { Arc::increment_strong_count(pointer) };
    let io = unsafe { Arc::from_raw(pointer) };
    if !io.registered.load(Ordering::Acquire) || !io.armed.swap(false, Ordering::AcqRel) {
        return;
    }

    // FFRT does not consistently honor EPOLLONESHOT on every OpenHarmony
    // architecture. Remove the descriptor before publishing readiness and
    // add it again only after the caller observes WouldBlock.
    let _ = unsafe {
        io.reactor
            .looper
            .epoll_ctl(EPOLL_CTL_DEL, io.fd, 0, std::ptr::null_mut(), None)
    };
    io.readiness.fetch_or(events, Ordering::AcqRel);

    let (readers, writers) = {
        let mut waiters = io.waiters.lock().unwrap();
        let readers = if events & (EPOLLIN | ERROR_EVENTS) != 0 {
            std::mem::take(&mut waiters.readers)
        } else {
            Vec::new()
        };
        let writers = if events & (EPOLLOUT | ERROR_EVENTS) != 0 {
            std::mem::take(&mut waiters.writers)
        } else {
            Vec::new()
        };
        (readers, writers)
    };

    for waker in readers.into_iter().chain(writers) {
        waker.wake();
    }
}

impl ScheduledIo {
    fn register(fd: RawFd, interest: Interest) -> io::Result<Arc<Self>> {
        if fd < 0 || interest.0 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AsyncFd requires a valid fd and non-empty interest",
            ));
        }

        let reactor = reactor().clone();
        let io = Arc::new(Self {
            fd,
            interest,
            readiness: AtomicU32::new(0),
            registered: AtomicBool::new(true),
            armed: AtomicBool::new(true),
            waiters: Mutex::new(Waiters::default()),
            reactor,
        });

        // Keep the callback data alive independently of the public AsyncFd.
        let data = Arc::into_raw(io.clone()).cast_mut().cast::<c_void>();
        let result = unsafe {
            io.reactor.looper.epoll_ctl(
                EPOLL_CTL_ADD,
                fd,
                interest.0 | ERROR_EVENTS,
                data,
                Some(readiness_callback),
            )
        };

        if let Err(code) = result {
            io.armed.store(false, Ordering::Release);
            io.registered.store(false, Ordering::Release);
            unsafe { drop(Arc::from_raw(data.cast::<ScheduledIo>())) };
            return Err(io::Error::from_raw_os_error(code));
        }

        Ok(io)
    }

    fn poll_ready(&self, cx: &mut Context<'_>, interest: Interest) -> Poll<io::Result<Ready>> {
        let mask = interest.0 | ERROR_EVENTS;
        let ready = self.readiness.load(Ordering::Acquire) & mask;
        if ready != 0 {
            return Poll::Ready(Ok(Ready(ready)));
        }

        let mut waiters = self.waiters.lock().unwrap();
        let ready = self.readiness.load(Ordering::Acquire) & mask;
        if ready != 0 {
            return Poll::Ready(Ok(Ready(ready)));
        }

        if interest.is_readable() {
            Waiters::register(&mut waiters.readers, cx.waker());
        }
        if interest.is_writable() {
            Waiters::register(&mut waiters.writers, cx.waker());
        }
        Poll::Pending
    }

    fn clear_ready(&self, ready: Ready) {
        self.readiness.fetch_and(!ready.0, Ordering::AcqRel);
        if self.registered.load(Ordering::Acquire)
            && self
                .armed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let result = unsafe {
                self.reactor.looper.epoll_ctl(
                    EPOLL_CTL_ADD,
                    self.fd,
                    self.interest.0 | ERROR_EVENTS,
                    (self as *const Self).cast_mut().cast::<c_void>(),
                    Some(readiness_callback),
                )
            };
            if result.is_err() {
                self.armed.store(false, Ordering::Release);
                self.readiness.fetch_or(EPOLLERR, Ordering::AcqRel);
                let waiters = {
                    let mut waiters = self.waiters.lock().unwrap();
                    let readers = std::mem::take(&mut waiters.readers);
                    readers
                        .into_iter()
                        .chain(std::mem::take(&mut waiters.writers))
                        .collect::<Vec<_>>()
                };
                for waker in waiters {
                    waker.wake();
                }
            }
        }
    }

    fn deregister(&self) {
        if !self.registered.swap(false, Ordering::AcqRel) {
            return;
        }

        self.armed.store(false, Ordering::Release);
        let _ = unsafe {
            self.reactor
                .looper
                .epoll_ctl(EPOLL_CTL_DEL, self.fd, 0, std::ptr::null_mut(), None)
        };

        // Balance the Arc::into_raw performed by register. FFRT guarantees
        // that a deleted registration no longer invokes its callback.
        unsafe { Arc::decrement_strong_count(self as *const Self) };
    }
}

/// Associates an owned I/O object with the FFRT readiness reactor.
///
/// The object must remain in non-blocking mode for the lifetime of `AsyncFd`.
pub struct AsyncFd<T: AsRawFd> {
    io: Option<T>,
    state: Arc<ScheduledIo>,
}

impl<T: AsRawFd> AsyncFd<T> {
    /// Registers an object for both read and write readiness.
    pub fn new(io: T) -> io::Result<Self> {
        Self::with_interest(io, Interest::READABLE | Interest::WRITABLE)
    }

    /// Registers an object for the requested readiness interest.
    pub fn with_interest(io: T, interest: Interest) -> io::Result<Self> {
        let state = ScheduledIo::register(io.as_raw_fd(), interest)?;
        Ok(Self {
            io: Some(io),
            state,
        })
    }

    /// Returns a shared reference to the registered object.
    pub fn get_ref(&self) -> &T {
        self.io.as_ref().expect("AsyncFd inner value missing")
    }

    /// Returns a mutable reference to the registered object.
    pub fn get_mut(&mut self) -> &mut T {
        self.io.as_mut().expect("AsyncFd inner value missing")
    }

    /// Deregisters and returns the underlying object.
    pub fn into_inner(mut self) -> T {
        self.state.deregister();
        self.io.take().expect("AsyncFd inner value missing")
    }

    /// Waits for read readiness.
    pub fn readable(&self) -> AsyncFdReady<'_, T> {
        AsyncFdReady {
            fd: self,
            interest: Interest::READABLE,
        }
    }

    /// Waits for write readiness.
    pub fn writable(&self) -> AsyncFdReady<'_, T> {
        AsyncFdReady {
            fd: self,
            interest: Interest::WRITABLE,
        }
    }

    /// Waits for the requested readiness interest.
    pub fn ready(&self, interest: Interest) -> AsyncFdReady<'_, T> {
        AsyncFdReady { fd: self, interest }
    }

    /// Polls for read readiness.
    pub fn poll_read_ready(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<AsyncFdReadyGuard<'_, T>>> {
        self.poll_ready_interest(cx, Interest::READABLE)
    }

    /// Polls for write readiness.
    pub fn poll_write_ready(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<AsyncFdReadyGuard<'_, T>>> {
        self.poll_ready_interest(cx, Interest::WRITABLE)
    }

    /// Polls for the requested readiness interest.
    pub fn poll_ready(
        &self,
        cx: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<AsyncFdReadyGuard<'_, T>>> {
        self.poll_ready_interest(cx, interest)
    }

    fn poll_ready_interest(
        &self,
        cx: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<AsyncFdReadyGuard<'_, T>>> {
        self.state
            .poll_ready(cx, interest)
            .map_ok(|ready| AsyncFdReadyGuard {
                fd: self,
                ready,
                cleared: false,
            })
    }
}

impl<T: AsRawFd + fmt::Debug> fmt::Debug for AsyncFd<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncFd")
            .field("inner", &self.io)
            .field("fd", &self.state.fd)
            .finish()
    }
}

impl<T: AsRawFd> AsRawFd for AsyncFd<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.state.fd
    }
}

impl<T: AsRawFd> Drop for AsyncFd<T> {
    fn drop(&mut self) {
        self.state.deregister();
    }
}

/// Future returned by [`AsyncFd::readable`] and [`AsyncFd::writable`].
#[must_use = "futures do nothing unless polled"]
pub struct AsyncFdReady<'a, T: AsRawFd> {
    fd: &'a AsyncFd<T>,
    interest: Interest,
}

impl<'a, T: AsRawFd> Future for AsyncFdReady<'a, T> {
    type Output = io::Result<AsyncFdReadyGuard<'a, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.fd.poll_ready_interest(cx, self.interest)
    }
}

/// A readiness observation for an [`AsyncFd`].
pub struct AsyncFdReadyGuard<'a, T: AsRawFd> {
    fd: &'a AsyncFd<T>,
    ready: Ready,
    cleared: bool,
}

impl<T: AsRawFd> AsyncFdReadyGuard<'_, T> {
    /// Returns the observed readiness flags.
    pub fn ready(&self) -> Ready {
        self.ready
    }

    /// Clears the observed readiness and rearms the one-shot registration.
    pub fn clear_ready(mut self) {
        self.fd.state.clear_ready(self.ready);
        self.cleared = true;
    }

    /// Runs a non-blocking I/O operation.
    ///
    /// `WouldBlock` clears the readiness and returns [`TryIoError`], allowing
    /// the caller to wait for a fresh edge.
    pub fn try_io<R>(
        mut self,
        f: impl FnOnce(&T) -> io::Result<R>,
    ) -> Result<io::Result<R>, TryIoError> {
        match f(self.fd.get_ref()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.fd.state.clear_ready(self.ready);
                self.cleared = true;
                Err(TryIoError(()))
            }
            result => Ok(result),
        }
    }
}

/// Error indicating that an operation still would have blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TryIoError(());

impl fmt::Display for TryIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operation would block")
    }
}

impl std::error::Error for TryIoError {}

//! Readiness based I/O built on the FFRT loop.
//!
//! [`AsyncFd`] is the lowest-level non-blocking I/O primitive in this crate.
//! File descriptors registered here must already be in non-blocking mode.

use std::fmt;
use std::future::Future;
use std::io;
use std::ops::{BitAnd, BitOr, BitOrAssign, Sub};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::raw::c_void;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::lock::Mutex;
use crate::looper::{
    Looper, LooperTimer, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLPRI, EPOLL_CTL_ADD,
    EPOLL_CTL_DEL,
};
use crate::queue::{Queue, QueueType};

const ERROR_EVENTS: u32 = EPOLLERR | EPOLLHUP;

fn pack_readiness(generation: u32, ready: u32) -> u64 {
    ((generation as u64) << 32) | ready as u64
}

fn unpack_readiness(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

/// An I/O readiness interest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interest(u32);

impl Interest {
    /// Read readiness.
    pub const READABLE: Self = Self(EPOLLIN);
    /// Write readiness.
    pub const WRITABLE: Self = Self(EPOLLOUT);
    /// Error readiness.
    pub const ERROR: Self = Self(EPOLLERR);
    /// Priority read readiness.
    pub const PRIORITY: Self = Self(EPOLLPRI);

    /// Returns whether this interest includes read readiness.
    pub const fn is_readable(self) -> bool {
        self.0 & EPOLLIN != 0
    }

    /// Returns whether this interest includes write readiness.
    pub const fn is_writable(self) -> bool {
        self.0 & EPOLLOUT != 0
    }

    pub const fn is_error(self) -> bool {
        self.0 & EPOLLERR != 0
    }

    pub const fn is_priority(self) -> bool {
        self.0 & EPOLLPRI != 0
    }

    #[must_use]
    pub const fn add(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub fn remove(self, other: Self) -> Option<Self> {
        let interest = Self(self.0 & !other.0);
        (interest.0 != 0).then_some(interest)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Priority data may be read.
    pub const PRIORITY: Self = Self(EPOLLPRI);
    /// The descriptor reported a hangup.
    pub const READ_CLOSED: Self = Self(EPOLLHUP);
    /// The descriptor reported a hangup.
    pub const WRITE_CLOSED: Self = Self(EPOLLHUP);
    /// Every readiness represented by OpenHarmony epoll.
    pub const ALL: Self = Self(EPOLLIN | EPOLLOUT | EPOLLPRI | EPOLLERR | EPOLLHUP);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

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

    pub const fn is_priority(self) -> bool {
        self.0 & EPOLLPRI != 0
    }
}

impl BitOr for Ready {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Ready {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Ready {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl Sub for Ready {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 & !rhs.0)
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

/// Registers a one-shot timer on the same FFRT loop that drives I/O.
pub(crate) fn register_timer(
    timeout: Duration,
    callback: impl FnMut() + Send + 'static,
) -> io::Result<LooperTimer> {
    // FFRT loop timers use millisecond precision. Round up so an async sleep
    // never completes before its requested deadline.
    let millis = timeout
        .as_nanos()
        .saturating_add(999_999)
        .checked_div(1_000_000)
        .unwrap_or(0)
        .max(1);
    let millis = u64::try_from(millis).unwrap_or(u64::MAX);
    reactor()
        .looper
        .try_timer_start(millis, false, callback)
        .map_err(|code| {
            if code > 0 {
                io::Error::from_raw_os_error(code)
            } else {
                io::Error::other("failed to register FFRT loop timer")
            }
        })
}

struct ScheduledIo {
    fd: RawFd,
    interest: Interest,
    readiness: AtomicU64,
    registered: AtomicBool,
    // Serializes ADD/DEL, readiness changes and final deregistration.
    events: Mutex<u32>,
    token: usize,
    waiters: Mutex<Waiters>,
    reactor: Arc<ReactorInner>,
}

fn registrations(
) -> &'static std::sync::Mutex<std::collections::HashMap<usize, std::sync::Weak<ScheduledIo>>> {
    static REGISTRATIONS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<usize, std::sync::Weak<ScheduledIo>>>,
    > = OnceLock::new();
    REGISTRATIONS.get_or_init(Default::default)
}

fn next_token() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .expect("FFRT I/O registration identifiers exhausted")
}

unsafe extern "C" fn readiness_callback(data: *mut c_void, events: u32) {
    // FFRT may deliver an event from a batch fetched before DEL. The opaque
    // token is never dereferenced or reused; stale callbacks simply miss the
    // registry. A successful upgrade owns the state for the entire callback.
    let io = registrations()
        .lock()
        .unwrap()
        .get(&(data as usize))
        .and_then(std::sync::Weak::upgrade);
    let Some(io) = io else { return };
    {
        let mut registered_events = io.events.lock().unwrap();
        if !io.registered.load(Ordering::Acquire) {
            return;
        }
        let (generation, ready) = unpack_readiness(io.readiness.load(Ordering::Acquire));
        io.readiness.store(
            pack_readiness(generation.wrapping_add(1), ready | events),
            Ordering::Release,
        );
        if io.update_interest(&mut registered_events).is_err() {
            io.record_error();
        }
    }
    io.wake_ready();
}

impl ScheduledIo {
    fn register(fd: RawFd, interest: Interest) -> io::Result<Arc<Self>> {
        if fd < 0 || interest.0 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AsyncFd requires a valid fd and non-empty interest",
            ));
        }
        let io = Arc::new(Self {
            fd,
            interest,
            readiness: AtomicU64::new(0),
            registered: AtomicBool::new(true),
            events: Mutex::new(0),
            token: next_token(),
            waiters: Mutex::new(Waiters::default()),
            reactor: reactor().clone(),
        });
        registrations()
            .lock()
            .unwrap()
            .insert(io.token, Arc::downgrade(&io));
        let result = io.update_interest(&mut io.events.lock().unwrap());
        if let Err(error) = result {
            io.deregister();
            return Err(error);
        }
        Ok(io)
    }

    // Called only with `events` locked. Keep monitoring directions that have
    // no cached readiness: a writable socket must still receive read events.
    // FFRT lacks reliable MOD/ONESHOT support, so change masks using DEL/ADD.
    fn update_interest(&self, events: &mut u32) -> io::Result<()> {
        let (_, ready) = unpack_readiness(self.readiness.load(Ordering::Acquire));
        let desired = if ready & ERROR_EVENTS != 0 {
            0
        } else {
            self.interest.0 & !ready
        };
        if desired == *events {
            return Ok(());
        }
        if *events != 0 {
            let result = unsafe {
                self.reactor
                    .looper
                    .epoll_ctl(EPOLL_CTL_DEL, self.fd, 0, std::ptr::null_mut(), None)
            };
            *events = 0;
            if result.is_err() {
                return Err(io::Error::other("failed to remove FFRT fd interest"));
            }
        }
        if desired != 0 {
            unsafe {
                self.reactor.looper.epoll_ctl(
                    EPOLL_CTL_ADD,
                    self.fd,
                    desired | ERROR_EVENTS,
                    self.token as *mut c_void,
                    Some(readiness_callback),
                )
            }
            .map_err(|_| io::Error::other("failed to register FFRT fd interest"))?;
            *events = desired;
        }
        Ok(())
    }

    fn record_error(&self) {
        let (generation, ready) = unpack_readiness(self.readiness.load(Ordering::Acquire));
        self.readiness.store(
            pack_readiness(generation.wrapping_add(1), ready | EPOLLERR),
            Ordering::Release,
        );
    }

    fn wake_ready(&self) {
        let (_, ready) = unpack_readiness(self.readiness.load(Ordering::Acquire));
        let (readers, writers) = {
            let mut waiters = self.waiters.lock().unwrap();
            let readers = if ready & (EPOLLIN | EPOLLPRI | ERROR_EVENTS) != 0 {
                std::mem::take(&mut waiters.readers)
            } else {
                Vec::new()
            };
            let writers = if ready & (EPOLLOUT | ERROR_EVENTS) != 0 {
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

    fn poll_ready(
        &self,
        cx: &mut Context<'_>,
        interest: Interest,
    ) -> Poll<io::Result<(Ready, u32)>> {
        let mask = interest.0 | ERROR_EVENTS;
        let mut waiters = self.waiters.lock().unwrap();
        let (generation, ready) = unpack_readiness(self.readiness.load(Ordering::Acquire));
        let ready = ready & mask;
        if ready != 0 {
            return Poll::Ready(Ok((Ready(ready), generation)));
        }
        if interest.is_readable() || interest.is_priority() || interest.is_error() {
            Waiters::register(&mut waiters.readers, cx.waker());
        }
        if interest.is_writable() {
            Waiters::register(&mut waiters.writers, cx.waker());
        }
        Poll::Pending
    }

    fn clear_ready(&self, ready: Ready, generation: u32) {
        let failed = {
            let mut events = self.events.lock().unwrap();
            if !self.registered.load(Ordering::Acquire) {
                return;
            }
            let (observed_generation, observed_ready) =
                unpack_readiness(self.readiness.load(Ordering::Acquire));
            if observed_generation != generation {
                return;
            }
            self.readiness.store(
                pack_readiness(generation, observed_ready & !ready.0),
                Ordering::Release,
            );
            let failed = self.update_interest(&mut events).is_err();
            if failed {
                self.record_error();
            }
            failed
        };
        if failed {
            self.wake_ready();
        }
    }

    fn deregister(&self) {
        let mut events = self.events.lock().unwrap();
        if !self.registered.swap(false, Ordering::AcqRel) {
            return;
        }
        registrations().lock().unwrap().remove(&self.token);
        if *events != 0 {
            let _ = unsafe {
                self.reactor
                    .looper
                    .epoll_ctl(EPOLL_CTL_DEL, self.fd, 0, std::ptr::null_mut(), None)
            };
            *events = 0;
        }
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
        Self::try_with_interest(io, interest).map_err(AsyncFdTryNewError::into_error)
    }

    /// Registers an object while returning ownership when registration fails.
    pub fn try_new(io: T) -> Result<Self, AsyncFdTryNewError<T>> {
        Self::try_with_interest(io, Interest::READABLE | Interest::WRITABLE)
    }

    /// Registers an object with an interest while preserving it on failure.
    pub fn try_with_interest(io: T, interest: Interest) -> Result<Self, AsyncFdTryNewError<T>> {
        match ScheduledIo::register(io.as_raw_fd(), interest) {
            Ok(state) => Ok(Self {
                io: Some(io),
                state,
            }),
            Err(error) => Err(AsyncFdTryNewError { inner: io, error }),
        }
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

    /// Waits for read readiness with mutable access to the wrapper.
    pub fn readable_mut(&mut self) -> AsyncFdReadyMut<'_, T> {
        AsyncFdReadyMut {
            fd: Some(self),
            interest: Interest::READABLE,
        }
    }

    /// Waits for write readiness with mutable access to the wrapper.
    pub fn writable_mut(&mut self) -> AsyncFdReadyMut<'_, T> {
        AsyncFdReadyMut {
            fd: Some(self),
            interest: Interest::WRITABLE,
        }
    }

    /// Waits for the requested readiness interest.
    pub fn ready(&self, interest: Interest) -> AsyncFdReady<'_, T> {
        AsyncFdReady { fd: self, interest }
    }

    /// Waits for readiness with mutable access to the wrapper.
    pub fn ready_mut(&mut self, interest: Interest) -> AsyncFdReadyMut<'_, T> {
        AsyncFdReadyMut {
            fd: Some(self),
            interest,
        }
    }

    /// Performs non-blocking I/O once when readiness is already cached.
    pub fn try_io<R>(
        &self,
        interest: Interest,
        f: impl FnOnce(&T) -> io::Result<R>,
    ) -> io::Result<R> {
        let (generation, ready) = unpack_readiness(self.state.readiness.load(Ordering::Acquire));
        let ready = Ready(ready & (interest.0 | ERROR_EVENTS));
        if ready.0 == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        match f(self.get_ref()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.state.clear_ready(ready, generation);
                Err(error)
            }
            result => result,
        }
    }

    /// Performs mutable non-blocking I/O once when readiness is cached.
    pub fn try_io_mut<R>(
        &mut self,
        interest: Interest,
        f: impl FnOnce(&mut T) -> io::Result<R>,
    ) -> io::Result<R> {
        let (generation, ready) = unpack_readiness(self.state.readiness.load(Ordering::Acquire));
        let ready = Ready(ready & (interest.0 | ERROR_EVENTS));
        if ready.0 == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        match f(self.get_mut()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.state.clear_ready(ready, generation);
                Err(error)
            }
            result => result,
        }
    }

    /// Waits for readiness and retries a non-blocking I/O operation.
    pub async fn async_io<R>(
        &self,
        interest: Interest,
        mut f: impl FnMut(&T) -> io::Result<R>,
    ) -> io::Result<R> {
        loop {
            let mut guard = self.ready(interest).await?;
            match guard.try_io(|inner| f(inner)) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }

    /// Mutable variant of [`AsyncFd::async_io`].
    pub async fn async_io_mut<R>(
        &mut self,
        interest: Interest,
        mut f: impl FnMut(&mut T) -> io::Result<R>,
    ) -> io::Result<R> {
        loop {
            let mut guard = self.ready_mut(interest).await?;
            match guard.try_io(|inner| f(inner)) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
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
            .map_ok(|(ready, generation)| AsyncFdReadyGuard {
                fd: self,
                ready,
                generation,
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

impl<T: AsRawFd> AsFd for AsyncFd<T> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: the descriptor remains owned by `self.io` for the returned
        // borrow, and `AsyncFd` deregisters it before the inner value is dropped.
        unsafe { BorrowedFd::borrow_raw(self.state.fd) }
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

/// Future returned by mutable AsyncFd readiness methods.
#[must_use = "futures do nothing unless polled"]
pub struct AsyncFdReadyMut<'a, T: AsRawFd> {
    fd: Option<&'a mut AsyncFd<T>>,
    interest: Interest,
}

impl<'a, T: AsRawFd> Future for AsyncFdReadyMut<'a, T> {
    type Output = io::Result<AsyncFdReadyMutGuard<'a, T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let readiness = this
            .fd
            .as_deref()
            .expect("AsyncFdReadyMut polled after completion")
            .state
            .poll_ready(cx, this.interest);
        match readiness {
            Poll::Ready(Ok((ready, generation))) => Poll::Ready(Ok(AsyncFdReadyMutGuard {
                fd: this.fd.take().expect("AsyncFdReadyMut inner missing"),
                ready,
                generation,
                cleared: false,
            })),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
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
    generation: u32,
    cleared: bool,
}

impl<T: AsRawFd> AsyncFdReadyGuard<'_, T> {
    /// Returns the observed readiness flags.
    pub fn ready(&self) -> Ready {
        self.ready
    }

    /// Clears the observed readiness and rearms the one-shot registration.
    pub fn clear_ready(&mut self) {
        self.fd.state.clear_ready(self.ready, self.generation);
        self.cleared = true;
    }

    /// Explicitly retains readiness. This is intentionally a no-op.
    pub fn retain_ready(&mut self) {}

    /// Runs a non-blocking I/O operation.
    ///
    /// `WouldBlock` clears the readiness and returns [`TryIoError`], allowing
    /// the caller to wait for a fresh edge.
    pub fn try_io<R>(
        &mut self,
        f: impl FnOnce(&T) -> io::Result<R>,
    ) -> Result<io::Result<R>, TryIoError> {
        match f(self.fd.get_ref()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.fd.state.clear_ready(self.ready, self.generation);
                self.cleared = true;
                Err(TryIoError(()))
            }
            result => Ok(result),
        }
    }
}

/// A mutable readiness observation for an [`AsyncFd`].
pub struct AsyncFdReadyMutGuard<'a, T: AsRawFd> {
    fd: &'a mut AsyncFd<T>,
    ready: Ready,
    generation: u32,
    cleared: bool,
}

impl<T: AsRawFd> AsyncFdReadyMutGuard<'_, T> {
    pub fn ready(&self) -> Ready {
        self.ready
    }

    pub fn clear_ready(&mut self) {
        self.fd.state.clear_ready(self.ready, self.generation);
        self.cleared = true;
    }

    pub fn retain_ready(&mut self) {}

    pub fn get_inner_mut(&mut self) -> &mut T {
        self.fd.get_mut()
    }

    pub fn try_io<R>(
        &mut self,
        f: impl FnOnce(&mut T) -> io::Result<R>,
    ) -> Result<io::Result<R>, TryIoError> {
        match f(self.fd.get_mut()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.fd.state.clear_ready(self.ready, self.generation);
                self.cleared = true;
                Err(TryIoError(()))
            }
            result => Ok(result),
        }
    }
}

/// Registration error that preserves ownership of the submitted I/O object.
#[derive(Debug)]
pub struct AsyncFdTryNewError<T> {
    inner: T,
    error: io::Error,
}

impl<T> AsyncFdTryNewError<T> {
    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn error(&self) -> &io::Error {
        &self.error
    }

    pub fn into_error(self) -> io::Error {
        self.error
    }
}

impl<T> fmt::Display for AsyncFdTryNewError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl<T: fmt::Debug> std::error::Error for AsyncFdTryNewError<T> {}

/// Error indicating that an operation still would have blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TryIoError(());

impl fmt::Display for TryIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operation would block")
    }
}

impl std::error::Error for TryIoError {}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn stale_callbacks_cannot_access_dropped_or_reused_registrations() {
        for _ in 0..128 {
            let (socket, _peer) = UnixStream::pair().unwrap();
            socket.set_nonblocking(true).unwrap();
            let fd = AsyncFd::new(socket).unwrap();
            let token = fd.state.token;
            drop(fd);
            assert!(!registrations().lock().unwrap().contains_key(&token));
            // This is exactly the data an already-fetched epoll batch retains.
            unsafe {
                readiness_callback(token as *mut c_void, EPOLLIN | EPOLLOUT);
            }
        }
    }
}

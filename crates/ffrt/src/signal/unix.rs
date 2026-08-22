//! OpenHarmony Unix signal streams integrated with the FFRT reactor.

use std::future::Future;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_void};
use std::os::unix::net::UnixStream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};

use crate::lock::Mutex;
use crate::reactor::{AsyncFd, Interest};

const MAX_SIGNALS: usize = 64;
const SIG_HUP: c_int = 1;
const SIG_INT: c_int = 2;
const SIG_QUIT: c_int = 3;
const SIG_KILL: c_int = 9;
const SIG_USR1: c_int = 10;
const SIG_PIPE: c_int = 13;
const SIG_ALRM: c_int = 14;
const SIG_TERM: c_int = 15;
const SIG_CHLD: c_int = 17;
const SIG_CONT: c_int = 18;
const SIG_STOP: c_int = 19;
const SIG_TSTP: c_int = 20;
const SIG_TTIN: c_int = 21;
const SIG_TTOU: c_int = 22;
const SIG_USR2: c_int = 12;
const SIG_IO: c_int = 29;
const SIG_WINCH: c_int = 28;

static SIGNAL_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

unsafe extern "C" {
    #[link_name = "signal"]
    fn c_signal(signal: c_int, handler: usize) -> usize;
    fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
}

extern "C" fn signal_handler(signal: c_int) {
    let fd = SIGNAL_WRITE_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    let byte = signal as u8;
    // write(2) is async-signal-safe. A full socket is deliberately ignored;
    // standard Unix signals are coalescing notifications as well.
    let _ = unsafe { write(fd, (&byte as *const u8).cast::<c_void>(), 1) };
}

/// Identifies a Unix signal number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignalKind(c_int);

impl SignalKind {
    /// Creates a signal kind from a raw platform signal number.
    pub const fn from_raw(signal: c_int) -> Self {
        Self(signal)
    }

    /// Returns the raw platform signal number.
    pub const fn as_raw_value(self) -> c_int {
        self.0
    }

    pub const fn alarm() -> Self {
        Self(SIG_ALRM)
    }

    pub const fn child() -> Self {
        Self(SIG_CHLD)
    }

    pub const fn hangup() -> Self {
        Self(SIG_HUP)
    }

    pub const fn interrupt() -> Self {
        Self(SIG_INT)
    }

    pub const fn io() -> Self {
        Self(SIG_IO)
    }

    pub const fn pipe() -> Self {
        Self(SIG_PIPE)
    }

    pub const fn quit() -> Self {
        Self(SIG_QUIT)
    }

    pub const fn terminate() -> Self {
        Self(SIG_TERM)
    }

    pub const fn user_defined1() -> Self {
        Self(SIG_USR1)
    }

    pub const fn user_defined2() -> Self {
        Self(SIG_USR2)
    }

    pub const fn window_change() -> Self {
        Self(SIG_WINCH)
    }
}

struct SignalDriver {
    reader: AsyncFd<UnixStream>,
    _writer: UnixStream,
    installed: [AtomicBool; MAX_SIGNALS],
    generation: [AtomicU64; MAX_SIGNALS],
    waiters: Mutex<Vec<Vec<Waker>>>,
}

impl SignalDriver {
    fn new() -> io::Result<Arc<Self>> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        SIGNAL_WRITE_FD.store(writer.as_raw_fd(), Ordering::Release);
        Ok(Arc::new(Self {
            reader: AsyncFd::with_interest(reader, Interest::READABLE)?,
            _writer: writer,
            installed: std::array::from_fn(|_| AtomicBool::new(false)),
            generation: std::array::from_fn(|_| AtomicU64::new(0)),
            waiters: Mutex::new((0..MAX_SIGNALS).map(|_| Vec::new()).collect()),
        }))
    }

    fn install(&self, signal: c_int) -> io::Result<()> {
        if signal <= 0 || signal as usize >= MAX_SIGNALS || matches!(signal, SIG_KILL | SIG_STOP) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "signal cannot be intercepted",
            ));
        }

        let installed = &self.installed[signal as usize];
        if installed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        let previous = unsafe { c_signal(signal, signal_handler as *const () as usize) };
        if previous == usize::MAX {
            installed.store(false, Ordering::Release);
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn drain(&self, cx: &mut Context<'_>) -> io::Result<()> {
        loop {
            let mut signals = [0; 128];
            let mut reader = self.reader.get_ref();
            match reader.read(&mut signals) {
                Ok(amount) => {
                    for signal in &signals[..amount] {
                        let index = *signal as usize;
                        if index >= MAX_SIGNALS {
                            continue;
                        }
                        self.generation[index].fetch_add(1, Ordering::AcqRel);
                        let waiters = {
                            let mut all = self.waiters.lock().unwrap();
                            std::mem::take(&mut all[index])
                        };
                        for waker in waiters {
                            waker.wake();
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    match self.reader.poll_read_ready(cx) {
                        Poll::Ready(Ok(guard)) => {
                            guard.clear_ready();
                            continue;
                        }
                        Poll::Ready(Err(error)) => return Err(error),
                        Poll::Pending => return Ok(()),
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn register(&self, signal: usize, waker: &Waker) {
        let mut waiters = self.waiters.lock().unwrap();
        if let Some(existing) = waiters[signal]
            .iter_mut()
            .find(|existing| existing.will_wake(waker))
        {
            *existing = waker.clone();
        } else {
            waiters[signal].push(waker.clone());
        }
    }
}

fn driver() -> io::Result<&'static Arc<SignalDriver>> {
    static DRIVER: OnceLock<Result<Arc<SignalDriver>, Arc<io::Error>>> = OnceLock::new();
    match DRIVER.get_or_init(|| SignalDriver::new().map_err(Arc::new)) {
        Ok(driver) => Ok(driver),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

/// A stream-like listener for one Unix signal.
pub struct Signal {
    driver: Arc<SignalDriver>,
    signal: usize,
    seen: u64,
}

impl Signal {
    /// Receives the next signal occurrence.
    pub fn recv(&mut self) -> Recv<'_> {
        Recv { signal: self }
    }

    /// Polls for the next signal occurrence.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<()>> {
        if self.driver.drain(cx).is_err() {
            return Poll::Ready(None);
        }

        let generation = self.driver.generation[self.signal].load(Ordering::Acquire);
        if generation != self.seen {
            self.seen = self.seen.wrapping_add(1);
            return Poll::Ready(Some(()));
        }

        self.driver.register(self.signal, cx.waker());
        let generation = self.driver.generation[self.signal].load(Ordering::Acquire);
        if generation != self.seen {
            self.seen = self.seen.wrapping_add(1);
            Poll::Ready(Some(()))
        } else {
            Poll::Pending
        }
    }
}

/// Future returned by [`Signal::recv`].
#[must_use = "futures do nothing unless polled"]
pub struct Recv<'a> {
    signal: &'a mut Signal,
}

impl Future for Recv<'_> {
    type Output = Option<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().signal.poll_recv(cx)
    }
}

/// Creates a listener for `kind`.
pub fn signal(kind: SignalKind) -> io::Result<Signal> {
    let signal = kind.as_raw_value();
    let driver = driver()?.clone();
    driver.install(signal)?;
    let seen = driver.generation[signal as usize].load(Ordering::Acquire);
    Ok(Signal {
        driver,
        signal: signal as usize,
        seen,
    })
}

// Retain the familiar job-control constants for applications that construct
// kinds dynamically, and keep the constants checked by the compiler.
#[allow(dead_code)]
const JOB_CONTROL_SIGNALS: [c_int; 5] = [SIG_CONT, SIG_TSTP, SIG_TTIN, SIG_TTOU, SIG_CHLD];

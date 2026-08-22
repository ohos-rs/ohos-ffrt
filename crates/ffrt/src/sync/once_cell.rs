use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use crate::lock::Mutex as FfrtMutex;

struct OnceState {
    initializing: bool,
    waiters: VecDeque<Waker>,
}

/// A tokio-compatible cell that is initialized once, backed by an FFRT mutex.
pub struct OnceCell<T> {
    state: FfrtMutex<OnceState>,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send + Sync> Send for OnceCell<T> {}
unsafe impl<T: Send + Sync> Sync for OnceCell<T> {}

impl<T> OnceCell<T> {
    /// Creates an empty cell.
    pub fn new() -> Self {
        Self {
            state: FfrtMutex::new(OnceState {
                initializing: false,
                waiters: VecDeque::new(),
            }),
            value: UnsafeCell::new(None),
        }
    }

    /// Returns the initialized value, if any.
    pub fn get(&self) -> Option<&T> {
        // SAFETY: a set value is never moved or removed for the lifetime of the cell.
        unsafe { &*self.value.get() }.as_ref()
    }

    /// Sets the cell value.
    pub fn set(&self, value: T) -> Result<(), SetError<T>> {
        let mut state = self.state.lock().unwrap();
        if unsafe { &*self.value.get() }.is_some() {
            return Err(SetError(value));
        }

        unsafe { *self.value.get() = Some(value) };
        state.initializing = false;
        while let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
        Ok(())
    }

    /// Gets the value, initializing it with `init` if necessary.
    pub fn get_or_init<F>(&self, init: F) -> GetOrInitFuture<'_, T, F>
    where
        F: Future<Output = T>,
    {
        GetOrInitFuture {
            cell: self,
            init: Some(init),
            started: false,
        }
    }

    /// Gets the value, attempting to initialize it with `init`.
    pub fn get_or_try_init<F, E>(&self, init: F) -> GetOrTryInitFuture<'_, T, F>
    where
        F: Future<Output = Result<T, E>>,
    {
        GetOrTryInitFuture {
            cell: self,
            init: Some(init),
            started: false,
        }
    }

    /// Consumes the cell and returns its value.
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}

impl<T> Default for OnceCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned by [`OnceCell::set`] when the cell is already initialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetError<T>(pub T);

impl<T> fmt::Display for SetError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cell already initialized")
    }
}

impl<T: fmt::Debug> std::error::Error for SetError<T> {}

/// Future returned by [`OnceCell::get_or_init`].
#[must_use = "futures do nothing unless polled"]
pub struct GetOrInitFuture<'a, T, F> {
    cell: &'a OnceCell<T>,
    init: Option<F>,
    started: bool,
}

impl<'a, T, F> Future for GetOrInitFuture<'a, T, F>
where
    F: Future<Output = T>,
{
    type Output = &'a T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        if let Some(value) = this.cell.get() {
            // SAFETY: the value is stable for the remainder of the cell's life.
            return Poll::Ready(unsafe { &*(value as *const T) });
        }

        let mut state = this.cell.state.lock().unwrap();
        if state.initializing && !this.started {
            state.waiters.push_back(cx.waker().clone());
            return Poll::Pending;
        }
        if !state.initializing {
            state.initializing = true;
            this.started = true;
        }
        drop(state);

        let init = this
            .init
            .as_mut()
            .expect("initializer missing while polling");
        // SAFETY: `init` is pinned through this future and never moved after first poll.
        let init = unsafe { Pin::new_unchecked(init) };
        match init.poll(cx) {
            Poll::Ready(value) => {
                let mut state = this.cell.state.lock().unwrap();
                if unsafe { &*this.cell.value.get() }.is_none() {
                    unsafe { *this.cell.value.get() = Some(value) };
                }
                state.initializing = false;
                this.init = None;
                while let Some(waker) = state.waiters.pop_front() {
                    waker.wake();
                }
                drop(state);

                Poll::Ready(this.cell.get().expect("value was just initialized"))
            }
            Poll::Pending => {
                let mut state = this.cell.state.lock().unwrap();
                state.waiters.push_back(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Future returned by [`OnceCell::get_or_try_init`].
#[must_use = "futures do nothing unless polled"]
pub struct GetOrTryInitFuture<'a, T, F> {
    cell: &'a OnceCell<T>,
    init: Option<F>,
    started: bool,
}

impl<'a, T, F, E> Future for GetOrTryInitFuture<'a, T, F>
where
    F: Future<Output = Result<T, E>>,
{
    type Output = Result<&'a T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        if let Some(value) = this.cell.get() {
            return Poll::Ready(Ok(unsafe { &*(value as *const T) }));
        }

        let mut state = this.cell.state.lock().unwrap();
        if state.initializing && !this.started {
            state.waiters.push_back(cx.waker().clone());
            return Poll::Pending;
        }
        if !state.initializing {
            state.initializing = true;
            this.started = true;
        }
        drop(state);

        let init = this
            .init
            .as_mut()
            .expect("initializer missing while polling");
        let init = unsafe { Pin::new_unchecked(init) };
        match init.poll(cx) {
            Poll::Ready(Ok(value)) => {
                let mut state = this.cell.state.lock().unwrap();
                if unsafe { &*this.cell.value.get() }.is_none() {
                    unsafe { *this.cell.value.get() = Some(value) };
                }
                state.initializing = false;
                this.init = None;
                while let Some(waker) = state.waiters.pop_front() {
                    waker.wake();
                }
                drop(state);

                Poll::Ready(Ok(this.cell.get().expect("value was just initialized")))
            }
            Poll::Ready(Err(error)) => {
                let mut state = this.cell.state.lock().unwrap();
                state.initializing = false;
                this.init = None;
                while let Some(waker) = state.waiters.pop_front() {
                    waker.wake();
                }
                drop(state);

                Poll::Ready(Err(error))
            }
            Poll::Pending => {
                let mut state = this.cell.state.lock().unwrap();
                state.waiters.push_back(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_init_sets_value() {
        let cell = OnceCell::new();
        let result = crate::Runtime::new().block_on(async move {
            let value = cell.get_or_init(async { 42 }).await;
            assert_eq!(*value, 42);
            let again = cell.get_or_init(async { 0 }).await;
            assert_eq!(*again, 42);
            Ok::<(), crate::RuntimeError>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn set_rejects_duplicate() {
        let cell = OnceCell::new();
        assert!(cell.set(1).is_ok());
        assert!(cell.set(2).is_err());
    }
}

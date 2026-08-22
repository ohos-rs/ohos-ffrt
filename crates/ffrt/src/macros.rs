/// Polls one future and stores its output when it becomes ready.
///
/// This helper macro exists to keep the generated `join!`/`try_join!` code
/// readable.
#[macro_export]
#[doc(hidden)]
macro_rules! poll_one {
    ($done:ident, $out:ident, $fut:ident, $cx:ident) => {{
        if !$done {
            let future = unsafe { $fut.as_mut().get_unchecked_mut() }
                .as_mut()
                .expect("join future missing");
            let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
            if let ::std::task::Poll::Ready(output) = ::std::future::Future::poll(pinned, $cx) {
                $done = true;
                $out = Some(output);
            }
        }
    }};
}

/// Waits for all futures to complete and returns a tuple of their outputs.
#[macro_export]
macro_rules! join {
    ($fut1:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut done1 = false;
            let mut out1 = None;

            ::std::future::poll_fn(|cx| {
                $crate::poll_one!(done1, out1, fut1, cx);

                if done1 {
                    ::std::task::Poll::Ready((out1.take().expect("missing joined output"),))
                } else {
                    ::std::task::Poll::Pending
                }
            })
            .await
        }
    }};

    ($fut1:expr, $fut2:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut fut2 = ::std::pin::pin!(Some($fut2));
            let mut done1 = false;
            let mut done2 = false;
            let mut out1 = None;
            let mut out2 = None;

            ::std::future::poll_fn(|cx| {
                $crate::poll_one!(done1, out1, fut1, cx);
                $crate::poll_one!(done2, out2, fut2, cx);

                if done1 && done2 {
                    ::std::task::Poll::Ready((
                        out1.take().expect("missing joined output"),
                        out2.take().expect("missing joined output"),
                    ))
                } else {
                    ::std::task::Poll::Pending
                }
            })
            .await
        }
    }};

    ($fut1:expr, $fut2:expr, $fut3:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut fut2 = ::std::pin::pin!(Some($fut2));
            let mut fut3 = ::std::pin::pin!(Some($fut3));
            let mut done1 = false;
            let mut done2 = false;
            let mut done3 = false;
            let mut out1 = None;
            let mut out2 = None;
            let mut out3 = None;

            ::std::future::poll_fn(|cx| {
                $crate::poll_one!(done1, out1, fut1, cx);
                $crate::poll_one!(done2, out2, fut2, cx);
                $crate::poll_one!(done3, out3, fut3, cx);

                if done1 && done2 && done3 {
                    ::std::task::Poll::Ready((
                        out1.take().expect("missing joined output"),
                        out2.take().expect("missing joined output"),
                        out3.take().expect("missing joined output"),
                    ))
                } else {
                    ::std::task::Poll::Pending
                }
            })
            .await
        }
    }};
}

/// Waits for all `Result`-returning futures and short-circuits on the first error.
#[macro_export]
macro_rules! try_join {
    ($fut1:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut done1 = false;
            let mut out1 = None;

            ::std::future::poll_fn(|cx| {
                if !done1 {
                    let future = unsafe { fut1.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("join future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready(Ok(output)) => {
                            done1 = true;
                            out1 = Some(output);
                        }
                        ::std::task::Poll::Ready(Err(error)) => {
                            return ::std::task::Poll::Ready(Err(error));
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                if done1 {
                    ::std::task::Poll::Ready(Ok((out1.take().expect("missing joined output"),)))
                } else {
                    ::std::task::Poll::Pending
                }
            })
            .await
        }
    }};

    ($fut1:expr, $fut2:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut fut2 = ::std::pin::pin!(Some($fut2));
            let mut done1 = false;
            let mut done2 = false;
            let mut out1 = None;
            let mut out2 = None;

            ::std::future::poll_fn(|cx| {
                if !done1 {
                    let future = unsafe { fut1.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("join future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready(Ok(output)) => {
                            done1 = true;
                            out1 = Some(output);
                        }
                        ::std::task::Poll::Ready(Err(error)) => {
                            return ::std::task::Poll::Ready(Err(error));
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                if !done2 {
                    let future = unsafe { fut2.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("join future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready(Ok(output)) => {
                            done2 = true;
                            out2 = Some(output);
                        }
                        ::std::task::Poll::Ready(Err(error)) => {
                            return ::std::task::Poll::Ready(Err(error));
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                if done1 && done2 {
                    ::std::task::Poll::Ready(Ok((
                        out1.take().expect("missing joined output"),
                        out2.take().expect("missing joined output"),
                    )))
                } else {
                    ::std::task::Poll::Pending
                }
            })
            .await
        }
    }};
}

/// Pins a value on the stack.
#[macro_export]
macro_rules! pin {
    ($x:ident) => {
        let mut $x = $x;
        let mut $x = unsafe { ::std::pin::Pin::new_unchecked(&mut $x) };
    };
    ($x:ident = $init:expr) => {
        let mut $x = $init;
        let mut $x = unsafe { ::std::pin::Pin::new_unchecked(&mut $x) };
    };
}

/// Polls futures and returns the handler of the first ready branch.
#[macro_export]
macro_rules! select {
    ($pat1:pat = $fut1:expr => $handler1:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));

            ::std::future::poll_fn(|cx| {
                let future = unsafe { fut1.as_mut().get_unchecked_mut() }
                    .as_mut()
                    .expect("select future missing");
                let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                match ::std::future::Future::poll(pinned, cx) {
                    ::std::task::Poll::Ready($pat1) => {
                        return ::std::task::Poll::Ready($handler1);
                    }
                    ::std::task::Poll::Pending => {}
                }

                ::std::task::Poll::Pending
            })
            .await
        }
    }};

    ($pat1:pat = $fut1:expr => $handler1:expr, $pat2:pat = $fut2:expr => $handler2:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut fut2 = ::std::pin::pin!(Some($fut2));

            ::std::future::poll_fn(|cx| {
                {
                    let future = unsafe { fut1.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("select future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready($pat1) => {
                            return ::std::task::Poll::Ready($handler1);
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                {
                    let future = unsafe { fut2.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("select future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready($pat2) => {
                            return ::std::task::Poll::Ready($handler2);
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                ::std::task::Poll::Pending
            })
            .await
        }
    }};

    ($pat1:pat = $fut1:expr => $handler1:expr, $pat2:pat = $fut2:expr => $handler2:expr, $pat3:pat = $fut3:expr => $handler3:expr $(,)?) => {{
        async move {
            let mut fut1 = ::std::pin::pin!(Some($fut1));
            let mut fut2 = ::std::pin::pin!(Some($fut2));
            let mut fut3 = ::std::pin::pin!(Some($fut3));

            ::std::future::poll_fn(|cx| {
                {
                    let future = unsafe { fut1.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("select future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready($pat1) => {
                            return ::std::task::Poll::Ready($handler1);
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                {
                    let future = unsafe { fut2.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("select future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready($pat2) => {
                            return ::std::task::Poll::Ready($handler2);
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                {
                    let future = unsafe { fut3.as_mut().get_unchecked_mut() }
                        .as_mut()
                        .expect("select future missing");
                    let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                    match ::std::future::Future::poll(pinned, cx) {
                        ::std::task::Poll::Ready($pat3) => {
                            return ::std::task::Poll::Ready($handler3);
                        }
                        ::std::task::Poll::Pending => {}
                    }
                }

                ::std::task::Poll::Pending
            })
            .await
        }
    }};
}

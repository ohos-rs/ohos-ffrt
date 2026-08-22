/// Declares one or more scoped task-local keys.
#[macro_export]
macro_rules! task_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty; $($rest:tt)*) => {
        $(#[$attr])*
        $vis static $name: $crate::task::LocalKey<$ty> = {
            fn __ffrt_task_local_get() -> *const $crate::task::LocalKeyInner<$ty> {
                ::std::thread_local! {
                    static VALUE: $crate::task::LocalKeyInner<$ty> =
                        $crate::task::LocalKeyInner::new();
                }
                VALUE.with(|value| value as *const $crate::task::LocalKeyInner<$ty>)
            }
            $crate::task::LocalKey::new(__ffrt_task_local_get)
        };
        $crate::task_local! { $($rest)* }
    };
}

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
    (biased; $($branches:tt)*) => {
        $crate::select!(@dispatch biased; $($branches)*)
    };

    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr,
        $p3:pat = $f3:expr => $h3:expr,
        $p4:pat = $f4:expr => $h4:expr,
        $p5:pat = $f5:expr => $h5:expr,
        $p6:pat = $f6:expr => $h6:expr,
        $p7:pat = $f7:expr => $h7:expr,
        $p8:pat = $f8:expr => $h8:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 8;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2),
            (2, future3, $p3, $f3, $h3), (3, future4, $p4, $f4, $h4),
            (4, future5, $p5, $f5, $h5), (5, future6, $p6, $f6, $h6),
            (6, future7, $p7, $f7, $h7), (7, future8, $p8, $f8, $h8))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr,
        $p3:pat = $f3:expr => $h3:expr,
        $p4:pat = $f4:expr => $h4:expr,
        $p5:pat = $f5:expr => $h5:expr,
        $p6:pat = $f6:expr => $h6:expr,
        $p7:pat = $f7:expr => $h7:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 7;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2),
            (2, future3, $p3, $f3, $h3), (3, future4, $p4, $f4, $h4),
            (4, future5, $p5, $f5, $h5), (5, future6, $p6, $f6, $h6),
            (6, future7, $p7, $f7, $h7))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr,
        $p3:pat = $f3:expr => $h3:expr,
        $p4:pat = $f4:expr => $h4:expr,
        $p5:pat = $f5:expr => $h5:expr,
        $p6:pat = $f6:expr => $h6:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 6;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2),
            (2, future3, $p3, $f3, $h3), (3, future4, $p4, $f4, $h4),
            (4, future5, $p5, $f5, $h5), (5, future6, $p6, $f6, $h6))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr,
        $p3:pat = $f3:expr => $h3:expr,
        $p4:pat = $f4:expr => $h4:expr,
        $p5:pat = $f5:expr => $h5:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 5;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2),
            (2, future3, $p3, $f3, $h3), (3, future4, $p4, $f4, $h4),
            (4, future5, $p5, $f5, $h5))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr,
        $p3:pat = $f3:expr => $h3:expr,
        $p4:pat = $f4:expr => $h4:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 4;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2),
            (2, future3, $p3, $f3, $h3), (3, future4, $p4, $f4, $h4))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr,
        $p3:pat = $f3:expr => $h3:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 3;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2),
            (2, future3, $p3, $f3, $h3))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr => $h1:expr,
        $p2:pat = $f2:expr => $h2:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 2;
            (0, future1, $p1, $f1, $h1), (1, future2, $p2, $f2, $h2))
    };
    (@dispatch $mode:ident; $p1:pat = $f1:expr => $h1:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 1; (0, future1, $p1, $f1, $h1))
    };
    (@dispatch $mode:ident; $($invalid:tt)*) => {
        compile_error!("select! supports between one and eight branches")
    };
    ($($branches:tt)*) => {
        $crate::select!(@dispatch fair; $($branches)*)
    };
}

/// Implementation detail for [`select!`].
#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_run {
    ($mode:ident; $count:expr; $(($index:pat, $name:ident, $pat:pat, $future:expr, $handler:expr)),+) => {{
        async move {
            $(let mut $name = ::std::pin::pin!(Some($future));)+
            let mut active = $count;
            let start = $crate::__ffrt_select_mode_start!($mode, $count);
            ::std::future::poll_fn(move |cx| {
                for offset in 0..$count {
                    match (start + offset) % $count {
                        $($index => {
                            if $name.as_ref().get_ref().is_none() {
                                continue;
                            }
                            let result = {
                                let future = unsafe { $name.as_mut().get_unchecked_mut() }
                                    .as_mut()
                                    .expect("select future missing");
                                let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
                                ::std::future::Future::poll(pinned, cx)
                            };
                            if let ::std::task::Poll::Ready(output) = result {
                                $name.as_mut().set(None);
                                active -= 1;
                                #[allow(unreachable_patterns)]
                                match output {
                                    $pat => return ::std::task::Poll::Ready($handler),
                                    _ => {}
                                }
                            }
                        },)+
                        _ => unreachable!("select branch index out of range"),
                    }
                }
                if active == 0 {
                    panic!("all select branches were disabled");
                }
                ::std::task::Poll::Pending
            })
            .await
        }
    }};
}

/// Selects the first branch for biased or fair polling.
#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_mode_start {
    (biased, $count:expr) => {
        0usize
    };
    (fair, $count:expr) => {
        $crate::__select_start($count)
    };
}

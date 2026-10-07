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

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_join_run {
    ($(($future:ident, $done:ident, $out:ident, $expr:expr)),+ $(,)?) => {{
        {
            $(let mut $future = ::std::pin::pin!(Some($expr));)+
            $(let mut $done = false;)+
            $(let mut $out = None;)+
            ::std::future::poll_fn(|cx| {
                $($crate::poll_one!($done, $out, $future, cx);)+
                if true $(&& $done)+ {
                    ::std::task::Poll::Ready(($(
                        $out.take().expect("missing joined output"),
                    )+))
                } else {
                    ::std::task::Poll::Pending
                }
            }).await
        }
    }};
}

/// Waits for all futures to complete and returns a tuple of their outputs.
#[macro_export]
macro_rules! join {
    ($f1:expr $(,)?) => {
        $crate::__ffrt_join_run!((future1, done1, out1, $f1))
    };
    ($f1:expr, $f2:expr $(,)?) => {
        $crate::__ffrt_join_run!((future1, done1, out1, $f1), (future2, done2, out2, $f2))
    };
    ($f1:expr, $f2:expr, $f3:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr, $f14:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13),
            (future14, done14, out14, $f14)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr, $f14:expr, $f15:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13),
            (future14, done14, out14, $f14),
            (future15, done15, out15, $f15)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr, $f14:expr, $f15:expr, $f16:expr $(,)?) => {
        $crate::__ffrt_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13),
            (future14, done14, out14, $f14),
            (future15, done15, out15, $f15),
            (future16, done16, out16, $f16)
        )
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_try_poll_one {
    ($done:ident, $out:ident, $future:ident, $cx:ident) => {{
        if !$done {
            let future = unsafe { $future.as_mut().get_unchecked_mut() }
                .as_mut()
                .expect("try_join future missing");
            let pinned = unsafe { ::std::pin::Pin::new_unchecked(future) };
            match ::std::future::Future::poll(pinned, $cx) {
                ::std::task::Poll::Ready(Ok(output)) => {
                    $done = true;
                    $out = Some(output);
                }
                ::std::task::Poll::Ready(Err(error)) => {
                    return ::std::task::Poll::Ready(Err(error));
                }
                ::std::task::Poll::Pending => {}
            }
        }
    }};
}

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_try_join_run {
    ($(($future:ident, $done:ident, $out:ident, $expr:expr)),+ $(,)?) => {{
        {
            $(let mut $future = ::std::pin::pin!(Some($expr));)+
            $(let mut $done = false;)+
            $(let mut $out = None;)+
            ::std::future::poll_fn(|cx| {
                $($crate::__ffrt_try_poll_one!($done, $out, $future, cx);)+
                if true $(&& $done)+ {
                    ::std::task::Poll::Ready(Ok(($(
                        $out.take().expect("missing try_join output"),
                    )+)))
                } else {
                    ::std::task::Poll::Pending
                }
            }).await
        }
    }};
}

/// Waits for all Result-returning futures and short-circuits on the first error.
#[macro_export]
macro_rules! try_join {
    ($f1:expr $(,)?) => {
        $crate::__ffrt_try_join_run!((future1, done1, out1, $f1))
    };
    ($f1:expr, $f2:expr $(,)?) => {
        $crate::__ffrt_try_join_run!((future1, done1, out1, $f1), (future2, done2, out2, $f2))
    };
    ($f1:expr, $f2:expr, $f3:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr, $f14:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13),
            (future14, done14, out14, $f14)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr, $f14:expr, $f15:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13),
            (future14, done14, out14, $f14),
            (future15, done15, out15, $f15)
        )
    };
    ($f1:expr, $f2:expr, $f3:expr, $f4:expr, $f5:expr, $f6:expr, $f7:expr, $f8:expr, $f9:expr, $f10:expr, $f11:expr, $f12:expr, $f13:expr, $f14:expr, $f15:expr, $f16:expr $(,)?) => {
        $crate::__ffrt_try_join_run!(
            (future1, done1, out1, $f1),
            (future2, done2, out2, $f2),
            (future3, done3, out3, $f3),
            (future4, done4, out4, $f4),
            (future5, done5, out5, $f5),
            (future6, done6, out6, $f6),
            (future7, done7, out7, $f7),
            (future8, done8, out8, $f8),
            (future9, done9, out9, $f9),
            (future10, done10, out10, $f10),
            (future11, done11, out11, $f11),
            (future12, done12, out12, $f12),
            (future13, done13, out13, $f13),
            (future14, done14, out14, $f14),
            (future15, done15, out15, $f15),
            (future16, done16, out16, $f16)
        )
    };
}

/// Pins one or more local variables on the stack.
#[macro_export]
macro_rules! pin {
    ($($x:ident),+ $(,)?) => {
        $(let mut $x = $x;
        #[allow(unused_mut)]
        let mut $x = unsafe { ::std::pin::Pin::new_unchecked(&mut $x) };)+
    };
    ($x:ident = $init:expr) => {
        let mut $x = $init;
        #[allow(unused_mut)]
        let mut $x = unsafe { ::std::pin::Pin::new_unchecked(&mut $x) };
    };
}

/// Waits for the first matching branch; handlers execute in the caller's scope.
#[macro_export]
macro_rules! select {
    ($($branches:tt)*) => {
        $crate::__select!(($crate::__select_start); $($branches)*)
    };
}

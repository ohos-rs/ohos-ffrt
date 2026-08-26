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
        async move {
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
        }.await
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
        async move {
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
        }.await
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

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_enabled {
    () => {
        true
    };
    ($condition:expr) => {
        $condition
    };
}

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

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_finish {
    (no_else) => {
        panic!("all select branches were disabled")
    };
    (else $handler:expr) => {
        return ::std::task::Poll::Ready($handler)
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_run {
    ($mode:ident; $count:expr; $finish:tt $( $else_handler:expr)?;
        $(($index:pat, $name:ident, $enabled:ident, $condition:expr,
           $pattern:pat, $future:expr, $handler:expr)),+
    ) => {{
        async move {
            $(let mut $enabled = $condition;
            let mut $name = ::std::pin::pin!(Some($future));)+
            let mut active = 0usize $(+ usize::from($enabled))+;
            let start = $crate::__ffrt_select_mode_start!($mode, $count);
            ::std::future::poll_fn(move |cx| {
                for offset in 0..$count {
                    match $crate::__select_index(start, offset, $count) {
                        $($index => {
                            if !$enabled || $name.as_ref().get_ref().is_none() {
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
                                $enabled = false;
                                active -= 1;
                                #[allow(unreachable_patterns)]
                                match output {
                                    $pattern => return ::std::task::Poll::Ready($handler),
                                    _ => {}
                                }
                            }
                        },)+
                        _ => unreachable!("select branch index out of range"),
                    }
                }
                if active == 0 {
                    $crate::__ffrt_select_finish!($finish $( $else_handler)?);
                }
                ::std::task::Poll::Pending
            }).await
        }.await
    }};
}

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_count {
    ($(($index:pat, $name:ident, $enabled:ident, $condition:expr,
        $pattern:pat, $future:expr, $handler:expr)),+ $(,)?) => {
        0usize $(+ { let _ = stringify!($index); 1usize })+
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_emit {
    ($mode:ident; no_else;
        $(($index:pat, $name:ident, $enabled:ident, $condition:expr,
           $pattern:pat, $future:expr, $handler:expr)),+ $(,)?) => {
        $crate::__ffrt_select_run!(
            $mode;
            $crate::__ffrt_select_count!($(($index, $name, $enabled, $condition,
                $pattern, $future, $handler)),+);
            no_else;
            $(($index, $name, $enabled, $condition, $pattern, $future, $handler)),+
        )
    };
    ($mode:ident; else $else_handler:expr;
        $(($index:pat, $name:ident, $enabled:ident, $condition:expr,
           $pattern:pat, $future:expr, $handler:expr)),+ $(,)?) => {
        $crate::__ffrt_select_run!(
            $mode;
            $crate::__ffrt_select_count!($(($index, $name, $enabled, $condition,
                $pattern, $future, $handler)),+);
            else $else_handler;
            $(($index, $name, $enabled, $condition, $pattern, $future, $handler)),+
        )
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __ffrt_select_collect {
    ($mode:ident; [$($slots:tt)*]; [$($ready:tt)+];
        else => $else_handler:expr $(,)?) => {
        $crate::__ffrt_select_emit!($mode; else $else_handler; $($ready)*)
    };
    ($mode:ident; [($index:literal, $name:ident, $enabled:ident) $($slots:tt)*];
        [$($ready:tt)*];
        $pattern:pat = $future:expr $(, if $condition:expr)? => $handler:expr,
        $($rest:tt)+) => {
        $crate::__ffrt_select_collect!(
            $mode;
            [$($slots)*];
            [$($ready)* ($index, $name, $enabled,
                $crate::__ffrt_select_enabled!($($condition)?),
                $pattern, $future, $handler),];
            $($rest)+
        )
    };
    ($mode:ident; [($index:literal, $name:ident, $enabled:ident) $($slots:tt)*];
        [$($ready:tt)*];
        $pattern:pat = $future:expr $(, if $condition:expr)? => $handler:expr $(,)?) => {
        $crate::__ffrt_select_emit!(
            $mode;
            no_else;
            $($ready)*
            ($index, $name, $enabled, $crate::__ffrt_select_enabled!($($condition)?),
                $pattern, $future, $handler),
        )
    };
    ($mode:ident; []; [$($ready:tt)*]; $($rest:tt)*) => {
        compile_error!("select! supports at most 64 branches")
    };
    ($mode:ident; $slots:tt; []; $($rest:tt)*) => {
        compile_error!("select! requires at least one branch")
    };
    ($mode:ident; $slots:tt; [$($ready:tt)+]; $($rest:tt)*) => {
        compile_error!("invalid select! syntax")
    };
}

#[macro_export]
macro_rules! select {
    (biased; $($branches:tt)*) => {
        $crate::select!(@dispatch biased; $($branches)*)
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 1; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 1; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 2; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 2; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 3; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 3; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 4; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 4; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 5; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 5; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 6; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 6; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 7; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 7; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 8; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 8; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 9; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 9; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 10; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 10; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 11; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 11; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 12; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 12; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 13; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 13; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        $p14:pat = $f14:expr $(, if $c14:expr)? => $h14:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 14; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13),
            (13, future14, enabled14, $crate::__ffrt_select_enabled!($($c14)?), $p14, $f14, $h14))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        $p14:pat = $f14:expr $(, if $c14:expr)? => $h14:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 14; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13),
            (13, future14, enabled14, $crate::__ffrt_select_enabled!($($c14)?), $p14, $f14, $h14))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        $p14:pat = $f14:expr $(, if $c14:expr)? => $h14:expr,
        $p15:pat = $f15:expr $(, if $c15:expr)? => $h15:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 15; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13),
            (13, future14, enabled14, $crate::__ffrt_select_enabled!($($c14)?), $p14, $f14, $h14),
            (14, future15, enabled15, $crate::__ffrt_select_enabled!($($c15)?), $p15, $f15, $h15))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        $p14:pat = $f14:expr $(, if $c14:expr)? => $h14:expr,
        $p15:pat = $f15:expr $(, if $c15:expr)? => $h15:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 15; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13),
            (13, future14, enabled14, $crate::__ffrt_select_enabled!($($c14)?), $p14, $f14, $h14),
            (14, future15, enabled15, $crate::__ffrt_select_enabled!($($c15)?), $p15, $f15, $h15))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        $p14:pat = $f14:expr $(, if $c14:expr)? => $h14:expr,
        $p15:pat = $f15:expr $(, if $c15:expr)? => $h15:expr,
        $p16:pat = $f16:expr $(, if $c16:expr)? => $h16:expr,
        else => $else:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 16; else $else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13),
            (13, future14, enabled14, $crate::__ffrt_select_enabled!($($c14)?), $p14, $f14, $h14),
            (14, future15, enabled15, $crate::__ffrt_select_enabled!($($c15)?), $p15, $f15, $h15),
            (15, future16, enabled16, $crate::__ffrt_select_enabled!($($c16)?), $p16, $f16, $h16))
    };
    (@dispatch $mode:ident;
        $p1:pat = $f1:expr $(, if $c1:expr)? => $h1:expr,
        $p2:pat = $f2:expr $(, if $c2:expr)? => $h2:expr,
        $p3:pat = $f3:expr $(, if $c3:expr)? => $h3:expr,
        $p4:pat = $f4:expr $(, if $c4:expr)? => $h4:expr,
        $p5:pat = $f5:expr $(, if $c5:expr)? => $h5:expr,
        $p6:pat = $f6:expr $(, if $c6:expr)? => $h6:expr,
        $p7:pat = $f7:expr $(, if $c7:expr)? => $h7:expr,
        $p8:pat = $f8:expr $(, if $c8:expr)? => $h8:expr,
        $p9:pat = $f9:expr $(, if $c9:expr)? => $h9:expr,
        $p10:pat = $f10:expr $(, if $c10:expr)? => $h10:expr,
        $p11:pat = $f11:expr $(, if $c11:expr)? => $h11:expr,
        $p12:pat = $f12:expr $(, if $c12:expr)? => $h12:expr,
        $p13:pat = $f13:expr $(, if $c13:expr)? => $h13:expr,
        $p14:pat = $f14:expr $(, if $c14:expr)? => $h14:expr,
        $p15:pat = $f15:expr $(, if $c15:expr)? => $h15:expr,
        $p16:pat = $f16:expr $(, if $c16:expr)? => $h16:expr $(,)?) => {
        $crate::__ffrt_select_run!($mode; 16; no_else;
            (0, future1, enabled1, $crate::__ffrt_select_enabled!($($c1)?), $p1, $f1, $h1),
            (1, future2, enabled2, $crate::__ffrt_select_enabled!($($c2)?), $p2, $f2, $h2),
            (2, future3, enabled3, $crate::__ffrt_select_enabled!($($c3)?), $p3, $f3, $h3),
            (3, future4, enabled4, $crate::__ffrt_select_enabled!($($c4)?), $p4, $f4, $h4),
            (4, future5, enabled5, $crate::__ffrt_select_enabled!($($c5)?), $p5, $f5, $h5),
            (5, future6, enabled6, $crate::__ffrt_select_enabled!($($c6)?), $p6, $f6, $h6),
            (6, future7, enabled7, $crate::__ffrt_select_enabled!($($c7)?), $p7, $f7, $h7),
            (7, future8, enabled8, $crate::__ffrt_select_enabled!($($c8)?), $p8, $f8, $h8),
            (8, future9, enabled9, $crate::__ffrt_select_enabled!($($c9)?), $p9, $f9, $h9),
            (9, future10, enabled10, $crate::__ffrt_select_enabled!($($c10)?), $p10, $f10, $h10),
            (10, future11, enabled11, $crate::__ffrt_select_enabled!($($c11)?), $p11, $f11, $h11),
            (11, future12, enabled12, $crate::__ffrt_select_enabled!($($c12)?), $p12, $f12, $h12),
            (12, future13, enabled13, $crate::__ffrt_select_enabled!($($c13)?), $p13, $f13, $h13),
            (13, future14, enabled14, $crate::__ffrt_select_enabled!($($c14)?), $p14, $f14, $h14),
            (14, future15, enabled15, $crate::__ffrt_select_enabled!($($c15)?), $p15, $f15, $h15),
            (15, future16, enabled16, $crate::__ffrt_select_enabled!($($c16)?), $p16, $f16, $h16))
    };
    (@dispatch $mode:ident; $($branches:tt)*) => {
        $crate::__ffrt_select_collect!(
            $mode;
            [
                (0, __ffrt_future0, __ffrt_enabled0)
                (1, __ffrt_future1, __ffrt_enabled1)
                (2, __ffrt_future2, __ffrt_enabled2)
                (3, __ffrt_future3, __ffrt_enabled3)
                (4, __ffrt_future4, __ffrt_enabled4)
                (5, __ffrt_future5, __ffrt_enabled5)
                (6, __ffrt_future6, __ffrt_enabled6)
                (7, __ffrt_future7, __ffrt_enabled7)
                (8, __ffrt_future8, __ffrt_enabled8)
                (9, __ffrt_future9, __ffrt_enabled9)
                (10, __ffrt_future10, __ffrt_enabled10)
                (11, __ffrt_future11, __ffrt_enabled11)
                (12, __ffrt_future12, __ffrt_enabled12)
                (13, __ffrt_future13, __ffrt_enabled13)
                (14, __ffrt_future14, __ffrt_enabled14)
                (15, __ffrt_future15, __ffrt_enabled15)
                (16, __ffrt_future16, __ffrt_enabled16)
                (17, __ffrt_future17, __ffrt_enabled17)
                (18, __ffrt_future18, __ffrt_enabled18)
                (19, __ffrt_future19, __ffrt_enabled19)
                (20, __ffrt_future20, __ffrt_enabled20)
                (21, __ffrt_future21, __ffrt_enabled21)
                (22, __ffrt_future22, __ffrt_enabled22)
                (23, __ffrt_future23, __ffrt_enabled23)
                (24, __ffrt_future24, __ffrt_enabled24)
                (25, __ffrt_future25, __ffrt_enabled25)
                (26, __ffrt_future26, __ffrt_enabled26)
                (27, __ffrt_future27, __ffrt_enabled27)
                (28, __ffrt_future28, __ffrt_enabled28)
                (29, __ffrt_future29, __ffrt_enabled29)
                (30, __ffrt_future30, __ffrt_enabled30)
                (31, __ffrt_future31, __ffrt_enabled31)
                (32, __ffrt_future32, __ffrt_enabled32)
                (33, __ffrt_future33, __ffrt_enabled33)
                (34, __ffrt_future34, __ffrt_enabled34)
                (35, __ffrt_future35, __ffrt_enabled35)
                (36, __ffrt_future36, __ffrt_enabled36)
                (37, __ffrt_future37, __ffrt_enabled37)
                (38, __ffrt_future38, __ffrt_enabled38)
                (39, __ffrt_future39, __ffrt_enabled39)
                (40, __ffrt_future40, __ffrt_enabled40)
                (41, __ffrt_future41, __ffrt_enabled41)
                (42, __ffrt_future42, __ffrt_enabled42)
                (43, __ffrt_future43, __ffrt_enabled43)
                (44, __ffrt_future44, __ffrt_enabled44)
                (45, __ffrt_future45, __ffrt_enabled45)
                (46, __ffrt_future46, __ffrt_enabled46)
                (47, __ffrt_future47, __ffrt_enabled47)
                (48, __ffrt_future48, __ffrt_enabled48)
                (49, __ffrt_future49, __ffrt_enabled49)
                (50, __ffrt_future50, __ffrt_enabled50)
                (51, __ffrt_future51, __ffrt_enabled51)
                (52, __ffrt_future52, __ffrt_enabled52)
                (53, __ffrt_future53, __ffrt_enabled53)
                (54, __ffrt_future54, __ffrt_enabled54)
                (55, __ffrt_future55, __ffrt_enabled55)
                (56, __ffrt_future56, __ffrt_enabled56)
                (57, __ffrt_future57, __ffrt_enabled57)
                (58, __ffrt_future58, __ffrt_enabled58)
                (59, __ffrt_future59, __ffrt_enabled59)
                (60, __ffrt_future60, __ffrt_enabled60)
                (61, __ffrt_future61, __ffrt_enabled61)
                (62, __ffrt_future62, __ffrt_enabled62)
                (63, __ffrt_future63, __ffrt_enabled63)
            ];
            [];
            $($branches)*
        )
    };
    ($($branches:tt)*) => {
        $crate::select!(@dispatch fair; $($branches)*)
    };
}

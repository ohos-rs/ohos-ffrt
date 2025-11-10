pub mod sync {
    use ohos_ffrt_sys::{ffrt_error_t_ffrt_success, ffrt_usleep};
    use std::time::Duration;

    /// Sleep for a given duration
    pub fn sleep(duration: Duration) {
        let ret = unsafe { ffrt_usleep(duration.as_micros() as u64) };

        #[cfg(debug_assertions)]
        assert!(ret == ffrt_error_t_ffrt_success, "Failed to sleep");
    }
}

pub mod r#async {
    use ohos_ffrt_sys::ffrt_usleep;
    use std::{
        pin::Pin,
        task::{Context, Poll},
        time::{Duration, Instant},
    };

    /// sleep for a given duration
    pub async fn sleep(duration: Duration) {
        struct Sleep {
            deadline: Instant,
        }

        impl Future for Sleep {
            type Output = ();

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                if Instant::now() >= self.deadline {
                    Poll::Ready(())
                } else {
                    let remaining = self.deadline - Instant::now();
                    unsafe {
                        ffrt_usleep(remaining.as_micros() as u64);
                    }
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }

        Sleep {
            deadline: Instant::now() + duration,
        }
        .await
    }
}

//! Regression coverage for cancellation, ownership, and macro compatibility.
use std::cell::Cell;
use std::future::{Future, pending, ready};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use ffrt::sync::{self, mpsc, watch};
use static_assertions::{assert_impl_all, assert_not_impl_any};

assert_impl_all!(sync::MutexGuard<'static, Cell<u8>>: Send);
assert_impl_all!(sync::OwnedMutexGuard<Cell<u8>>: Send);
assert_impl_all!(sync::MutexGuard<'static, u8>: Send, Sync);
assert_impl_all!(sync::OwnedMutexGuard<u8>: Send, Sync);
assert_not_impl_any!(sync::MutexGuard<'static, Cell<u8>>: Sync);
assert_not_impl_any!(sync::OwnedMutexGuard<Cell<u8>>: Sync);
assert_not_impl_any!(ffrt::lock::MutexGuard<'static, Cell<u8>>: Sync);
assert_not_impl_any!(watch::Ref<'static, Cell<u8>>: Sync);

fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn notify_waiters_includes_unpolled_futures() {
    let notify = Arc::new(sync::Notify::new());
    let mut borrowed = pin!(notify.notified());
    let mut owned = pin!(notify.clone().notified_owned());
    notify.notify_waiters();
    assert!(poll(borrowed.as_mut()).is_ready());
    assert!(poll(owned.as_mut()).is_ready());
    assert!(poll(pin!(notify.notified())).is_pending());
}

#[test]
fn cancelled_notification_transfers_permit() {
    let notify = Arc::new(sync::Notify::new());
    let mut first = Box::pin(notify.notified());
    let mut second = pin!(notify.clone().notified_owned());
    assert!(poll(first.as_mut()).is_pending());
    assert!(poll(second.as_mut()).is_pending());
    notify.notify_one();
    drop(first);
    assert!(poll(second.as_mut()).is_ready());
    assert!(poll(pin!(notify.notified())).is_pending());

    let mut last = Box::pin(notify.clone().notified_owned());
    assert!(!last.as_mut().enable());
    notify.notify_last();
    drop(last);
    assert!(poll(pin!(notify.notified())).is_ready());
}

#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn notification_enable_keeps_registered_waker() {
    let notify = sync::Notify::new();
    let mut future = pin!(notify.notified());
    let counter = Arc::new(WakeCount::default());
    let waker = Waker::from(counter.clone());
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert!(!future.as_mut().enable());
    notify.notify_one();
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(poll(future.as_mut()).is_ready());
}

#[test]
fn closed_channels_finish_with_live_senders() {
    let (tx, mut rx) = mpsc::channel::<u8>(2);
    tx.try_send(1).unwrap();
    rx.close();
    assert!(matches!(poll(pin!(rx.recv())), Poll::Ready(Some(1))));
    assert!(matches!(poll(pin!(rx.recv())), Poll::Ready(None)));
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert!(tx.try_send(2).is_err());

    let (tx, mut rx) = mpsc::unbounded_channel::<u8>();
    tx.send(1).unwrap();
    rx.close();
    assert!(matches!(poll(pin!(rx.recv())), Poll::Ready(Some(1))));
    assert!(matches!(poll(pin!(rx.recv())), Poll::Ready(None)));
    assert!(tx.send(2).is_err());
}

#[test]
fn closed_channel_waits_for_reserved_permits_and_wakes_on_release() {
    let (tx, mut rx) = mpsc::channel::<u8>(1);
    let permit = tx.try_reserve().unwrap();
    rx.close();
    let counter = Arc::new(WakeCount::default());
    let waker = Waker::from(counter.clone());
    let mut recv = pin!(rx.recv());
    assert!(
        recv.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    drop(permit);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(matches!(poll(recv.as_mut()), Poll::Ready(None)));
}

#[test]
fn weak_senders_cannot_revive_a_channel() {
    let (tx, mut rx) = mpsc::channel::<u8>(1);
    let weak = tx.downgrade();
    assert_eq!(weak.strong_count(), 1);
    let second = weak.upgrade().unwrap();
    assert_eq!(weak.strong_count(), 2);
    drop((tx, second));
    assert_eq!(weak.strong_count(), 0);
    assert!(weak.upgrade().is_none());
    assert!(matches!(poll(pin!(rx.recv())), Poll::Ready(None)));

    let (tx, mut rx) = mpsc::unbounded_channel::<u8>();
    let weak = tx.downgrade();
    assert_eq!(weak.strong_count(), 1);
    drop(tx);
    assert_eq!(weak.strong_count(), 0);
    assert!(weak.upgrade().is_none());
    assert!(matches!(poll(pin!(rx.recv())), Poll::Ready(None)));
}

#[test]
fn cloned_watch_receivers_preserve_unseen_updates() {
    let (tx, mut rx) = watch::channel(0);
    tx.send(1).unwrap();
    let mut clone = rx.clone();
    assert!(clone.has_changed().unwrap());
    assert!(poll(pin!(clone.changed())).is_ready());
    assert!(!clone.has_changed().unwrap());
    assert!(rx.has_changed().unwrap());
    rx.borrow_and_update();
    assert!(!rx.clone().has_changed().unwrap());
}

#[test]
fn task_names_and_clones_have_independent_ownership() {
    let attr = ffrt::TaskAttr::new();
    let text = String::from("short trailing bytes that must not be read");
    attr.set_name(&text[..5]);
    let name = attr.get_name();
    let clone = attr.clone();
    attr.set_name(&"replacement".repeat(100));
    drop(attr);
    assert_eq!(name, "short");
    assert_eq!(clone.get_name(), "short");
    assert!(std::mem::needs_drop::<ffrt::TaskAttr>());
}

#[test]
fn timer_keeps_loop_and_queue_alive() {
    let queue = ffrt::queue::Queue::new(ffrt::queue::QueueType::Concurrent, "ownership-test", None);
    let looper = ffrt::looper::Looper::new(&queue);
    let captured = Arc::new(());
    let callback_value = captured.clone();
    let mut timer = looper.timer_start(60_000, false, move || {
        let _ = &callback_value;
    });
    drop(queue);
    drop(looper);
    timer.stop().unwrap();
    drop(timer);
    assert_eq!(Arc::strong_count(&captured), 1);
}

#[test]
#[allow(
    clippy::never_loop,
    clippy::needless_return,
    clippy::needless_question_mark
)]
fn select_handlers_keep_async_and_control_flow_scope() {
    ffrt::Runtime::new().unwrap().block_on(async {
        let value = ffrt::select! { value = ready(4) => ready(value + 1).await };
        assert_eq!(value, 5);
        let value = loop {
            ffrt::select! { _ = ready(()) => break 7 };
        };
        assert_eq!(value, 7);
        let value: Result<_, ()> = async {
            ffrt::select! { value = ready(Ok::<_, ()>(9)) => return Ok(value?) }
        }
        .await;
        assert_eq!(value, Ok(9));
        let mut count = 0;
        loop {
            count += 1;
            if count == 2 {
                break;
            }
            ffrt::select! { _ = ready(()) => continue };
        }
        assert_eq!(count, 2);
    });
}

#[test]
fn select_drops_losing_futures_before_handler_and_handles_patterns() {
    struct OnDrop(Arc<AtomicUsize>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    ffrt::Runtime::new().unwrap().block_on(async {
        let drops = Arc::new(AtomicUsize::new(0));
        let guard = OnDrop(drops.clone());
        let mut values = vec![1];
        ffrt::select! {
            biased;
            Some(ref mut value) = ready(Some(3)) => {
                *value += 1;
                assert_eq!(*value, 4);
                values.push(2);
                assert_eq!(drops.load(Ordering::Relaxed), 1);
            }
            _ = async { let _guard = guard; values.clear(); pending::<()>().await } => unreachable!(),
        }
        assert_eq!(values, [1, 2]);
        let result = ffrt::select! {
            Some(_) = ready(None::<u8>) => unreachable!(),
            _ = ready(()), if false => unreachable!(),
            else => 11,
        };
        assert_eq!(result, 11);
    });
}

#[test]
fn join_macros_preserve_borrowed_inputs() {
    ffrt::Runtime::new().unwrap().block_on(async {
        let text = String::from("retained");
        let (length, ()) = ffrt::join!(async { text.len() }, ready(()));
        assert_eq!(length, text.len());
        let result = ffrt::try_join!(async { Ok::<_, ()>(text.len()) }, ready(Ok::<_, ()>(())));
        assert_eq!(result.unwrap(), (text.len(), ()));
    });
}

#[cfg(feature = "macros")]
#[ffrt::main]
async fn attributed_function_with_args(value: usize) -> usize {
    value + 1
}

#[cfg(feature = "macros")]
#[test]
fn main_attribute_accepts_ordinary_function_arguments() {
    assert_eq!(attributed_function_with_args(4), 5);
}

#[test]
fn compatibility_type_paths_are_available() {
    let notify = sync::Notify::new();
    let notified: sync::futures::Notified<'_> = notify.notified();
    drop(notified);
    let _: Option<ffrt::time::error::Elapsed> = None;
}

#[test]
fn cached_writable_readiness_does_not_disable_reads() {
    use ffrt::io::{AsyncReadExt, AsyncWriteExt};
    ffrt::Runtime::new().unwrap().block_on(async {
        ffrt::time::timeout(Duration::from_secs(5), async {
            let listener = ffrt::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (client, accepted) =
                ffrt::join!(ffrt::net::TcpStream::connect(addr), listener.accept());
            let mut client = client.unwrap();
            let (mut server, _) = accepted.unwrap();
            client.writable().await.unwrap();
            let reader = async {
                let mut bytes = [0; 4];
                client.read_exact(&mut bytes).await.unwrap();
                assert_eq!(&bytes, b"ping");
            };
            let writer = async {
                ffrt::time::sleep(Duration::from_millis(30)).await;
                server.write_all(b"ping").await.unwrap();
            };
            ffrt::join!(reader, writer);
        })
        .await
        .expect("read readiness was lost after write readiness");
    });
}

#[test]
fn cancelled_child_wait_does_not_block_kill() {
    ffrt::Runtime::new().unwrap().block_on(async {
        let mut child = ffrt::process::Command::new("sleep")
            .arg("10")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        assert!(
            ffrt::time::timeout(Duration::from_millis(30), child.wait())
                .await
                .is_err()
        );
        let start = Instant::now();
        child.start_kill().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        ffrt::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn cancelled_child_wait_does_not_block_drop() {
    ffrt::Runtime::new().unwrap().block_on(async {
        let mut child = ffrt::process::Command::new("sleep")
            .arg("10")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        assert!(
            ffrt::time::timeout(Duration::from_millis(30), child.wait())
                .await
                .is_err()
        );
        let start = Instant::now();
        drop(child);
        assert!(start.elapsed() < Duration::from_secs(1));
        wait_until_reaped(pid).await;
    });
}

#[test]
fn mutex_mapping_panics_release_every_guard_kind() {
    use sync::{MappedMutexGuard, Mutex, MutexGuard, OwnedMappedMutexGuard, OwnedMutexGuard};
    let lock = Arc::new(Mutex::new(1u8));
    macro_rules! check {
        ($guard:expr, $ty:ident, $method:ident) => {{
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = $ty::$method::<u8, _>($guard, |_| panic!("mapping panic"));
                }))
                .is_err()
            );
            assert!(
                lock.try_lock().is_ok(),
                "{}::{} leaked its lock",
                stringify!($ty),
                stringify!($method)
            );
        }};
    }
    check!(lock.try_lock().unwrap(), MutexGuard, map);
    check!(lock.try_lock().unwrap(), MutexGuard, try_map);
    check!(lock.clone().try_lock_owned().unwrap(), OwnedMutexGuard, map);
    check!(
        lock.clone().try_lock_owned().unwrap(),
        OwnedMutexGuard,
        try_map
    );
    check!(
        MutexGuard::map(lock.try_lock().unwrap(), |x| x),
        MappedMutexGuard,
        map
    );
    check!(
        MutexGuard::map(lock.try_lock().unwrap(), |x| x),
        MappedMutexGuard,
        try_map
    );
    check!(
        OwnedMutexGuard::map(lock.clone().try_lock_owned().unwrap(), |x| x),
        OwnedMappedMutexGuard,
        map
    );
    check!(
        OwnedMutexGuard::map(lock.clone().try_lock_owned().unwrap(), |x| x),
        OwnedMappedMutexGuard,
        try_map
    );
}

#[test]
fn rwlock_mapping_panics_release_every_guard_kind() {
    use sync::{
        OwnedRwLockMappedWriteGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
        RwLockMappedWriteGuard, RwLockReadGuard, RwLockWriteGuard,
    };
    let lock = Arc::new(RwLock::new(1u8));
    macro_rules! check {
        ($guard:expr, $ty:ident, $method:ident) => {{
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = $ty::$method::<u8, _>($guard, |_| panic!("mapping panic"));
                }))
                .is_err()
            );
            assert!(
                lock.try_write().is_ok(),
                "{}::{} leaked its lock",
                stringify!($ty),
                stringify!($method)
            );
        }};
    }
    check!(lock.try_read().unwrap(), RwLockReadGuard, map);
    check!(lock.try_read().unwrap(), RwLockReadGuard, try_map);
    check!(lock.try_write().unwrap(), RwLockWriteGuard, map);
    check!(lock.try_write().unwrap(), RwLockWriteGuard, try_map);
    check!(lock.try_write().unwrap(), RwLockWriteGuard, downgrade_map);
    check!(
        lock.try_write().unwrap(),
        RwLockWriteGuard,
        try_downgrade_map
    );
    check!(
        lock.clone().try_read_owned().unwrap(),
        OwnedRwLockReadGuard,
        map
    );
    check!(
        lock.clone().try_read_owned().unwrap(),
        OwnedRwLockReadGuard,
        try_map
    );
    check!(
        lock.clone().try_write_owned().unwrap(),
        OwnedRwLockWriteGuard,
        map
    );
    check!(
        lock.clone().try_write_owned().unwrap(),
        OwnedRwLockWriteGuard,
        try_map
    );
    check!(
        lock.clone().try_write_owned().unwrap(),
        OwnedRwLockWriteGuard,
        downgrade_map
    );
    check!(
        RwLockWriteGuard::map(lock.try_write().unwrap(), |x| x),
        RwLockMappedWriteGuard,
        map
    );
    check!(
        RwLockWriteGuard::map(lock.try_write().unwrap(), |x| x),
        RwLockMappedWriteGuard,
        try_map
    );
    check!(
        OwnedRwLockWriteGuard::map(lock.clone().try_write_owned().unwrap(), |x| x),
        OwnedRwLockMappedWriteGuard,
        map
    );
    check!(
        OwnedRwLockWriteGuard::map(lock.clone().try_write_owned().unwrap(), |x| x),
        OwnedRwLockMappedWriteGuard,
        try_map
    );
}

#[test]
fn closed_oneshot_finishes_even_while_sender_is_alive() {
    let (tx, mut rx) = sync::oneshot::channel::<u8>();
    rx.close();
    assert!(rx.is_finished());
    assert!(matches!(
        rx.try_recv(),
        Err(sync::oneshot::error::TryRecvError::Closed)
    ));
    assert_eq!(tx.send(1), Err(1));
    let (_tx, mut rx) = sync::oneshot::channel::<u8>();
    rx.close();
    assert!(matches!(poll(pin!(rx)), Poll::Ready(Err(_))));
    let (tx, mut rx) = sync::oneshot::channel::<u8>();
    tx.send(3).unwrap();
    rx.close();
    assert_eq!(rx.try_recv().unwrap(), 3);
}

#[test]
fn select_internal_bindings_do_not_shadow_caller_values() {
    ffrt::Runtime::new().unwrap().block_on(async {
        let __ffrt_disabled = 7;
        let __ffrt_futures = 8;
        let __ffrt_output = 9;
        let result = ffrt::select! {
            value = ready(__ffrt_disabled + __ffrt_futures) => value + __ffrt_output,
        };
        assert_eq!(result, 24);
    });
}

#[test]
fn local_tasks_report_their_own_id_and_finish_when_set_is_dropped() {
    let runtime = ffrt::Runtime::new().unwrap();
    let local = ffrt::task::LocalSet::new();
    let task = local.spawn_local(async { ffrt::task::id() });
    let id = task.id();
    assert_eq!(runtime.block_on(local.run_until(task)).unwrap(), id);
    assert!(ffrt::task::try_id().is_none());

    let task = local.spawn_local(pending::<()>());
    let abort = task.abort_handle();
    drop(local);
    assert!(abort.is_finished());
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
}

#[test]
fn local_set_observes_wakes_from_other_threads() {
    ffrt::Runtime::new().unwrap().block_on(async {
        ffrt::time::timeout(Duration::from_secs(5), async {
            for _ in 0..64 {
                let local = ffrt::task::LocalSet::new();
                let (tx, rx) = sync::oneshot::channel();
                let task = local.spawn_local(async { rx.await.unwrap() });
                let worker = std::thread::spawn(move || {
                    tx.send(1).unwrap();
                });
                assert_eq!(local.run_until(task).await.unwrap(), 1);
                worker.join().unwrap();
            }
        })
        .await
        .unwrap();
    });
}

async fn wait_until_reaped(pid: u32) {
    ffrt::time::timeout(Duration::from_secs(2), async {
        loop {
            if unsafe { libc::kill(pid as libc::pid_t, 0) } < 0 {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
                break;
            }
            ffrt::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("dropped child was not reaped");
}

#[test]
fn detached_children_are_reaped_after_exiting() {
    ffrt::Runtime::new().unwrap().block_on(async {
        let child = ffrt::process::Command::new("sleep")
            .arg("0.05")
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        drop(child);
        wait_until_reaped(pid).await;
    });
}

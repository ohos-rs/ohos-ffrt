use std::error::Error;
#[cfg(target_env = "ohos")]
use std::os::raw::c_int;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use ffrt::io::{
    AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter, copy, join, split,
};
use ffrt::net::{TcpListener, TcpStream};
use ffrt::reactor::{AsyncFd, Interest};
#[cfg(target_env = "ohos")]
use ffrt::signal::unix::{SignalKind, signal};
use ffrt::sync::{
    Mutex, Notify, OnceCell, RwLock, Semaphore, SetOnce, broadcast, mpsc, oneshot, watch,
};

ffrt::task_local! {
    static REQUEST_ID: u32;
}

type SmokeResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[cfg(target_env = "ohos")]
unsafe extern "C" {
    fn raise(signal: c_int) -> c_int;
}

fn check(condition: bool, message: &str) -> SmokeResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

async fn check_io() -> SmokeResult {
    let mut source: &[u8] = b"one\ntwo";
    let mut exact = [0; 4];
    source.read_exact(&mut exact).await?;
    check(&exact == b"one\n", "read_exact mismatch")?;

    let mut tail = Vec::new();
    check(
        source.read_to_end(&mut tail).await? == 3 && tail == b"two",
        "read_to_end mismatch",
    )?;

    let mut reader = BufReader::with_capacity(2, &b"alpha\nbeta"[..]);
    let mut line = String::new();
    check(
        reader.read_line(&mut line).await? == 6 && line == "alpha\n",
        "BufReader/read_line mismatch",
    )?;

    let mut writer = BufWriter::with_capacity(3, Vec::new());
    writer.write_all(b"buffered").await?;
    writer.flush().await?;
    check(writer.get_ref() == b"buffered", "BufWriter mismatch")?;

    let mut copy_source: &[u8] = b"copied";
    let mut copy_target = Vec::new();
    check(
        copy(&mut copy_source, &mut copy_target).await? == 6 && copy_target == b"copied",
        "copy mismatch",
    )?;

    let mut cursor = std::io::Cursor::new(vec![0x12, 0x34]);
    check(
        cursor.read_u16().await? == 0x1234,
        "Cursor AsyncRead mismatch",
    )?;

    let mut joined = join(&b"joined"[..], Vec::new());
    let mut joined_bytes = Vec::new();
    joined.read_to_end(&mut joined_bytes).await?;
    joined.write_all(b"writer").await?;
    let (_, writer) = joined.into_inner();
    check(
        joined_bytes == b"joined" && writer == b"writer",
        "io::join mismatch",
    )?;

    #[cfg(target_env = "ohos")]
    {
        let (mut sender, mut receiver) = ffrt::net::unix::pipe::pipe()?;
        sender.write_all(b"pipe").await?;
        let mut bytes = [0; 4];
        receiver.read_exact(&mut bytes).await?;
        check(&bytes == b"pipe", "unix pipe mismatch")?;
    }

    println!("PASS io-combinators");
    Ok(())
}

async fn check_owned_guards() -> SmokeResult {
    let mutex = Arc::new(Mutex::new(1));
    let mut guard = mutex.clone().lock_owned().await;
    *guard += 1;
    check(*guard == 2, "Mutex::lock_owned mismatch")?;
    drop(guard);

    let rwlock = Arc::new(RwLock::new(3));
    check(
        *rwlock.clone().read_owned().await == 3,
        "RwLock::read_owned mismatch",
    )?;
    let mut write = rwlock.clone().write_owned().await;
    *write = 4;
    drop(write);
    check(
        *rwlock.read_owned().await == 4,
        "RwLock::write_owned mismatch",
    )?;

    let semaphore = Arc::new(Semaphore::new(1));
    let permit = semaphore.clone().acquire_owned().await?;
    check(
        semaphore.available_permits() == 0 && permit.num_permits() == 1,
        "Semaphore::acquire_owned mismatch",
    )?;
    drop(permit);
    check(
        semaphore.available_permits() == 1,
        "owned semaphore permit was not returned",
    )?;

    let mapped = Arc::new(Mutex::new((5, 6))).lock_owned().await;
    let mut mapped = ffrt::sync::OwnedMutexGuard::map(mapped, |value| &mut value.1);
    *mapped += 1;
    check(*mapped == 7, "OwnedMutexGuard::map mismatch")?;
    println!("PASS owned-guards");
    Ok(())
}

async fn check_channels_and_tasks() -> SmokeResult {
    let (tx, mut rx) = mpsc::channel(2);
    let permit = tx.reserve().await?;
    check(
        tx.capacity() == 1,
        "mpsc reservation did not consume capacity",
    )?;
    permit.send(7);
    let owned = tx.clone().reserve_owned().await?;
    let tx = owned.send(8);
    check(rx.recv().await == Some(7), "mpsc borrowed permit mismatch")?;
    check(rx.recv().await == Some(8), "mpsc owned permit mismatch")?;
    check(tx.capacity() == 2, "mpsc permits were not returned")?;

    let (many_tx, mut many_rx) = mpsc::unbounded_channel();
    many_tx.send(1)?;
    many_tx.send(2)?;
    let mut many = vec![0];
    check(
        many_rx.recv_many(&mut many, 2).await == 2 && many == [0, 1, 2],
        "mpsc recv_many counted pre-existing entries",
    )?;

    let (one_tx, mut one_rx) = oneshot::channel();
    one_tx.send(13).map_err(|_| "oneshot send failed")?;
    check(one_rx.try_recv()? == 13, "oneshot receive mismatch")?;
    check(
        matches!(one_rx.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
        "oneshot sender remained open after send",
    )?;

    let (close_tx, close_rx) = oneshot::channel::<()>();
    let close_waiter = ffrt::spawn(async move {
        let mut close_tx = close_tx;
        close_tx.closed().await;
    });
    ffrt::task::yield_now().await;
    drop(close_rx);
    ffrt::time::timeout(Duration::from_secs(3), close_waiter).await??;

    let (broadcast_tx, mut broadcast_a) = broadcast::channel(4);
    let mut broadcast_b = broadcast_tx.subscribe();
    broadcast_tx.send(11)?;
    check(
        broadcast_a.recv().await? == 11 && broadcast_b.recv().await? == 11,
        "broadcast did not deliver to every receiver",
    )?;

    let (watch_tx, mut watch_rx) = watch::channel(1);
    watch_tx.send(2)?;
    let watched = watch_rx.borrow_and_update();
    check(
        *watched == 2 && watched.has_changed(),
        "watch borrow_and_update state mismatch",
    )?;
    drop(watched);
    check(!watch_rx.has_changed()?, "watch value was not marked seen")?;

    let notify = Arc::new(Notify::new());
    let waiter = ffrt::spawn(notify.clone().notified_owned());
    ffrt::task::yield_now().await;
    notify.notify_one();
    ffrt::time::timeout(Duration::from_secs(3), waiter).await??;

    let once = Arc::new(SetOnce::new());
    let once_setter = once.clone();
    let once_task = ffrt::spawn(async move { once_setter.set(19) });
    check(*once.wait().await == 19, "SetOnce wait mismatch")?;
    once_task.await??;

    let once_cell = Arc::new(OnceCell::new());
    let initializing = once_cell.clone();
    let (started_tx, started_rx) = oneshot::channel();
    let initializer = ffrt::spawn(async move {
        initializing
            .get_or_init(|| async move {
                let _ = started_tx.send(());
                ffrt::time::sleep(Duration::from_secs(30)).await;
                1
            })
            .await;
    });
    started_rx.await?;
    initializer.abort();
    check(
        initializer.await.unwrap_err().is_cancelled(),
        "OnceCell initializer task was not cancelled",
    )?;
    check(
        *ffrt::time::timeout(
            Duration::from_secs(3),
            once_cell.get_or_init(|| async { 2 }),
        )
        .await?
            == 2,
        "OnceCell did not recover after initializer cancellation",
    )?;

    let task_id = ffrt::spawn(async { (ffrt::task::id(), ffrt::task::try_id()) }).await?;
    check(
        Some(task_id.0) == task_id.1,
        "task id was unavailable inside spawned task",
    )?;

    let cancelled = ffrt::spawn(async {
        ffrt::time::sleep(Duration::from_secs(30)).await;
    });
    let cancelled_id = cancelled.id();
    cancelled.abort();
    let cancelled = match ffrt::time::timeout(Duration::from_secs(3), cancelled).await? {
        Ok(()) => return Err("aborted task completed normally".into()),
        Err(error) => error,
    };
    check(cancelled.is_cancelled(), "aborted task was not cancelled")?;
    check(
        cancelled.id() == cancelled_id,
        "JoinError did not preserve the task id",
    )?;
    check(
        ffrt::runtime::Handle::current().metrics().num_workers() > 0,
        "runtime metrics reported no workers",
    )?;

    println!("PASS channels-notify-cancel");
    Ok(())
}

async fn check_task_local_and_select() -> SmokeResult {
    let local = REQUEST_ID
        .scope(41, async {
            ffrt::task::yield_now().await;
            REQUEST_ID.get() + 1
        })
        .await;
    check(local == 42, "task_local scope mismatch")?;

    let biased = ffrt::select! {
        biased;
        value = async { 1 } => value,
        value = async { 2 } => value,
        value = async { 3 } => value,
        value = async { 4 } => value,
        value = async { 5 } => value,
        value = async { 6 } => value,
    };
    check(biased == 1, "biased select order mismatch")?;

    let mut selected = [0usize; 6];
    for _ in 0..192 {
        let branch = ffrt::select! {
            _ = async {} => 0usize,
            _ = async {} => 1usize,
            _ = async {} => 2usize,
            _ = async {} => 3usize,
            _ = async {} => 4usize,
            _ = async {} => 5usize,
        };
        selected[branch] += 1;
    }
    check(
        selected.iter().filter(|count| **count != 0).count() == 6,
        "fair select did not reach every branch",
    )?;
    println!("PASS task-local-select counts={selected:?}");
    Ok(())
}

async fn check_async_fd() -> SmokeResult {
    let (reader, mut writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    let reader = AsyncFd::with_interest(reader, Interest::READABLE)?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        std::io::Write::write_all(&mut writer, b"ready").expect("UnixStream write failed");
        std::thread::sleep(Duration::from_millis(25));
        std::io::Write::write_all(&mut writer, b"again").expect("UnixStream rearm write failed");
        std::thread::sleep(Duration::from_millis(25));
        std::io::Write::write_all(&mut writer, b"third").expect("UnixStream async_io write failed");
    });

    for expected in [b"ready", b"again"] {
        let mut bytes = [0; 5];
        loop {
            let mut guard = reader.readable().await?;
            match guard.try_io(|stream| {
                let mut stream = stream;
                std::io::Read::read(&mut stream, &mut bytes)
            }) {
                Ok(result) => {
                    check(result? == 5 && &bytes == expected, "AsyncFd mismatch")?;
                    break;
                }
                Err(_) => continue,
            }
        }
    }

    let mut bytes = [0; 5];
    let amount = reader
        .async_io(Interest::READABLE, |stream| {
            let mut stream = stream;
            std::io::Read::read(&mut stream, &mut bytes)
        })
        .await?;
    check(
        amount == 5 && &bytes == b"third",
        "AsyncFd::async_io mismatch",
    )?;
    println!("PASS async-fd");
    Ok(())
}

async fn check_net() -> SmokeResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (accepted, connected) = ffrt::join!(listener.accept(), TcpStream::connect(address));
    let (server, peer) = accepted?;
    let mut client = connected?;
    check(peer.ip().is_loopback(), "accepted non-loopback peer")?;

    let (read_half, write_half) = split(server);
    let mut server = read_half.unsplit(write_half);

    client.write_all(b"reactor").await?;
    let mut payload = [0; 7];
    server.read_exact(&mut payload).await?;
    check(&payload == b"reactor", "reactor TCP payload mismatch")?;
    println!("PASS nonblocking-net");
    Ok(())
}

#[cfg(target_env = "ohos")]
async fn check_signal() -> SmokeResult {
    let kind = SignalKind::user_defined1();
    let mut stream = signal(kind)?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        for _ in 0..2 {
            let result = unsafe { raise(kind.as_raw_value()) };
            assert_eq!(result, 0, "raise failed");
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    for _ in 0..2 {
        let received = ffrt::time::timeout(Duration::from_secs(3), stream.recv()).await?;
        check(received.is_some(), "signal stream closed")?;
    }
    println!("PASS unix-signal");
    Ok(())
}

#[cfg(not(target_env = "ohos"))]
async fn check_signal() -> SmokeResult {
    println!("SKIP unix-signal (OpenHarmony only)");
    Ok(())
}

async fn smoke() -> SmokeResult {
    check_io().await?;
    check_owned_guards().await?;
    check_channels_and_tasks().await?;
    check_task_local_and_select().await?;
    check_async_fd().await?;
    check_net().await?;
    check_signal().await?;
    Ok(())
}

fn main() {
    let expected = std::env::args().nth(1).unwrap_or_default();
    let actual = std::env::consts::ARCH;
    if !expected.is_empty() && expected != actual {
        eprintln!("FAIL architecture expected={expected} actual={actual}");
        std::process::exit(2);
    }

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(45));
        eprintln!("FAIL watchdog timeout");
        std::process::exit(124);
    });

    println!("START ffrt-qemu-smoke arch={actual}");
    match ffrt::Runtime::new()
        .expect("create FFRT runtime")
        .block_on(smoke())
    {
        Ok(()) => println!("PASS ffrt-qemu-smoke arch={actual}"),
        Err(error) => {
            eprintln!("FAIL smoke: {error}");
            std::process::exit(1);
        }
    }
}

use std::error::Error;
#[cfg(target_env = "ohos")]
use std::os::raw::c_int;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use ffrt::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter, copy, split};
use ffrt::net::{TcpListener, TcpStream};
use ffrt::reactor::{AsyncFd, Interest};
#[cfg(target_env = "ohos")]
use ffrt::signal::unix::{SignalKind, signal};
use ffrt::sync::{Mutex, RwLock, Semaphore};

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
    println!("PASS owned-guards");
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
    }
    .await;
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
        }
        .await;
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
    });

    for expected in [b"ready", b"again"] {
        let mut bytes = [0; 5];
        loop {
            let guard = reader.readable().await?;
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
    println!("PASS async-fd");
    Ok(())
}

async fn check_net() -> SmokeResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (accepted, connected) = ffrt::join!(listener.accept(), TcpStream::connect(address)).await;
    let (server, peer) = accepted?;
    let client = connected?;
    check(peer.ip().is_loopback(), "accepted non-loopback peer")?;

    let (read_half, write_half) = split(server);
    let mut server = read_half.unsplit(write_half);

    client.write_all(b"reactor".to_vec()).await?;
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
    match ffrt::Runtime::new().block_on(smoke()) {
        Ok(Ok(())) => println!("PASS ffrt-qemu-smoke arch={actual}"),
        Ok(Err(error)) => {
            eprintln!("FAIL smoke: {error}");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("FAIL runtime: {error}");
            std::process::exit(1);
        }
    }
}

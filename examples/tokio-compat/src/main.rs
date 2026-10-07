use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex, Notify, OnceCell, RwLock, Semaphore, SetOnce};
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

static CONST_MUTEX: Mutex<u8> = Mutex::const_new(1);
static CONST_RWLOCK: RwLock<u8> = RwLock::const_with_max_readers(2, 8);
static CONST_SEMAPHORE: Semaphore = Semaphore::const_new(1);
static CONST_NOTIFY: Notify = Notify::const_new();
static CONST_ONCE_CELL: OnceCell<u8> = OnceCell::const_new_with(3);

tokio::task_local! {
    static REQUEST_ID: u64;
}

#[derive(Default)]
struct TraceCounts {
    task_spans: AtomicUsize,
    task_schema: AtomicUsize,
    task_polls: AtomicUsize,
    blocking_spans: AtomicUsize,
    blocking_schema: AtomicUsize,
    waker_events: AtomicUsize,
    waker_schema: AtomicUsize,
}

struct TraceCapture(Arc<TraceCounts>);

impl<S> Layer<S> for TraceCapture
where
    S: Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        let metadata = attrs.metadata();
        if metadata.name() == "runtime.spawn" && metadata.target() == "tokio::task" {
            self.0.task_spans.fetch_add(1, Ordering::Relaxed);
            if [
                "kind",
                "task.name",
                "task.id",
                "original_size.bytes",
                "size.bytes",
                "loc.file",
                "loc.line",
                "loc.col",
            ]
            .iter()
            .all(|name| metadata.fields().field(name).is_some())
            {
                self.0.task_schema.fetch_add(1, Ordering::Relaxed);
            }
        }
        if metadata.name() == "runtime.spawn" && metadata.target() == "tokio::task::blocking" {
            self.0.blocking_spans.fetch_add(1, Ordering::Relaxed);
            if [
                "kind",
                "task.name",
                "task.id",
                "fn",
                "original_size.bytes",
                "size.bytes",
                "loc.file",
                "loc.line",
                "loc.col",
            ]
            .iter()
            .all(|name| metadata.fields().field(name).is_some())
            {
                self.0.blocking_schema.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if ctx.span(id).is_some_and(|span| {
            span.metadata().name() == "runtime.spawn" && span.metadata().target() == "tokio::task"
        }) {
            self.0.task_polls.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "tokio::task::waker" {
            self.0.waker_events.fetch_add(1, Ordering::Relaxed);
            if ["op", "task.id"]
                .iter()
                .all(|name| event.metadata().fields().field(name).is_some())
            {
                self.0.waker_schema.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let trace_counts = Arc::new(TraceCounts::default());
    tracing::subscriber::set_global_default(
        Registry::default().with(TraceCapture(trace_counts.clone())),
    )?;

    tokio::task::Builder::new()
        .name("compat-task")
        .spawn(async {
            tokio::task::yield_now().await;
        })?
        .await?;
    tokio::task::Builder::new()
        .name("compat-blocking")
        .spawn_blocking(|| 7)?
        .await?;
    assert!(trace_counts.task_spans.load(Ordering::Relaxed) >= 1);
    assert!(trace_counts.task_schema.load(Ordering::Relaxed) >= 1);
    assert!(trace_counts.task_polls.load(Ordering::Relaxed) >= 1);
    assert!(trace_counts.blocking_spans.load(Ordering::Relaxed) >= 1);
    assert!(trace_counts.blocking_schema.load(Ordering::Relaxed) >= 1);
    assert!(trace_counts.waker_events.load(Ordering::Relaxed) >= 1);
    assert!(trace_counts.waker_schema.load(Ordering::Relaxed) >= 1);

    let _: tokio::io::Result<()> = Ok(());
    assert_eq!(*CONST_MUTEX.lock().await, 1);
    assert_eq!(*CONST_RWLOCK.read().await, 2);
    drop(CONST_SEMAPHORE.acquire().await?);
    CONST_NOTIFY.notify_one();
    CONST_NOTIFY.notified().await;
    assert_eq!(CONST_ONCE_CELL.get(), Some(&3));

    let (tx, mut rx) = mpsc::channel(1);
    tx.send(REQUEST_ID.scope(41, async { REQUEST_ID.get() + 1 }).await)
        .await?;
    assert_eq!(rx.recv().await, Some(42));

    let mutex = Arc::new(Mutex::new(1));
    *mutex.clone().lock_owned().await += 1;
    assert_eq!(*mutex.lock().await, 2);

    let notify = Arc::new(Notify::new());
    let waiter = tokio::spawn(notify.clone().notified_owned());
    tokio::task::yield_now().await;
    notify.notify_one();
    waiter.await?;

    let once = Arc::new(SetOnce::new());
    let setter = once.clone();
    tokio::spawn(async move { setter.set(9) }).await??;
    assert_eq!(*once.wait().await, 9);

    let once_cell = OnceCell::new();
    assert_eq!(*once_cell.get_or_init(|| async { 10 }).await, 10);

    let (mut client, mut server) = tokio::io::duplex(64);
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        client.write_all(b"tokio-compatible").await.unwrap();
        done_tx.send(()).unwrap();
    });
    let mut payload = vec![0; 16];
    server.read_exact(&mut payload).await?;
    done_rx.await?;
    assert_eq!(payload, b"tokio-compatible");

    let mut joined = tokio::io::join(&b"joined"[..], Vec::new());
    let mut joined_bytes = Vec::new();
    joined.read_to_end(&mut joined_bytes).await?;
    joined.write_all(b"writer").await?;
    assert_eq!(joined_bytes, b"joined");
    assert_eq!(joined.into_inner().1, b"writer");

    let mut empty = tokio::io::empty();
    assert_eq!(empty.write(b"discarded").await?, 9);
    assert_eq!(empty.seek(tokio::io::SeekFrom::End(10)).await?, 0);

    let local = tokio::task::LocalSet::new();
    let local_handle = {
        let _guard = local.enter();
        tokio::task::spawn_local(async { 11 })
    };
    assert_eq!(local.run_until(local_handle).await?, 11);

    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async { 1 });
    tasks.spawn(async { 2 });
    let mut joined_tasks = tasks.join_all().await;
    joined_tasks.sort();
    assert_eq!(joined_tasks, [1, 2]);

    let task_id = tokio::spawn(async { (tokio::task::id(), tokio::task::try_id()) }).await?;
    assert_eq!(Some(task_id.0), task_id.1);

    let selected = tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(1)) => 1,
        _ = tokio::time::sleep(Duration::from_secs(1)) => 2,
    };
    assert_eq!(selected, 1);

    #[cfg(target_env = "ohos")]
    {
        assert!(tokio::fs::try_exists("/").await?);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (accepted, connected) =
            tokio::join!(listener.accept(), tokio::net::TcpStream::connect(address));
        let (mut server, _) = accepted?;
        let mut client = connected?;
        client.write_all(b"reactor").await?;
        let mut bytes = [0; 7];
        server.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"reactor");

        let _unix_address: Option<tokio::net::unix::SocketAddr> = None;
        let (mut pipe_tx, mut pipe_rx) = tokio::net::unix::pipe::pipe()?;
        pipe_tx.write_all(b"pipe").await?;
        let mut pipe_bytes = [0; 4];
        pipe_rx.read_exact(&mut pipe_bytes).await?;
        assert_eq!(&pipe_bytes, b"pipe");
    }
    println!("PASS tokio-package-alias arch={}", std::env::consts::ARCH);
    Ok(())
}

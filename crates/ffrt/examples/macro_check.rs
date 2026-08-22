use ffrt::{join, pin, select, try_join};

pub async fn f() {
    let (a, b) = join!(async { 1 }, async { 2 }).await;
    let _ = (a, b);
    let (a, b, c) = join!(async { 1 }, async { 2 }, async { 3 }).await;
    let _ = (a, b, c);
    let x: Result<(i32, i32), &'static str> = try_join!(async { Ok(1) }, async { Ok(2) }).await;
    let _ = x;
    let y: Result<(i32,), &'static str> = try_join!(async { Ok(1) }).await;
    let _ = y;
}

pub async fn g() {
    let value = select! {
        v = async { 1 } => v,
        _ = async { 2 } => 0,
    }
    .await;
    let _ = value;

    let value3 = select! {
        v = async { 1 } => v,
        _ = async { 2 } => 0,
        _ = async { 3 } => -1,
    }
    .await;
    let _ = value3;
}

pub async fn local_demo() {
    use std::rc::Rc;
    let mut local = ffrt::task::LocalSet::new();
    let value = Rc::new(5);
    local.spawn_local(async move {
        let _ = value;
    });
    local.run_until(async {}).await;
}

pub async fn h() {
    let cell = ffrt::sync::OnceCell::new();
    let value = cell.get_or_init(async { 1 }).await;
    let _ = *value;
    let result = cell
        .get_or_try_init(async { Ok::<_, &'static str>(2) })
        .await;
    let _ = result;
}

#[allow(clippy::let_underscore_future)]
fn main() {
    let fut = async {};
    pin!(fut);
    let _ = fut;
}

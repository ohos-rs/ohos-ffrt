use ffrt::{join, pin, select, try_join};

pub async fn f() {
    let (a, b) = join!(async { 1 }, async { 2 });
    let _ = (a, b);
    let (a, b, c) = join!(async { 1 }, async { 2 }, async { 3 });
    let _ = (a, b, c);
    let x: Result<(i32, i32), &'static str> = try_join!(async { Ok(1) }, async { Ok(2) });
    let _ = x;
    let y: Result<(i32,), &'static str> = try_join!(async { Ok(1) });
    let _ = y;
}

pub async fn g() {
    let value = select! {
        v = async { 1 } => v,
        _ = async { 2 } => 0,
    };
    let _ = value;

    let value3 = select! {
        v = async { 1 } => v,
        _ = async { 2 } => 0,
        _ = async { 3 } => -1,
    };
    let _ = value3;

    let many = select! {
        biased;
        value = async { 1 } => value,
        value = async { 2 } => value,
        value = async { 3 } => value,
        value = async { 4 } => value,
        value = async { 5 } => value,
        value = async { 6 } => value,
    };
    assert_eq!(many, 1);

    let shared = String::from("shared");
    let shared_len = select! {
        biased;
        _ = async {} => shared.len(),
        _ = async {} => shared.len(),
    };
    assert_eq!(shared_len, 6);

    let enabled = false;
    let fallback = select! {
        _ = async {} , if enabled => 1,
        else => 2,
    };
    assert_eq!(fallback, 2);

    let ten = select! {
        biased;
        value = async { 1 } => value,
        value = async { 2 } => value,
        value = async { 3 } => value,
        value = async { 4 } => value,
        value = async { 5 } => value,
        value = async { 6 } => value,
        value = async { 7 } => value,
        value = async { 8 } => value,
        value = async { 9 } => value,
        value = async { 10 } => value,
    };
    assert_eq!(ten, 1);

    let seventeen = select! {
        biased;
        value = async { 1 } => value,
        value = async { 2 } => value,
        value = async { 3 } => value,
        value = async { 4 } => value,
        value = async { 5 } => value,
        value = async { 6 } => value,
        value = async { 7 } => value,
        value = async { 8 } => value,
        value = async { 9 } => value,
        value = async { 10 } => value,
        value = async { 11 } => value,
        value = async { 12 } => value,
        value = async { 13 } => value,
        value = async { 14 } => value,
        value = async { 15 } => value,
        value = async { 16 } => value,
        value = async { 17 } => value,
    };
    assert_eq!(seventeen, 1);
}

pub async fn local_demo() {
    use std::rc::Rc;
    let local = ffrt::task::LocalSet::new();
    let value = Rc::new(5);
    local.spawn_local(async move {
        let _ = value;
    });
    local.run_until(async {}).await;
}

pub async fn h() {
    let cell = ffrt::sync::OnceCell::new();
    let value = cell.get_or_init(|| async { 1 }).await;
    let _ = *value;
    let result = cell
        .get_or_try_init(|| async { Ok::<_, &'static str>(2) })
        .await;
    let _ = result;
}

#[allow(clippy::let_underscore_future)]
fn main() {
    let fut = async {};
    pin!(fut);
    let _ = fut;
}

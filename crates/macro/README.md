# napi-ffrt-ext-macro

This crate provide a simple macro that can help us use ffrt with simple code.

## Examples

```rs
use napi_ffrt_ext::*;

#[ffrt]
pub async fn example_e() -> napi_ohos::Result<String> {
    Ok("Hello, World!".to_string())
}
```

## LICENSE

[MIT](./LICENSE)

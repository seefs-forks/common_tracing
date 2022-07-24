# common_tracing
fork from https://github.com/datafuselabs/databend/tree/main/common/tracing

## 使用方式
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 注意要声明 变量
    let _guard = common_tracing::init_global_tracing("test", "/home/seefs/logs", "debug", None);

    info!("test");
    error!("test");
}
```
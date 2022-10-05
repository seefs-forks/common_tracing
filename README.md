# common_tracing
fork from https://github.com/datafuselabs/databend/tree/main/common/tracing

## 使用方式
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use common_tracing::init_logging;
    use common_tracing::Config as LogConfig;
    
    // 注意要声明 变量
    let _guards = init_logging("metactl", &LogConfig::default());

    info!("test");
    error!("test");
}
```
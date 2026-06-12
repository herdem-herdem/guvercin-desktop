#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--init-keyring") {
        rust_backend::init_keyring().await?;
        return Ok(());
    }
    if args.iter().any(|a| a == "--check-keyring") {
        rust_backend::check_keyring().await?;
        return Ok(());
    }

    rust_backend::run(None)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

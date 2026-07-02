#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // args no longer needed


    // Start backend and receive assigned ephemeral port
    let port = rust_backend::run(None)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // If GUVERCIN_KEEP_ALIVE is set, block until Ctrl+C so the server stays alive for manual testing
    if std::env::var("GUVERCIN_KEEP_ALIVE").is_ok() {
        tracing::info!(
            "Backend running on port {}. GUVERCIN_KEEP_ALIVE set; blocking until Ctrl+C",
            port
        );
        tokio::signal::ctrl_c().await?;
    }

    Ok(())
}

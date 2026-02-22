use anyhow::Result;
use tracing_subscriber::EnvFilter;

mod dbus_interface;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("visaged starting");

    // TODO: Initialize camera via visage-hw
    // TODO: Load ONNX models via visage-core
    // TODO: Register D-Bus interface
    // TODO: Enter main loop

    tracing::info!("visaged ready");

    // Keep running until signaled
    tokio::signal::ctrl_c().await?;
    tracing::info!("visaged shutting down");

    Ok(())
}

use std::sync::Arc;
use tokio::sync::Mutex;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

mod config;
mod dbus_interface;
mod engine;
mod store;

use config::Config;
use dbus_interface::{AppState, VisageService};
use engine::spawn_engine;
use store::FaceModelStore;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("visaged starting");

    // 1. Load configuration
    let config = Config::from_env();
    tracing::info!(
        camera = %config.camera_device,
        model_dir = %config.model_dir.display(),
        db_path = %config.db_path.display(),
        threshold = config.similarity_threshold,
        "configuration loaded"
    );

    // 2. Spawn engine (opens camera, loads models — fail-fast)
    let engine = spawn_engine(
        &config.camera_device,
        &config.scrfd_model_path(),
        &config.arcface_model_path(),
        config.warmup_frames,
        config.emitter_enabled,
    )?;
    tracing::info!("engine started");

    // 3. Open face model store (creates DB if needed)
    let store = FaceModelStore::open(&config.db_path).await?;
    let model_count = store.count_all().await.unwrap_or(0);
    tracing::info!(db = %config.db_path.display(), models = model_count, "store opened");

    // 4. Register D-Bus service on the system bus (required for PAM)
    //    Set VISAGE_SESSION_BUS=1 to use session bus for development without sudo.
    let state = Arc::new(Mutex::new(AppState {
        config,
        engine,
        store,
    }));

    let service = VisageService { state };

    let use_session = std::env::var("VISAGE_SESSION_BUS").is_ok();
    let _conn = if use_session {
        zbus::connection::Builder::session()?
    } else {
        zbus::connection::Builder::system()?
    }
    .name("org.freedesktop.Visage1")?
    .serve_at("/org/freedesktop/Visage1", service)?
    .build()
    .await?;

    let bus_name = if use_session { "session" } else { "system" };
    tracing::info!(bus = bus_name, "visaged ready — listening on org.freedesktop.Visage1");

    // 5. Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    tracing::info!("visaged shutting down");

    Ok(())
}

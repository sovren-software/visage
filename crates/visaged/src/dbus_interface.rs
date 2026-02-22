use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::interface;

use crate::config::Config;
use crate::engine::EngineHandle;
use crate::store::FaceModelStore;

/// Shared state accessible by D-Bus method handlers.
pub struct AppState {
    pub config: Config,
    pub engine: EngineHandle,
    pub store: FaceModelStore,
}

/// D-Bus interface for the Visage biometric daemon.
///
/// Bus name: org.freedesktop.Visage1
/// Object path: /org/freedesktop/Visage1
pub struct VisageService {
    pub state: Arc<Mutex<AppState>>,
}

#[interface(name = "org.freedesktop.Visage1")]
impl VisageService {
    /// Enroll a new face model for the given user.
    ///
    /// Returns the UUID of the newly created model.
    async fn enroll(&self, user: &str, label: &str) -> zbus::fdo::Result<String> {
        tracing::info!(user, label, "enroll requested");

        // Copy values while holding lock, then release
        let (engine, frames_count) = {
            let state = self.state.lock().await;
            (state.engine.clone(), state.config.frames_per_enroll)
        };

        // Run engine (no lock held)
        let result = engine.enroll(frames_count).await.map_err(|e| {
            tracing::error!(error = %e, "enroll failed");
            zbus::fdo::Error::Failed(e.to_string())
        })?;

        tracing::info!(quality = result.quality_score, "enroll: embedding extracted");

        // Store result (re-acquire lock)
        let state = self.state.lock().await;
        let model_id = state
            .store
            .insert(user, label, &result.embedding, result.quality_score)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "enroll: store insert failed");
                zbus::fdo::Error::Failed(e.to_string())
            })?;

        tracing::info!(model_id = %model_id, user, label, "enrolled successfully");
        Ok(model_id)
    }

    /// Verify the current face against enrolled models for the given user.
    ///
    /// Returns true if the face matches any enrolled model above the threshold.
    async fn verify(&self, user: &str) -> zbus::fdo::Result<bool> {
        tracing::info!(user, "verify requested");

        // Fetch gallery and config while holding lock
        let (engine, gallery, threshold, frames_count, timeout_secs) = {
            let state = self.state.lock().await;
            let gallery = state.store.get_gallery_for_user(user).await.map_err(|e| {
                tracing::error!(error = %e, "verify: gallery fetch failed");
                zbus::fdo::Error::Failed(e.to_string())
            })?;
            (
                state.engine.clone(),
                gallery,
                state.config.similarity_threshold,
                state.config.frames_per_verify,
                state.config.verify_timeout_secs,
            )
        };

        if gallery.is_empty() {
            tracing::warn!(user, "verify: no enrolled models");
            return Err(zbus::fdo::Error::Failed(format!(
                "no enrolled models for user '{user}'"
            )));
        }

        // Run engine with timeout (no lock held)
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let result = engine
            .verify(gallery, threshold, frames_count, timeout)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "verify failed");
                zbus::fdo::Error::Failed(e.to_string())
            })?;

        tracing::info!(
            user,
            matched = result.result.matched,
            similarity = result.result.similarity,
            model_id = ?result.result.model_id,
            "verify complete"
        );

        Ok(result.result.matched)
    }

    /// Return daemon status information as JSON.
    async fn status(&self) -> zbus::fdo::Result<String> {
        let state = self.state.lock().await;
        let model_count = state.store.count_all().await.unwrap_or(0);

        Ok(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "camera": state.config.camera_device,
            "models_enrolled": model_count,
            "similarity_threshold": state.config.similarity_threshold,
        })
        .to_string())
    }

    /// List enrolled face models for the given user as JSON.
    async fn list_models(&self, user: &str) -> zbus::fdo::Result<String> {
        tracing::info!(user, "list_models requested");
        let state = self.state.lock().await;
        let models = state.store.list_by_user(user).await.map_err(|e| {
            zbus::fdo::Error::Failed(e.to_string())
        })?;
        serde_json::to_string(&models).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    /// Remove an enrolled face model by ID (scoped to user).
    async fn remove_model(&self, user: &str, model_id: &str) -> zbus::fdo::Result<bool> {
        tracing::info!(user, model_id, "remove_model requested");
        let state = self.state.lock().await;
        let removed = state.store.remove(user, model_id).await.map_err(|e| {
            zbus::fdo::Error::Failed(e.to_string())
        })?;
        if removed {
            tracing::info!(model_id, "model removed");
        } else {
            tracing::warn!(model_id, user, "model not found or not owned by user");
        }
        Ok(removed)
    }
}

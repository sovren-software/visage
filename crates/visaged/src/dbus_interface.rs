use zbus::interface;

/// D-Bus interface for the Visage biometric daemon.
///
/// Bus name: org.freedesktop.Visage1
/// Object path: /org/freedesktop/Visage1
pub struct VisageService;

#[interface(name = "org.freedesktop.Visage1")]
impl VisageService {
    /// Enroll a new face model for the given user.
    async fn enroll(&self, user: &str, label: &str) -> zbus::fdo::Result<String> {
        tracing::info!(user, label, "enroll requested");
        // TODO: Capture frames, extract embeddings, store model
        Err(zbus::fdo::Error::NotSupported(
            "enrollment not yet implemented".into(),
        ))
    }

    /// Verify the current face against enrolled models for the given user.
    async fn verify(&self, user: &str) -> zbus::fdo::Result<bool> {
        tracing::info!(user, "verify requested");
        // TODO: Capture frame, run detection + recognition, compare
        Err(zbus::fdo::Error::NotSupported(
            "verification not yet implemented".into(),
        ))
    }

    /// Return daemon status information.
    async fn status(&self) -> zbus::fdo::Result<String> {
        Ok(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "camera": "not initialized",
            "models_loaded": false,
            "ir_emitter": "not initialized",
        })
        .to_string())
    }

    /// List enrolled face models for the given user.
    async fn list_models(&self, user: &str) -> zbus::fdo::Result<String> {
        tracing::info!(user, "list_models requested");
        // TODO: Query model store
        Ok("[]".into())
    }

    /// Remove an enrolled face model by ID.
    async fn remove_model(&self, user: &str, model_id: &str) -> zbus::fdo::Result<bool> {
        tracing::info!(user, model_id, "remove_model requested");
        // TODO: Remove from model store
        Err(zbus::fdo::Error::NotSupported(
            "model removal not yet implemented".into(),
        ))
    }
}

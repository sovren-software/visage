use nix::unistd::User;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use zbus::interface;
use zbus::object_server::SignalEmitter;

use crate::config::Config;
use crate::engine::{EngineError, EngineHandle, PreviewFrame};
use crate::rate_limiter::RateLimiter;
use crate::store::{AuthAttempt, FaceModelStore};

/// Shared state accessible by D-Bus method handlers.
pub struct AppState {
    pub config: Config,
    pub engine: EngineHandle,
    pub store: FaceModelStore,
    pub rate_limiter: RateLimiter,
}

/// D-Bus interface for the Visage biometric daemon.
///
/// Bus name: org.freedesktop.Visage1
/// Object path: /org/freedesktop/Visage1
pub struct VisageService {
    pub state: Arc<Mutex<AppState>>,
}

/// Retrieve the UID of the D-Bus peer identified by `sender_str` (a unique bus name).
async fn get_caller_uid(sender_str: &str, conn: &zbus::Connection) -> zbus::fdo::Result<u32> {
    let dbus_proxy = zbus::fdo::DBusProxy::new(conn)
        .await
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
    let bus_name = zbus::names::BusName::try_from(sender_str)
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
    dbus_proxy
        .get_connection_unix_user(bus_name)
        .await
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
}

/// How long the framing phase should run for this enrollment.
///
/// Framing exists so a human can see themselves and get centred. With no
/// preview channel there is no human watching, and the phase is pure added
/// latency on the authentication-adjacent path — so it collapses to zero.
///
/// Keeping this as its own function rather than an inline `if` is deliberate:
/// it is the rule "a client that does not want previews pays none of their
/// cost", and a rule worth stating is worth being able to assert.
fn framing_for(has_preview: bool, configured: std::time::Duration) -> std::time::Duration {
    if has_preview {
        configured
    } else {
        std::time::Duration::ZERO
    }
}

/// Look up the numeric UID for a username via NSS.
fn uid_for_name(name: &str) -> Option<u32> {
    match User::from_name(name) {
        Ok(Some(user)) => Some(user.uid.as_raw()),
        Ok(None) => None,
        Err(_) => None,
    }
}

/// Defense-in-depth: require the D-Bus caller to be root (UID 0) for a
/// privileged method (`Enroll`, `RemoveModel`, `ListModels`).
///
/// The system-bus policy (`org.freedesktop.Visage1.conf`) already restricts
/// these methods to root by omission from the `default` context. This re-checks
/// the caller's UID in-process so a missing, mis-scoped, or overly-permissive
/// policy file cannot silently widen access to enrollment mutation or the
/// enrollment listing. On the session bus (development mode) the check is
/// skipped — mirroring `verify`'s UID validation, since all session-bus callers
/// share one user and no system policy applies.
async fn require_root_caller(
    method: &str,
    session_bus: bool,
    header: &zbus::message::Header<'_>,
    conn: &zbus::Connection,
) -> zbus::fdo::Result<()> {
    if session_bus {
        return Ok(());
    }
    let sender = header
        .sender()
        .ok_or_else(|| zbus::fdo::Error::Failed("no sender in message".to_string()))?;
    let caller_uid = get_caller_uid(sender.as_str(), conn).await?;
    if caller_uid != 0 {
        tracing::warn!(
            method,
            caller_uid,
            "privileged method denied: caller is not root"
        );
        return Err(zbus::fdo::Error::AccessDenied(format!(
            "method '{method}' requires root"
        )));
    }
    Ok(())
}

/// Best-effort history write: a full disk or locked DB must never fail a login.
async fn record_history(state: &Arc<Mutex<AppState>>, attempt: AuthAttempt) {
    let store = state.lock().await.store.clone();
    if let Err(e) = store.record_attempt(&attempt).await {
        tracing::warn!(error = %e, "auth history write failed (auth result unaffected)");
    }
}

#[interface(name = "org.freedesktop.Visage1")]
impl VisageService {
    /// Enroll a new face model for the given user.
    ///
    /// Returns the UUID of the newly created model.
    async fn enroll(
        &self,
        user: &str,
        label: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<String> {
        tracing::info!(user, label, "enroll requested");

        // Copy values while holding lock, then release
        let (engine, frames_count, session_bus, framing) = {
            let state = self.state.lock().await;
            (
                state.engine.clone(),
                state.config.frames_per_enroll,
                state.config.session_bus,
                state.config.framing_duration,
            )
        };

        // Defense-in-depth (enrollment is a privileged mutation).
        require_root_caller("Enroll", session_bus, &header, conn).await?;

        // --- Preview: unicast PreviewFrame signals, for this enrollment only ---
        //
        // There is deliberately no method to ask for a camera frame. The only
        // producer is an Enroll the caller started, and the frames go only to
        // that caller's own bus name — so the preview cannot be used as a
        // general camera tap, and it inherits Enroll's root-only policy without
        // adding a second privilege boundary. The channel lives for exactly the
        // duration of this call.
        //
        // Frames are already downscaled by the engine before they reach here;
        // full-resolution captures never cross a channel.
        let preview_tx = match header.sender() {
            Some(sender) => {
                let dest = sender.to_string();
                // Small buffer: a preview is only useful live, so a stale frame
                // is worth less than the memory to hold it.
                let (tx, mut rx) = mpsc::channel::<PreviewFrame>(8);
                let conn = conn.clone();
                tokio::spawn(async move {
                    while let Some(f) = rx.recv().await {
                        // A failed emit is not an enrollment failure. The client
                        // may have gone away, and enrollment must not depend on
                        // anything about the preview succeeding.
                        if let Err(e) = conn
                            .emit_signal(
                                Some(dest.as_str()),
                                "/org/freedesktop/Visage1",
                                "org.freedesktop.Visage1",
                                "PreviewFrame",
                                &(f.width, f.height, f.is_dark, f.data),
                            )
                            .await
                        {
                            tracing::debug!(error = %e, "preview emit failed; stopping preview");
                            break;
                        }
                    }
                });
                Some(tx)
            }
            // No sender means no unicast destination — emit nothing rather than
            // broadcasting camera frames to the whole bus.
            None => None,
        };

        // Run engine (no lock held). The sender is moved in and dropped when the
        // engine finishes with it, which closes the channel and ends the task.
        let framing = framing_for(preview_tx.is_some(), framing);

        let result = engine
            .enroll(frames_count, preview_tx, framing)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "enroll failed");
                zbus::fdo::Error::Failed(e.to_string())
            })?;

        tracing::info!(
            quality = result.quality_score,
            "enroll: embedding extracted"
        );

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

    /// A downscaled camera frame captured during an in-flight `Enroll`.
    ///
    /// Sent only to the caller that started that enrollment, only while it is
    /// running. `is_dark` marks a frame the capture loop rejected as too dark —
    /// those are emitted precisely because "too dark" is the most useful thing
    /// a user can be told while enrolling.
    ///
    /// `data` is 8-bit grayscale, row-major, `width * height` bytes, downscaled
    /// to at most 160px on the longest edge: enough to check framing, and
    /// deliberately not enough to be a useful biometric capture.
    #[zbus(signal)]
    async fn preview_frame(
        signal_emitter: &SignalEmitter<'_>,
        width: u32,
        height: u32,
        is_dark: bool,
        data: &[u8],
    ) -> zbus::Result<()>;

    /// Verify the current face against enrolled models for the given user.
    ///
    /// Returns true if the face matches any enrolled model above the threshold.
    ///
    /// Security: on the system bus the caller UID is validated against the target
    /// username before any camera access or rate-limit check.  Root (UID 0) is always
    /// permitted.  On the session bus (development mode) UID validation is skipped.
    async fn verify(
        &self,
        user: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<bool> {
        tracing::info!(user, "verify requested");

        // Read session_bus flag without holding lock across the async UID lookup
        let session_bus = self.state.lock().await.config.session_bus;

        // --- UID validation (system bus only) ---
        // The validated caller UID, for the history row. Root (0) or session-bus
        // mode records None — there is no per-user caller to distinguish.
        let mut history_uid: Option<i64> = None;
        if !session_bus {
            let sender = header
                .sender()
                .ok_or_else(|| zbus::fdo::Error::Failed("no sender in message".to_string()))?;
            let caller_uid = get_caller_uid(sender.as_str(), conn).await?;
            if caller_uid != 0 {
                history_uid = Some(caller_uid as i64);
                match uid_for_name(user) {
                    Some(expected_uid) if caller_uid == expected_uid => {}
                    Some(_) => {
                        tracing::warn!(
                            user,
                            caller_uid,
                            "verify: caller UID does not match target user UID"
                        );
                        record_history(
                            &self.state,
                            AuthAttempt::now(user, false, 0.0, None, None, "denied", history_uid),
                        )
                        .await;
                        return Err(zbus::fdo::Error::AccessDenied(format!(
                            "caller is not permitted to verify user '{user}'"
                        )));
                    }
                    None => {
                        tracing::warn!(user, "verify: unknown user");
                        record_history(
                            &self.state,
                            AuthAttempt::now(user, false, 0.0, None, None, "denied", history_uid),
                        )
                        .await;
                        return Err(zbus::fdo::Error::Failed(format!("unknown user '{user}'")));
                    }
                }
            }
        }

        // --- Rate limit check ---
        {
            let mut state = self.state.lock().await;
            if let Err(msg) = state.rate_limiter.check(user) {
                tracing::warn!(user, "verify: rate limited");
                drop(state);
                record_history(
                    &self.state,
                    AuthAttempt::now(user, false, 0.0, None, None, "rate-limited", history_uid),
                )
                .await;
                return Err(zbus::fdo::Error::Failed(msg));
            }
        }

        // --- Fetch gallery and config (release lock before engine call) ---
        let (
            engine,
            gallery,
            threshold,
            frames_count,
            timeout_secs,
            liveness_enabled,
            liveness_min_displacement,
        ) = {
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
                state.config.liveness_enabled,
                state.config.liveness_min_displacement,
            )
        };

        if gallery.is_empty() {
            tracing::warn!(user, "verify: no enrolled models");
            return Err(zbus::fdo::Error::Failed(format!(
                "no enrolled models for user '{user}'"
            )));
        }

        // --- Run engine with timeout (no lock held) ---
        // Runtime errors (camera failure, timeout) are returned as Err and do NOT count
        // as rate-limit failures. Liveness failures are treated as deliberate auth failures
        // and converted to non-match so they are rate-limited like other failed attempts.
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let result = match engine
            .verify(
                gallery,
                threshold,
                frames_count,
                timeout,
                liveness_enabled,
                liveness_min_displacement,
            )
            .await
        {
            Ok(result) => result,
            Err(EngineError::LivenessCheckFailed {
                displacement,
                threshold,
            }) => {
                tracing::warn!(
                    user,
                    displacement,
                    threshold,
                    "verify: liveness check failed — treating as non-match"
                );
                crate::engine::VerifyResult {
                    result: visage_core::MatchResult {
                        matched: false,
                        similarity: 0.0,
                        model_id: None,
                        model_label: None,
                    },
                    best_quality: 0.0,
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "verify failed");
                // Runtime failures (camera dark, timeout) are auth failures too —
                // the user saw a reject. Record best-effort, then report the error.
                let reason = match &e {
                    EngineError::NoFaceDetected | EngineError::NoUsableFrames => {
                        "no-face-or-liveness"
                    }
                    _ => "error",
                };
                record_history(
                    &self.state,
                    AuthAttempt::now(user, false, 0.0, None, None, reason, history_uid),
                )
                .await;
                return Err(zbus::fdo::Error::Failed(e.to_string()));
            }
        };

        // --- Record rate-limit outcome + history (history never fails auth) ---
        {
            let mut state = self.state.lock().await;
            if result.result.matched {
                state.rate_limiter.record_success(user);
            } else {
                state.rate_limiter.record_failure(user);
            }
        }
        // Deliberately NOT distinguishing wrong-face from liveness-reject here:
        // telling a caller (or a history reader) that the face *matched* but
        // liveness vetoed it would leak recognition info. The daemon log keeps
        // the real reason; history records a plain non-match either way.
        // ("no-face" is safe to name: it says nobody was in frame, not that a
        // face was recognised.)
        let reason = if result.result.matched {
            "verify"
        } else if result.result.similarity == 0.0
            && result.result.model_id.is_none()
            && result.best_quality == 0.0
        {
            "no-face-or-liveness"
        } else {
            "verify"
        };
        record_history(
            &self.state,
            AuthAttempt::now(
                user,
                result.result.matched,
                result.result.similarity,
                result.result.model_id.clone(),
                result.result.model_label.clone(),
                reason,
                history_uid,
            ),
        )
        .await;

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
            "model_dir": state.config.model_dir.display().to_string(),
            "db_path": state.config.db_path.display().to_string(),
            "models_enrolled": model_count,
            "similarity_threshold": state.config.similarity_threshold,
            "verify_timeout_secs": state.config.verify_timeout_secs,
            "warmup_frames": state.config.warmup_frames,
            "frames_per_verify": state.config.frames_per_verify,
            "frames_per_enroll": state.config.frames_per_enroll,
            "emitter_enabled": state.config.emitter_enabled,
            "liveness_enabled": state.config.liveness_enabled,
            "liveness_min_displacement": state.config.liveness_min_displacement,
            "session_bus": state.config.session_bus,
        })
        .to_string())
    }

    /// List enrolled face models for the given user as JSON.
    async fn list_models(
        &self,
        user: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<String> {
        tracing::info!(user, "list_models requested");
        // Defense-in-depth: enrollment listing is a root-only operation.
        let session_bus = self.state.lock().await.config.session_bus;
        require_root_caller("ListModels", session_bus, &header, conn).await?;
        let state = self.state.lock().await;
        let models = state
            .store
            .list_by_user(user)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        serde_json::to_string(&models).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    /// Remove an enrolled face model by ID (scoped to user).
    async fn remove_model(
        &self,
        user: &str,
        model_id: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<bool> {
        tracing::info!(user, model_id, "remove_model requested");
        // Defense-in-depth (removal is a privileged mutation).
        let session_bus = self.state.lock().await.config.session_bus;
        require_root_caller("RemoveModel", session_bus, &header, conn).await?;
        let state = self.state.lock().await;
        let removed = state
            .store
            .remove(user, model_id)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        if removed {
            tracing::info!(model_id, "model removed");
        } else {
            tracing::warn!(model_id, user, "model not found or not owned by user");
        }
        Ok(removed)
    }

    /// Recent authentication attempts (newest first) as JSON. Root-only:
    /// history names who logged in when, so it stays behind the same
    /// privilege boundary as the enrollment listing.
    async fn history(
        &self,
        user: &str,
        limit: u32,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<String> {
        tracing::info!(user, limit, "history requested");
        let session_bus = self.state.lock().await.config.session_bus;
        require_root_caller("History", session_bus, &header, conn).await?;
        let state = self.state.lock().await;
        let filter = if user.is_empty() { None } else { Some(user) };
        let attempts = state
            .store
            .recent_attempts(filter, limit.max(1) as usize)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        serde_json::to_string(&attempts).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

#[cfg(test)]
mod framing_tests {
    use super::framing_for;
    use std::time::Duration;

    const CONFIGURED: Duration = Duration::from_millis(1500);

    #[test]
    fn a_client_that_wants_previews_gets_the_configured_window() {
        assert_eq!(framing_for(true, CONFIGURED), CONFIGURED);
    }

    /// The rule that matters: framing is latency on an enrollment path, and a
    /// caller with nowhere to send frames must not pay it. Without this, every
    /// scripted or headless enrollment would silently get slower.
    #[test]
    fn a_client_without_a_preview_channel_pays_nothing() {
        assert_eq!(framing_for(false, CONFIGURED), Duration::ZERO);
    }

    /// Zero is the documented disable switch, so it must survive the path that
    /// would otherwise be the "yes, framing" branch.
    #[test]
    fn zero_stays_zero_even_with_a_watcher() {
        assert_eq!(framing_for(true, Duration::ZERO), Duration::ZERO);
    }
}

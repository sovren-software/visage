//! The client side of `org.freedesktop.Visage1`, defined once.
//!
//! Every process that talks to `visaged` — the CLI, and any enrollment
//! front-end — uses the proxy in this crate rather than declaring its own.
//!
//! That is not tidiness. This repository has shipped duplicate-definition drift
//! twice: the hardware compatibility table drifted from the quirks actually
//! embedded in the binary, and the README's copy of that same table then fell
//! four entries further behind. A second hand-written copy of a wire contract
//! is the same shape of defect, and it fails in a worse place — at runtime,
//! against a daemon that is behaving correctly.
//!
//! `crates/visaged/tests/dbus_contract.rs` checks this file against the
//! daemon's interface. One definition, one place the contract test looks.

/// The Visage daemon's D-Bus interface, from a client's point of view.
#[zbus::proxy(
    interface = "org.freedesktop.Visage1",
    default_service = "org.freedesktop.Visage1",
    default_path = "/org/freedesktop/Visage1"
)]
pub trait Visage {
    /// Enroll a face for `user` under `label`. Root-only.
    async fn enroll(&self, user: &str, label: &str) -> zbus::fdo::Result<String>;

    /// Verify the face in front of the camera against `user`'s models.
    async fn verify(&self, user: &str) -> zbus::fdo::Result<bool>;

    /// Daemon status as a human-readable string.
    async fn status(&self) -> zbus::fdo::Result<String>;

    /// List `user`'s enrolled models. Root-only.
    async fn list_models(&self, user: &str) -> zbus::fdo::Result<String>;

    /// Remove one of `user`'s models by id. Root-only.
    async fn remove_model(&self, user: &str, model_id: &str) -> zbus::fdo::Result<bool>;

    /// Recent authentication attempts (newest first) as JSON. Root-only.
    ///
    /// `limit` caps rows (1-500, default 50 server-side). Empty `user` means
    /// all users. Text-only: no images, no embeddings.
    async fn history(&self, user: &str, limit: u32) -> zbus::fdo::Result<String>;

    /// A downscaled camera frame, sent only to the caller whose `enroll` is
    /// currently running.
    ///
    /// 8-bit grayscale, row-major, `width * height` bytes, at most 160px on the
    /// longest edge. `is_dark` marks a frame the capture loop rejected as too
    /// dark — those are sent deliberately, because "too dark" is the most
    /// actionable thing a user can be told.
    ///
    /// ⚠️ The daemon flags dark frames only. A *saturated* frame — the white-out
    /// that a mis-warmed IR emitter produces — arrives with `is_dark` false and
    /// looks unremarkable. A client holding the pixels can compute mean
    /// brightness and say "too bright" itself; the daemon cannot do it for you.
    #[zbus(signal)]
    async fn preview_frame(
        &self,
        width: u32,
        height: u32,
        is_dark: bool,
        data: Vec<u8>,
    ) -> zbus::Result<()>;
}

/// Whether `VISAGE_SESSION_BUS` asks for the session bus.
///
/// ⚠️ Absent, empty and `"0"` all mean **no**. The obvious implementation —
/// `env::var(..).is_ok()` — treats `VISAGE_SESSION_BUS=0` as a *yes*, because
/// the variable is set. The daemon fixed that fail-open trap; the CLI did not,
/// and the two then disagreed: `VISAGE_SESSION_BUS=0` put the daemon on the
/// system bus and the client on the session bus, and the user was told "is
/// visaged running?" about a daemon that was running perfectly.
///
/// Both sides read this function now, so they cannot disagree again.
pub fn wants_session_bus(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.is_empty() && v != "0")
}

/// `wants_session_bus`, reading the process environment.
pub fn session_bus_from_env() -> bool {
    wants_session_bus(std::env::var("VISAGE_SESSION_BUS").ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::wants_session_bus;

    #[test]
    fn absent_means_system_bus() {
        assert!(!wants_session_bus(None));
    }

    /// The case the CLI got wrong. Setting a variable to "0" is how a person
    /// turns something off, and reading it as "on" is a fail-open.
    #[test]
    fn zero_means_system_bus() {
        assert!(!wants_session_bus(Some("0")));
    }

    #[test]
    fn empty_means_system_bus() {
        assert!(!wants_session_bus(Some("")));
    }

    #[test]
    fn anything_else_means_session_bus() {
        for v in ["1", "true", "yes", "on"] {
            assert!(
                wants_session_bus(Some(v)),
                "{v:?} should select the session bus"
            );
        }
    }
}

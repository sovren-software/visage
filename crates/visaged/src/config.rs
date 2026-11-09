use std::path::PathBuf;

/// Daemon configuration, loaded from environment variables.
pub struct Config {
    /// V4L2 device path (default: /dev/video2).
    pub camera_device: String,
    /// Directory containing ONNX model files.
    pub model_dir: PathBuf,
    /// Path to the SQLite database file.
    pub db_path: PathBuf,
    /// Cosine similarity threshold for a positive match.
    pub similarity_threshold: f32,
    /// Timeout in seconds for a verify operation.
    pub verify_timeout_secs: u64,
    /// Number of warmup frames to discard at startup (camera AGC/AE stabilization).
    pub warmup_frames: usize,
    /// Number of frames to capture per verify attempt.
    pub frames_per_verify: usize,
    /// Number of frames to capture per enroll attempt.
    pub frames_per_enroll: usize,
    /// How long to stream framing frames to the enrolling client before each
    /// capture. These are shown to the user so they can get centred, and are
    /// then discarded — they never reach the model.
    ///
    /// Zero disables the framing phase, restoring pre-framing behaviour.
    pub framing_duration: std::time::Duration,
    /// Whether to activate the IR emitter around each capture sequence.
    pub emitter_enabled: bool,
    /// Whether passive liveness detection (landmark stability) is enabled.
    pub liveness_enabled: bool,
    /// Minimum mean eye landmark displacement (pixels) for liveness check.
    /// Lower values are more permissive; higher values reject more aggressively.
    /// Only used when `liveness_enabled` is true.
    pub liveness_min_displacement: f32,
    /// Where and how the face models run: `VISAGE_INFERENCE_DEVICE` (`cpu` or
    /// `gpu`), `VISAGE_CPU_THREADS`, and `VISAGE_INFERENCE_CACHE_DIR` for
    /// compiled GPU kernels.
    pub inference: visage_core::InferenceOptions,
    /// Whether the daemon is running on the session bus (development mode).
    /// UID validation is skipped on the session bus — all callers share the same user.
    pub session_bus: bool,
}

impl Config {
    /// Load configuration from `VISAGE_*` environment variables with defaults.
    pub fn from_env() -> Self {
        let model_dir = std::env::var("VISAGE_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| visage_core::default_model_dir());

        let data_dir = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".local/share")
            })
            .join("visage");

        let db_path = std::env::var("VISAGE_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("faces.db"));

        Self {
            camera_device: std::env::var("VISAGE_CAMERA_DEVICE")
                .unwrap_or_else(|_| "/dev/video2".to_string()),
            db_path,
            similarity_threshold: env_f32("VISAGE_SIMILARITY_THRESHOLD", 0.40),
            verify_timeout_secs: env_u64("VISAGE_VERIFY_TIMEOUT_SECS", 10),
            warmup_frames: env_usize("VISAGE_WARMUP_FRAMES", 4),
            frames_per_verify: env_usize("VISAGE_FRAMES_PER_VERIFY", 3),
            frames_per_enroll: env_usize("VISAGE_FRAMES_PER_ENROLL", 5),
            // 1.5s is long enough to see yourself and move, short enough that
            // four labels do not make onboarding feel slow. Bounded by time
            // rather than frames on purpose: see `Camera::stream_frames_for`.
            framing_duration: std::time::Duration::from_millis(env_u64("VISAGE_FRAMING_MS", 1500)),
            emitter_enabled: std::env::var("VISAGE_EMITTER_ENABLED")
                .map(|v| v != "0")
                .unwrap_or(true),
            liveness_enabled: std::env::var("VISAGE_LIVENESS_ENABLED")
                .map(|v| v != "0")
                .unwrap_or(true),
            liveness_min_displacement: env_f32("VISAGE_LIVENESS_MIN_DISPLACEMENT", 0.8),
            inference: visage_core::InferenceOptions {
                device: parse_device(std::env::var("VISAGE_INFERENCE_DEVICE").ok().as_deref()),
                cpu_threads: env_usize("VISAGE_CPU_THREADS", 2).max(1),
                cache_dir: Some(
                    std::env::var("VISAGE_INFERENCE_CACHE_DIR")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| default_inference_cache_dir(&model_dir)),
                ),
            },
            model_dir,
            session_bus: parse_session_bus(std::env::var("VISAGE_SESSION_BUS").ok().as_deref()),
        }
    }

    /// Path to the SCRFD detection model.
    pub fn scrfd_model_path(&self) -> String {
        self.model_dir
            .join("det_10g.onnx")
            .to_string_lossy()
            .into_owned()
    }

    /// Path to the ArcFace recognition model.
    pub fn arcface_model_path(&self) -> String {
        self.model_dir
            .join("w600k_r50.onnx")
            .to_string_lossy()
            .into_owned()
    }
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Parse `VISAGE_INFERENCE_DEVICE`.
///
/// Unset means the CPU. An unknown value also means the CPU, with a warning:
/// a typo in a tuning knob must never keep the daemon from starting, which
/// would lock users out of face authentication entirely.
fn parse_device(value: Option<&str>) -> visage_core::Device {
    match value.map(str::parse::<visage_core::Device>) {
        None => visage_core::Device::Cpu,
        Some(Ok(device)) => device,
        Some(Err(e)) => {
            tracing::warn!(error = %e, "VISAGE_INFERENCE_DEVICE ignored, using the CPU");
            visage_core::Device::Cpu
        }
    }
}

/// Compiled GPU kernels live next to the models (`/var/lib/visage/gpu-cache`
/// for the packaged daemon), inside the state directory the service may write.
fn default_inference_cache_dir(model_dir: &std::path::Path) -> PathBuf {
    model_dir.parent().unwrap_or(model_dir).join("gpu-cache")
}

/// Parse the `VISAGE_SESSION_BUS` value into the session-bus flag.
///
/// Security-sensitive: session-bus mode *skips* D-Bus caller-UID validation
/// (see `dbus_interface::verify` and `require_root_caller`), so it must default
/// to `false` (system bus, validation ON) and enable only on an explicit opt-in.
/// An unset value, an empty value, and the literal `"0"` all keep it OFF — only a
/// non-empty, non-`"0"` value (e.g. `"1"`) turns it on.
///
/// Previously this used `env::var(..).is_ok()`, so *any* presence of the
/// variable — including `VISAGE_SESSION_BUS=0`, the natural way to try to turn it
/// off — enabled session-bus mode and silently disabled UID validation: a
/// fail-open trap. This helper closes it.
fn parse_session_bus(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.is_empty() && v != "0")
}

#[cfg(test)]
mod tests {
    use super::{default_inference_cache_dir, parse_device, parse_session_bus};
    use std::path::Path;
    use visage_core::Device;

    #[test]
    fn inference_device_defaults_to_cpu_and_never_fails_on_a_typo() {
        assert_eq!(parse_device(None), Device::Cpu);
        assert_eq!(parse_device(Some("gpu")), Device::Gpu);
        assert_eq!(parse_device(Some("GPU")), Device::Gpu);
        assert_eq!(parse_device(Some("cpu")), Device::Cpu);
        // Unknown values fall back rather than refusing to start.
        assert_eq!(parse_device(Some("gpuu")), Device::Cpu);
        assert_eq!(parse_device(Some("")), Device::Cpu);
    }

    #[test]
    fn gpu_cache_sits_beside_the_models_in_the_state_directory() {
        // The packaged service may only write under /var/lib/visage.
        assert_eq!(
            default_inference_cache_dir(Path::new("/var/lib/visage/models")),
            Path::new("/var/lib/visage/gpu-cache")
        );
    }

    #[test]
    fn session_bus_defaults_off_and_respects_zero() {
        // Secure default: absent, empty, or "0" → system bus (UID validation ON).
        assert!(
            !parse_session_bus(None),
            "unset must not enable session bus"
        );
        assert!(
            !parse_session_bus(Some("")),
            "empty must not enable session bus"
        );
        assert!(
            !parse_session_bus(Some("0")),
            "VISAGE_SESSION_BUS=0 must NOT enable session bus (was a fail-open trap)"
        );
        // Explicit opt-in: any non-empty, non-"0" value enables dev mode.
        assert!(
            parse_session_bus(Some("1")),
            "\"1\" must enable session bus"
        );
        assert!(
            parse_session_bus(Some("true")),
            "any other non-empty value enables session bus"
        );
    }
}

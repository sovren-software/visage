//! pam_visage — PAM module for Visage biometric authentication.
//!
//! This is a thin client that calls visaged over D-Bus.
//! The PAM module never owns the camera or runs inference directly.
//!
//! # PAM integration
//!
//! Install to /usr/lib/security/pam_visage.so
//! Add to PAM config: `auth sufficient pam_visage.so`

// TODO: Implement pam_sm_authenticate using pam-rs or raw FFI
// TODO: D-Bus client to call org.freedesktop.Visage1.Verify()
// TODO: Timeout handling (never block PAM indefinitely)
// TODO: Return PAM_IGNORE on failure for safe fallback

/// Placeholder — will be replaced with actual PAM entry points.
#[no_mangle]
pub extern "C" fn pam_sm_authenticate() -> i32 {
    // PAM_IGNORE = 25 — tells PAM to skip this module and continue
    25
}

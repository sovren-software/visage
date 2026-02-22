//! pam_visage — PAM module for Visage biometric authentication.
//!
//! Thin D-Bus client that calls visaged over the system bus.
//! The PAM module never owns the camera or runs inference directly.
//!
//! # Safety
//!
//! All Rust logic is wrapped in `catch_unwind` — a panic unwinding across the
//! `extern "C"` boundary would be undefined behavior.
//!
//! Every error path returns `PAM_IGNORE` (25), which tells the PAM stack to
//! skip this module and continue to the next (e.g., password). We never return
//! `PAM_AUTH_ERR` to avoid locking the user out if the daemon is unavailable.

use std::ffi::CStr;
use std::panic;

// PAM constants
const PAM_SUCCESS: libc::c_int = 0;
const PAM_IGNORE: libc::c_int = 25;
extern "C" {
    fn pam_get_user(
        pamh: *mut libc::c_void,
        user: *mut *const libc::c_char,
        prompt: *const libc::c_char,
    ) -> libc::c_int;
}

// D-Bus proxy — generates VisageProxyBlocking for synchronous calls.
#[zbus::proxy(
    interface = "org.freedesktop.Visage1",
    default_service = "org.freedesktop.Visage1",
    default_path = "/org/freedesktop/Visage1"
)]
trait Visage {
    async fn verify(&self, user: &str) -> zbus::Result<bool>;
}

/// Connect to the system bus and call Visage1.Verify(username).
fn verify_face(username: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::system()?;
    let proxy = VisageProxyBlocking::new(&conn)?;
    let matched = proxy.verify(username)?;
    Ok(matched)
}

/// PAM authentication entry point.
///
/// Called by the PAM stack when `auth sufficient pam_visage.so` is configured.
/// Extracts the username via `pam_get_user`, then calls `visaged` over D-Bus.
///
/// Returns:
/// - `PAM_SUCCESS` (0) if face matched
/// - `PAM_IGNORE` (25) on any failure — daemon down, no match, error, panic
///
/// # Safety
///
/// `pamh` must be a valid PAM handle provided by the PAM framework.
/// This function is called by the PAM stack via `dlopen` — it must never
/// panic across the FFI boundary (enforced by `catch_unwind`).
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut libc::c_void,
    _flags: libc::c_int,
    _argc: libc::c_int,
    _argv: *const *const libc::c_char,
) -> libc::c_int {
    let result = panic::catch_unwind(|| {
        // Extract username from PAM handle
        let mut user_ptr: *const libc::c_char = std::ptr::null();
        let ret = pam_get_user(pamh, &mut user_ptr, std::ptr::null());
        if ret != PAM_SUCCESS || user_ptr.is_null() {
            eprintln!("pam_visage: failed to get username (ret={})", ret);
            return PAM_IGNORE;
        }

        let username = match CStr::from_ptr(user_ptr).to_str() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("pam_visage: username is not valid UTF-8");
                return PAM_IGNORE;
            }
        };

        // Call visaged over D-Bus
        match verify_face(username) {
            Ok(true) => {
                eprintln!("pam_visage: face matched for user '{}'", username);
                PAM_SUCCESS
            }
            Ok(false) => {
                eprintln!("pam_visage: no match for user '{}'", username);
                PAM_IGNORE
            }
            Err(e) => {
                eprintln!("pam_visage: error: {}", e);
                PAM_IGNORE
            }
        }
    });

    result.unwrap_or(PAM_IGNORE)
}

/// PAM credential management entry point (required by the PAM ABI).
///
/// Visage does not manage credentials — always returns `PAM_IGNORE`.
///
/// # Safety
///
/// `_pamh` must be a valid PAM handle. This function is a no-op stub.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_setcred(
    _pamh: *mut libc::c_void,
    _flags: libc::c_int,
    _argc: libc::c_int,
    _argv: *const *const libc::c_char,
) -> libc::c_int {
    PAM_IGNORE
}

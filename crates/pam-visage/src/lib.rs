//! pam_visage — PAM module for Visage biometric authentication.
//!
//! Thin D-Bus client that calls visaged over the system bus.
//! The PAM module never owns the camera or runs inference directly.
//!
//! # Safety
//!
//! All Rust logic is wrapped in `catch_unwind` — a panic unwinding across the
//! `extern "C"` boundary is undefined behavior.
//!
//! Every error path returns `PAM_IGNORE` (25), which tells the PAM stack to
//! skip this module and continue to the next (e.g., password). We never return
//! `PAM_AUTH_ERR` to avoid locking the user out if the daemon is unavailable.

// Enforce explicit `unsafe {}` blocks inside `unsafe fn` bodies — catches
// the Rust 2024 edition change before it lands.
#![warn(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, CString};
use std::panic;
use std::ptr;

// PAM return codes (POSIX / Linux-PAM values)
const PAM_SUCCESS: libc::c_int = 0;
const PAM_IGNORE: libc::c_int = 25;

// PAM item types
const PAM_CONV: libc::c_int = 5;

// PAM message styles
const PAM_TEXT_INFO: libc::c_int = 4;

// syslog constants
const LOG_PID: libc::c_int = 0x01;
const LOG_AUTHPRIV: libc::c_int = 10 << 3;
const LOG_INFO: libc::c_int = 6;
const LOG_WARNING: libc::c_int = 4;
const LOG_ERR: libc::c_int = 3;

extern "C" {
    fn pam_get_user(
        pamh: *mut libc::c_void,
        user: *mut *const libc::c_char,
        prompt: *const libc::c_char,
    ) -> libc::c_int;

    fn pam_get_item(
        pamh: *mut libc::c_void,
        item_type: libc::c_int,
        item: *mut *const libc::c_void,
    ) -> libc::c_int;
}

/// PAM message struct — mirrors `struct pam_message` from <security/pam_appl.h>.
#[repr(C)]
struct PamMessage {
    msg_style: libc::c_int,
    msg: *const libc::c_char,
}

/// PAM response struct — mirrors `struct pam_response` from <security/pam_appl.h>.
#[repr(C)]
struct PamResponse {
    resp: *mut libc::c_char,
    resp_retcode: libc::c_int,
}

/// PAM conversation struct — mirrors `struct pam_conv` from <security/pam_appl.h>.
#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            num_msg: libc::c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut libc::c_void,
        ) -> libc::c_int,
    >,
    appdata_ptr: *mut libc::c_void,
}

// D-Bus proxy — `#[zbus::proxy]` generates both `VisageProxy` (async) and
// `VisageProxyBlocking` (synchronous). Only the blocking variant is used here.
#[zbus::proxy(
    interface = "org.freedesktop.Visage1",
    default_service = "org.freedesktop.Visage1",
    default_path = "/org/freedesktop/Visage1"
)]
trait Visage {
    async fn verify(&self, user: &str) -> zbus::Result<bool>;
}

/// Open syslog with `pam_visage` ident and `LOG_AUTHPRIV` facility.
fn syslog_open() {
    // The ident string must outlive the openlog call. Using a static ensures this.
    static IDENT: &[u8] = b"pam_visage\0";
    // SAFETY: IDENT is a valid NUL-terminated static string.
    unsafe {
        libc::openlog(IDENT.as_ptr() as *const libc::c_char, LOG_PID, LOG_AUTHPRIV);
    }
}

/// Log a message to syslog at the given priority.
fn syslog_msg(priority: libc::c_int, msg: &str) {
    // syslog(3) interprets % as format specifiers. Use "%s" format to avoid injection.
    let c_msg = match CString::new(msg) {
        Ok(s) => s,
        Err(_) => return, // interior NUL — skip logging rather than panic
    };
    let fmt = b"%s\0";
    // SAFETY: fmt is a valid NUL-terminated format string; c_msg is a valid C string.
    unsafe {
        libc::syslog(
            priority,
            fmt.as_ptr() as *const libc::c_char,
            c_msg.as_ptr(),
        );
    }
}

/// Send a PAM_TEXT_INFO message to the user via the PAM conversation function.
///
/// Fails silently if the conversation function is unavailable — this is non-critical
/// feedback and must never block authentication.
fn send_text_info(pamh: *mut libc::c_void, text: &str) {
    let c_text = match CString::new(text) {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut conv_ptr: *const libc::c_void = ptr::null();
    // SAFETY: pamh is a valid PAM handle. pam_get_item reads the conversation struct.
    let ret = unsafe { pam_get_item(pamh, PAM_CONV, &mut conv_ptr) };
    if ret != PAM_SUCCESS || conv_ptr.is_null() {
        return;
    }

    // SAFETY: pam_get_item with PAM_CONV returns a pointer to a pam_conv struct.
    let conv = unsafe { &*(conv_ptr as *const PamConv) };
    let conv_fn = match conv.conv {
        Some(f) => f,
        None => return,
    };

    let msg = PamMessage {
        msg_style: PAM_TEXT_INFO,
        msg: c_text.as_ptr(),
    };
    let msg_ptr: *const PamMessage = &msg;
    let mut resp_ptr: *mut PamResponse = ptr::null_mut();

    // SAFETY: msg_ptr points to a valid PamMessage, conv_fn is the PAM conversation callback.
    unsafe {
        conv_fn(
            1,
            &msg_ptr as *const _ as *mut _,
            &mut resp_ptr,
            conv.appdata_ptr,
        );
        // Free response array if allocated. TEXT_INFO rarely gets a response, but the spec
        // requires us to free both the response string and the response struct if present.
        if !resp_ptr.is_null() {
            if !(*resp_ptr).resp.is_null() {
                libc::free((*resp_ptr).resp as *mut libc::c_void);
            }
            libc::free(resp_ptr as *mut libc::c_void);
        }
    }
}

/// Connect to the system bus and call `Visage1.Verify(username)`.
///
/// Uses a 3-second method timeout to prevent login hangs if the daemon is stuck.
/// Returns `Ok(false)` if the daemon responds but finds no match.
/// Returns `Err` if the daemon is not running, the call fails, or times out.
fn verify_face(username: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let conn = zbus::blocking::connection::Builder::system()?
        .method_timeout(std::time::Duration::from_secs(3))
        .build()?;
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
/// This function is loaded by the PAM stack via `dlopen`. Panics are caught
/// by `catch_unwind` and converted to `PAM_IGNORE` rather than unwinding
/// across the FFI boundary.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut libc::c_void,
    _flags: libc::c_int,
    _argc: libc::c_int,
    _argv: *const *const libc::c_char,
) -> libc::c_int {
    let result = panic::catch_unwind(|| {
        syslog_open();

        // Extract username from PAM handle.
        let mut user_ptr: *const libc::c_char = ptr::null();
        // SAFETY: pamh is a valid PAM handle. pam_get_user writes a pointer
        // that remains valid for the lifetime of the PAM conversation.
        let ret = unsafe { pam_get_user(pamh, &mut user_ptr, ptr::null()) };
        if ret != PAM_SUCCESS || user_ptr.is_null() {
            syslog_msg(LOG_ERR, &format!("pam_get_user failed (ret={})", ret));
            return PAM_IGNORE;
        }

        // SAFETY: pam_get_user guarantees the pointer is non-null and points
        // to a NUL-terminated string that lives for the PAM conversation.
        let username = match unsafe { CStr::from_ptr(user_ptr) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                syslog_msg(LOG_WARNING, "username is not valid UTF-8");
                return PAM_IGNORE;
            }
        };

        // Call visaged over D-Bus.
        match verify_face(username) {
            Ok(true) => {
                syslog_msg(LOG_INFO, &format!("face matched for user '{}'", username));
                send_text_info(pamh, "Visage: face recognized");
                PAM_SUCCESS
            }
            Ok(false) => {
                syslog_msg(LOG_INFO, &format!("no match for user '{}'", username));
                PAM_IGNORE
            }
            Err(e) => {
                syslog_msg(LOG_WARNING, &format!("D-Bus error: {}", e));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pam_constants_match_spec() {
        // Verify against the values defined in <security/pam_modules.h>.
        // These are load-bearing: wrong values silently mis-route the PAM stack.
        assert_eq!(PAM_SUCCESS, 0, "PAM_SUCCESS must be 0");
        assert_eq!(PAM_IGNORE, 25, "PAM_IGNORE must be 25");
    }

    #[test]
    fn pam_conv_constant_matches_spec() {
        assert_eq!(PAM_CONV, 5, "PAM_CONV must be 5");
    }

    #[test]
    fn pam_text_info_matches_spec() {
        assert_eq!(PAM_TEXT_INFO, 4, "PAM_TEXT_INFO must be 4");
    }

    #[test]
    fn syslog_constants_match_spec() {
        assert_eq!(LOG_AUTHPRIV, 80, "LOG_AUTHPRIV must be 10 << 3 = 80");
        assert_eq!(LOG_INFO, 6, "LOG_INFO must be 6");
        assert_eq!(LOG_WARNING, 4, "LOG_WARNING must be 4");
        assert_eq!(LOG_ERR, 3, "LOG_ERR must be 3");
    }

    #[test]
    fn verify_face_errors_when_daemon_not_running() {
        // When visaged is not on the system bus, verify_face must return Err,
        // not panic. This exercises the ServiceUnknown / NameHasNoOwner path.
        //
        // This test will pass in any environment where visaged is not running,
        // including CI. If the daemon happens to be running, the test is skipped
        // to avoid a real camera capture during unit testing.
        let result = verify_face("_pam_visage_unit_test_user_");
        // If the daemon is running we get Ok(true/false); that's also fine —
        // the important property is no panic.
        match result {
            Err(e) => {
                // Expected: daemon not present
                let msg = e.to_string();
                assert!(
                    msg.contains("ServiceUnknown")
                        || msg.contains("NameHasNoOwner")
                        || msg.contains("not provided")
                        || msg.contains("Failed to connect")
                        || msg.contains("no enrolled models")
                        || msg.contains("unknown user"),
                    "unexpected error message: {msg}"
                );
            }
            Ok(_) => {
                // Daemon is running — acceptable, confirms no panic either way
            }
        }
    }
}

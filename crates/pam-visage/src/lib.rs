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

// Retry pacing for the verification loop (see `pam_sm_authenticate`).
//
// Transient failures — nobody in front of the camera, the camera still
// resuming from suspend, `visaged` restarting after wake — are retried
// quickly, so the first unlock attempt after opening the lid succeeds instead
// of racing the camera warmup. A deliberate face-not-match is retried at most
// once, slowly, and then handed to the password prompt: hammering a wrong
// face only burns visaged's rate-limit budget (deliberate non-matches are
// what trigger its lockout; transient errors never count).
const FAST_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);
const SLOW_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// Errors that no retry can fix, each with the reason logged when it ends the
/// loop. Every other error is treated as transient and retried until the
/// `timeout=N` budget runs out.
///
/// Each needle is a substring of an error visaged (or zbus, for
/// `AccessDenied`) produces today. `crates/visaged/tests/pam_retry_contract.rs`
/// fails if any of them stops appearing in the daemon's source, so a reworded
/// message cannot quietly turn a lockout into a retry loop.
const PERMANENT_ERRORS: &[(&str, &str)] = &[
    // An active lockout must not be hammered: visaged owns the counter.
    ("too many failed attempts", "rate limited by visaged"),
    ("no enrolled models", "no enrolled models for user"),
    ("unknown user", "unknown user"),
    // The caller is not allowed to ask; asking again changes nothing.
    ("AccessDenied", "verify refused"),
];

/// The log reason if `msg` is an error that retrying cannot fix.
fn permanent_error(msg: &str) -> Option<&'static str> {
    PERMANENT_ERRORS
        .iter()
        .find(|(needle, _)| msg.contains(needle))
        .map(|(_, reason)| *reason)
}

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

/// Default D-Bus method timeout, in seconds.
///
/// This bounds how long a stuck `visaged` can hang a login. It is *also* an
/// upper bound on how long verification is allowed to take: a verify that
/// exceeds it makes PAM fall through to the password prompt, and that outcome
/// is indistinguishable from a failed match — both simply return
/// `PAM_IGNORE`. On CPU-only hardware a verify can legitimately take seconds,
/// so a deployment whose measured latency approaches this value should raise
/// it with `pam_visage.so timeout=N` rather than accept intermittent password
/// prompts it cannot diagnose.
const DEFAULT_TIMEOUT_SECS: u64 = 3;

/// Parse `timeout=N` out of the PAM module arguments.
///
/// Unrecognised arguments are ignored. A malformed, zero, or unparseable
/// value logs a warning and falls back to [`DEFAULT_TIMEOUT_SECS`]: a typo in
/// a PAM config line must never be able to lock a user out, and refusing to
/// authenticate is a worse failure than using a slightly wrong timeout.
///
/// # Safety
///
/// `argv` must be null, or point to `argc` valid NUL-terminated C strings.
unsafe fn parse_timeout_secs(argc: libc::c_int, argv: *const *const libc::c_char) -> u64 {
    if argv.is_null() || argc <= 0 {
        return DEFAULT_TIMEOUT_SECS;
    }
    for i in 0..argc as isize {
        // SAFETY: caller guarantees argv holds argc valid pointers.
        let arg_ptr = unsafe { *argv.offset(i) };
        if arg_ptr.is_null() {
            continue;
        }
        // SAFETY: PAM module arguments are NUL-terminated C strings.
        let arg = match unsafe { CStr::from_ptr(arg_ptr) }.to_str() {
            Ok(a) => a,
            Err(_) => continue,
        };
        if let Some(value) = arg.strip_prefix("timeout=") {
            return match value.parse::<u64>() {
                Ok(secs) if secs > 0 => secs,
                _ => {
                    syslog_msg(
                        LOG_WARNING,
                        &format!("ignoring invalid module argument '{}'", arg),
                    );
                    DEFAULT_TIMEOUT_SECS
                }
            };
        }
    }
    DEFAULT_TIMEOUT_SECS
}

/// Connect to the system bus and call `Visage1.Verify(username)`.
///
/// `timeout` bounds the D-Bus method call so a stuck daemon cannot hang
/// the login. Returns `Ok(false)` if the daemon responds but finds no match.
/// Returns `Err` if the daemon is not running, the call fails, or times out.
fn verify_face(
    username: &str,
    timeout: std::time::Duration,
) -> Result<bool, Box<dyn std::error::Error>> {
    let conn = zbus::blocking::connection::Builder::system()?
        .method_timeout(timeout)
        .build()?;
    let proxy = VisageProxyBlocking::new(&conn)?;
    let matched = proxy.verify(username)?;
    Ok(matched)
}

/// What [`verify_until`] concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// A face matched: `PAM_SUCCESS`.
    Matched,
    /// Anything else: `PAM_IGNORE`, so the password prompt follows.
    Ignore,
}

/// The deadline-bounded retry loop. The policy is described where
/// `pam_sm_authenticate` calls it.
///
/// The clock, the daemon call and the sleep are parameters, so the tests run
/// this exact loop on a fake clock. `verify` gets the time left and returns
/// `Ok(matched)` or the error text.
///
/// No path sleeps to or past `deadline`. A wait that ends there leaves no
/// time for another verify, so it would only hold the password prompt back.
fn verify_until(
    username: &str,
    deadline: std::time::Instant,
    now: impl Fn() -> std::time::Instant,
    mut verify: impl FnMut(std::time::Duration) -> Result<bool, String>,
    mut sleep: impl FnMut(std::time::Duration),
    mut log: impl FnMut(libc::c_int, &str),
) -> Verdict {
    let mut slow_retry_used = false;
    loop {
        let remaining = deadline.saturating_duration_since(now());
        if remaining.is_zero() {
            log(LOG_INFO, "verification budget exhausted before a match");
            return Verdict::Ignore;
        }

        match verify(remaining) {
            Ok(true) => {
                log(LOG_INFO, &format!("face matched for user '{}'", username));
                return Verdict::Matched;
            }
            Ok(false) => {
                log(LOG_INFO, &format!("no match for user '{}'", username));
                if slow_retry_used {
                    return Verdict::Ignore;
                }
                if now() + SLOW_RETRY_DELAY >= deadline {
                    log(LOG_INFO, "no time left in the budget for a second attempt");
                    return Verdict::Ignore;
                }
                slow_retry_used = true;
                sleep(SLOW_RETRY_DELAY);
            }
            Err(msg) => {
                if let Some(reason) = permanent_error(&msg) {
                    log(LOG_WARNING, &format!("{reason}: {msg}"));
                    return Verdict::Ignore;
                }
                // Everything else is transient: no face in frame, dark
                // frames, camera resuming, visaged restarting after wake.
                log(LOG_INFO, &format!("verify failed, retrying: {}", msg));
                if now() + FAST_RETRY_DELAY >= deadline {
                    log(LOG_INFO, "verification budget exhausted before a match");
                    return Verdict::Ignore;
                }
                sleep(FAST_RETRY_DELAY);
            }
        }
    }
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
    argc: libc::c_int,
    argv: *const *const libc::c_char,
) -> libc::c_int {
    let result = panic::catch_unwind(|| {
        syslog_open();

        // Parsed after syslog_open so a bad argument can be reported.
        // SAFETY: PAM guarantees argv holds argc NUL-terminated strings.
        let timeout_secs = unsafe { parse_timeout_secs(argc, argv) };

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

        // Deadline-bounded verification loop.
        //
        // visaged's Verify is one-shot: it captures frames and returns a
        // single outcome. The retry policy lives here because only the PAM
        // layer knows the user's patience budget (`timeout=N`):
        //
        //   * transient failures — no face in frame, dark or unreadable
        //     frames, camera still resuming from suspend, visaged restarting
        //     after wake — are retried quickly; visaged does NOT count them
        //     toward its lockout, and this is what makes the first unlock
        //     after opening the lid succeed instead of racing the camera
        //     warmup;
        //   * a deliberate face-not-match (face seen, did not match) is
        //     retried once, slowly, and then handed to the password prompt —
        //     visaged counts those toward its lockout, so hammering is
        //     counterproductive;
        //   * an active rate-limit lockout is not hammered at all.
        //
        // Every exhausted or failed path returns PAM_IGNORE — the password
        // prompt is always the fallback and the user is never locked out
        // by this module.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        let verdict = verify_until(
            username,
            deadline,
            std::time::Instant::now,
            |remaining| verify_face(username, remaining).map_err(|e| e.to_string()),
            std::thread::sleep,
            syslog_msg,
        );
        if verdict == Verdict::Matched {
            send_text_info(pamh, "Visage: face recognized");
            PAM_SUCCESS
        } else {
            PAM_IGNORE
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
        let result = verify_face(
            "_pam_visage_unit_test_user_",
            std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        );
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
                        || msg.contains("unknown user")
                        // No system bus socket at all — the D-Bus client fails at
                        // open() with ENOENT rather than reaching a name-resolution
                        // error. This is the normal case in a hermetic build sandbox,
                        // a container, or a chroot, where /run/dbus/system_bus_socket
                        // does not exist. The list above only covered environments
                        // where the bus is present but visaged is not on it, so this
                        // test failed the whole build under `nix build` and would do
                        // the same in any containerised CI.
                        || msg.contains("No such file or directory"),
                    "unexpected error message: {msg}"
                );
            }
            Ok(_) => {
                // Daemon is running — acceptable, confirms no panic either way
            }
        }
    }

    /// Messages as `e.to_string()` renders them for the errors that must end
    /// the loop at once.
    #[test]
    fn permanent_errors_end_the_loop() {
        for msg in [
            "org.freedesktop.DBus.Error.Failed: too many failed attempts; try again in 30s",
            "org.freedesktop.DBus.Error.Failed: no enrolled models for user 'cc'",
            "org.freedesktop.DBus.Error.Failed: unknown user 'ghost'",
            "org.freedesktop.DBus.Error.AccessDenied: caller uid 1000 may not verify 'root'",
        ] {
            assert!(permanent_error(msg).is_some(), "should not retry: {msg}");
        }
    }

    /// The control for the test above: the errors #127 exists to retry must
    /// stay retryable, or the loop would give up on the first dark frame.
    #[test]
    fn transient_errors_are_retried() {
        for msg in [
            "org.freedesktop.DBus.Error.Failed: no face detected in any captured frame",
            "org.freedesktop.DBus.Error.Failed: no usable frames captured (camera returned only dark or unreadable frames)",
            "org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.Visage1 was not provided by any .service files",
            "org.freedesktop.DBus.Error.NoReply: Did not receive a reply",
        ] {
            assert_eq!(permanent_error(msg), None, "should retry: {msg}");
        }
    }

    fn ms(n: u64) -> std::time::Duration {
        std::time::Duration::from_millis(n)
    }

    /// What one run of [`verify_until`] did on the fake clock.
    struct Run {
        verdict: Verdict,
        elapsed: std::time::Duration,
        calls: usize,
        sleeps: Vec<std::time::Duration>,
    }

    /// Run the real loop on a fake clock. Each daemon call takes `call`, cut
    /// short at the time left as the D-Bus method timeout does, and answers
    /// from `answers`, repeating the last one. Nothing really waits.
    fn run(budget: u64, call: u64, answers: &[Result<bool, &str>]) -> Run {
        let t0 = std::time::Instant::now();
        let elapsed = std::cell::Cell::new(std::time::Duration::ZERO);
        let calls = std::cell::Cell::new(0usize);
        let mut sleeps = Vec::new();
        let deadline = t0 + ms(budget);
        let verdict = verify_until(
            "cc",
            deadline,
            || t0 + elapsed.get(),
            |remaining| {
                let i = calls.get();
                calls.set(i + 1);
                if ms(call) > remaining {
                    elapsed.set(elapsed.get() + remaining);
                    return Err("org.freedesktop.DBus.Error.NoReply: timed out".to_string());
                }
                elapsed.set(elapsed.get() + ms(call));
                answers[i.min(answers.len() - 1)].map_err(str::to_string)
            },
            |d| {
                sleeps.push(d);
                elapsed.set(elapsed.get() + d);
            },
            |_, _| {},
        );
        Run {
            verdict,
            elapsed: elapsed.get(),
            calls: calls.get(),
            sleeps,
        }
    }

    const NO_FACE: &str =
        "org.freedesktop.DBus.Error.Failed: no face detected in any captured frame";

    /// The defect this test pins: a non-match late in the budget slept 1.5 s
    /// past `timeout=N`, then gave up anyway. The numbers are the-first's: a
    /// full verify takes about 2.3 s, and its lock screen uses the 3 s default.
    #[test]
    fn a_late_non_match_does_not_sleep_past_the_budget() {
        let r = run(3000, 2300, &[Ok(false)]);
        assert_eq!(r.verdict, Verdict::Ignore);
        assert_eq!(r.calls, 1);
        assert!(
            r.sleeps.is_empty(),
            "slept {:?} with no time to use it",
            r.sleeps
        );
        assert_eq!(r.elapsed, ms(2300));
    }

    /// Control for the test above: with room left, a non-match still gets its
    /// one slow retry.
    #[test]
    fn a_non_match_with_room_left_is_retried_once() {
        let r = run(6000, 2000, &[Ok(false)]);
        assert_eq!(r.verdict, Verdict::Ignore);
        assert_eq!(r.calls, 2);
        assert_eq!(r.sleeps, vec![SLOW_RETRY_DELAY]);
        assert_eq!(r.elapsed, ms(5500));
    }

    #[test]
    fn a_match_on_the_slow_retry_succeeds() {
        let r = run(6000, 2000, &[Ok(false), Ok(true)]);
        assert_eq!(r.verdict, Verdict::Matched);
        assert_eq!(r.calls, 2);
    }

    #[test]
    fn transient_errors_are_retried_until_the_budget_ends() {
        let r = run(3000, 800, &[Err(NO_FACE)]);
        assert_eq!(r.verdict, Verdict::Ignore);
        // 0-800 ms, wait to 1000; 1000-1800, wait to 2000; 2000-2800, and a
        // wait to 3000 would leave nothing, so it stops.
        assert_eq!(r.calls, 3);
        assert_eq!(r.sleeps, vec![FAST_RETRY_DELAY; 2]);
        assert_eq!(r.elapsed, ms(2800));
    }

    #[test]
    fn a_permanent_error_ends_the_loop_at_once() {
        let r = run(
            6000,
            100,
            &[Err(
                "org.freedesktop.DBus.Error.Failed: too many failed attempts; try again in 30s",
            )],
        );
        assert_eq!((r.verdict, r.calls), (Verdict::Ignore, 1));
        assert!(r.sleeps.is_empty());
        assert_eq!(r.elapsed, ms(100));
    }

    /// `timeout=N` is a total budget: whatever the daemon answers and however
    /// long each call takes, the loop never runs past the deadline.
    #[test]
    fn no_answer_sequence_runs_past_the_budget() {
        let sequences: [&[Result<bool, &str>]; 4] = [
            &[Ok(false)],
            &[Err(NO_FACE)],
            &[Err(NO_FACE), Ok(false)],
            &[Ok(false), Err(NO_FACE)],
        ];
        let mut runs = 0;
        for budget in [1000, 3000, 6000] {
            for call in (100..=3000).step_by(100) {
                for answers in sequences {
                    let r = run(budget, call, answers);
                    assert!(
                        r.elapsed <= ms(budget),
                        "budget {budget} ms, call {call} ms, {answers:?}: ran {:?}",
                        r.elapsed
                    );
                    runs += 1;
                }
            }
        }
        assert_eq!(runs, 3 * 30 * 4, "the sweep must cover every case");
    }

    /// Build a C-style argv from Rust strings for the parser tests.
    fn with_argv<T>(
        args: &[&str],
        f: impl FnOnce(libc::c_int, *const *const libc::c_char) -> T,
    ) -> T {
        let owned: Vec<std::ffi::CString> = args
            .iter()
            .map(|a| std::ffi::CString::new(*a).unwrap())
            .collect();
        let ptrs: Vec<*const libc::c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        f(ptrs.len() as libc::c_int, ptrs.as_ptr())
    }

    #[test]
    fn timeout_defaults_when_absent() {
        // No args at all, and args that simply do not mention timeout.
        assert_eq!(
            unsafe { parse_timeout_secs(0, std::ptr::null()) },
            DEFAULT_TIMEOUT_SECS
        );
        with_argv(&["debug", "use_first_pass"], |argc, argv| {
            assert_eq!(
                unsafe { parse_timeout_secs(argc, argv) },
                DEFAULT_TIMEOUT_SECS
            );
        });
    }

    #[test]
    fn timeout_is_parsed_when_given() {
        // The control for the test above: a valid argument MUST change the result,
        // otherwise `timeout_defaults_when_absent` would pass on a parser that
        // never reads anything at all.
        with_argv(&["timeout=9"], |argc, argv| {
            assert_eq!(unsafe { parse_timeout_secs(argc, argv) }, 9);
        });
        with_argv(&["debug", "timeout=12", "other"], |argc, argv| {
            assert_eq!(unsafe { parse_timeout_secs(argc, argv) }, 12);
        });
    }

    #[test]
    fn malformed_timeout_falls_back_rather_than_failing() {
        // A typo in a PAM line must never be able to lock a user out, so every
        // one of these degrades to the default instead of erroring.
        for bad in [
            "timeout=",
            "timeout=0",
            "timeout=-1",
            "timeout=abc",
            "timeout=9999999999999999999999",
        ] {
            with_argv(&[bad], |argc, argv| {
                assert_eq!(
                    unsafe { parse_timeout_secs(argc, argv) },
                    DEFAULT_TIMEOUT_SECS,
                    "argument {bad:?} should fall back to the default"
                );
            });
        }
    }
}

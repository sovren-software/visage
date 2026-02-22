# ADR 005 — PAM Module and System Bus Migration

**Status:** Accepted
**Date:** 2026-02-22
**Step:** 4 — PAM module (`pam-visage`) + system bus migration (visaged, visage-cli)

---

## Context

Steps 1–3 produced a working camera pipeline, ONNX inference engine, and daemon
(`visaged`) on the session bus. Step 4 makes `sudo echo test` authenticate via face
recognition. This requires two changes:

1. **System bus migration.** PAM modules execute in a context with no session bus. The
   daemon must register on the system bus, and the CLI must connect to it by default.

2. **PAM module implementation.** The stub (`pam-visage`) had zero-argument C exports
   returning `PAM_IGNORE`. A correct PAM module requires the 4-argument C ABI, username
   extraction via `pam_get_user`, a synchronous D-Bus call, and safe fallback semantics.

---

## Decisions

### 1. System bus as default; `VISAGE_SESSION_BUS` env var for development

**Decision:** Both `visaged` and `visage-cli` connect to the system bus by default.
Setting `VISAGE_SESSION_BUS=1` falls back to the session bus.

**Rationale:** PAM modules are loaded by the PAM framework into the process invoking
`sudo` (e.g., `sudo echo test`). This process runs as root and has no user session
context. The D-Bus session bus is scoped to a user login session and is not available
in this context. The system bus is always reachable.

**Trade-off:** The system bus requires a D-Bus policy file to be installed at
`/usr/share/dbus-1/system.d/org.freedesktop.Visage1.conf` and the daemon must be
started with `sudo`. The session bus required neither. `VISAGE_SESSION_BUS=1` restores
the Step 3 development workflow.

**Alternative considered:** Detect the context at runtime (check `DBUS_SESSION_BUS_ADDRESS`).
Rejected — explicit configuration is clearer, and the system bus is correct for all
production deployment paths.

### 2. No async runtime in the PAM module — `zbus::blocking` only

**Decision:** `pam-visage` uses `zbus::blocking::Connection::system()` and
`VisageProxyBlocking`. There is no tokio runtime in the PAM module.

**Rationale:** PAM modules are synchronous by contract. Starting a tokio runtime inside
`pam_sm_authenticate` adds ~2–5 ms latency, requires runtime teardown on every PAM
call, and introduces thread-safety concerns with the PAM stack state. `zbus::blocking`
provides synchronous D-Bus access backed by an internal async executor that is
transparent to the caller.

**Trade-off:** The `#[zbus::proxy]` macro generates both `VisageProxy` (async) and
`VisageProxyBlocking`. The async variant is generated but unused in this crate. Dead
code elimination removes it from the `.so` at link time.

### 3. `PAM_IGNORE` on all failures — never `PAM_AUTH_ERR`

**Decision:** Every error path — daemon not running, D-Bus timeout, no face match,
panic recovery — returns `PAM_IGNORE` (25).

**Rationale:** `PAM_IGNORE` tells the PAM stack to skip this module and continue to the
next configured module (typically password). `PAM_AUTH_ERR` denies authentication
outright in some PAM stack configurations, which would lock the user out if the daemon
is unavailable. Since `visaged` can be absent (not yet installed, crashed, or
intentionally stopped), the PAM module must never be a single point of failure.

**Implication:** A user can always fall back to password authentication regardless of
the daemon's state. This is the correct security posture for a supplementary biometric
module.

### 4. `catch_unwind` around all Rust logic

**Decision:** The body of `pam_sm_authenticate` is wrapped in
`std::panic::catch_unwind`. Panics produce `PAM_IGNORE` rather than unwinding across
the `extern "C"` boundary.

**Rationale:** Unwinding a Rust panic across an `extern "C"` boundary is undefined
behavior per the Rust reference. The PAM stack does not have a Rust unwinding runtime.
A panic without `catch_unwind` would corrupt the calling process's stack.

**Implementation:** `catch_unwind` requires its closure to be `UnwindSafe`. Raw
pointers (`*mut libc::c_void`, `*const libc::c_char`) implement `UnwindSafe` because
they carry no ownership invariants that could be violated by an unwind.

### 5. Explicit `unsafe {}` blocks inside `catch_unwind` closure

**Decision:** `pam_get_user` and `CStr::from_ptr` are wrapped in explicit `unsafe {}`
blocks with `SAFETY` comments, even though the outer function is already `unsafe`.

**Rationale:** In Rust 2024 edition, `unsafe_op_in_unsafe_fn` becomes a hard error.
Unsafe calls inside an `unsafe fn` body without explicit `unsafe {}` blocks will not
compile. The crate enables `#![warn(unsafe_op_in_unsafe_fn)]` to catch this now.
Verified clean under `RUSTFLAGS="-D unsafe_op_in_unsafe_fn" cargo check`.

**Note:** The original implementation omitted these blocks. They were added as a fix
after self-evaluation of the Step 4 implementation.

### 6. `eprintln!` for development logging; syslog deferred to Step 6

**Decision:** `pam_visage` writes diagnostic messages to stderr via `eprintln!`.

**Rationale:** Step 6 will add `libc::syslog(LOG_AUTHPRIV, ...)` for production
logging. `eprintln!` is visible in the `sudo` terminal during development and requires
no additional dependency. Adding syslog at Step 6 alongside packaging is lower-risk than
doing it in Step 4.

**Known limitation:** In production, stderr from a PAM module is discarded. Messages
will not appear in any log until Step 6.

### 7. No PAM conversation API in v1

**Decision:** `pam_sm_authenticate` does not call the PAM conversation callback. Users
see either: (a) no password prompt (face matched), or (b) the normal password prompt
(face didn't match or daemon unavailable).

**Rationale:** The conversation API enables "Face recognized ✓" / "Face not recognized,
try password" feedback messages. It requires additional `pam_get_item(PAM_CONV)` +
`struct pam_conv *` FFI plumbing. Deferred to Step 6 alongside syslog and `pam-auth-update`.

### 8. Explicit `-lpam` via build.rs

**Decision:** `crates/pam-visage/build.rs` emits `cargo:rustc-link-lib=pam`.

**Rationale:** `pam_get_user` is declared as `extern "C"` in the Rust code but its
definition lives in `libpam.so`. As a `cdylib`, undefined symbols may resolve at `dlopen`
time via the process's already-loaded libpam. However, explicit linking catches missing
`libpam0g-dev` at build time rather than at first `sudo` attempt. It also documents the
dependency unambiguously.

---

## Known Limitations

| Limitation | Severity | Deferred to |
|-----------|----------|-------------|
| D-Bus timeout: 10–25 s if daemon deadlocks | Medium | Step 6 |
| No D-Bus caller authentication (`user` is caller-supplied) | Medium | Step 6 |
| `eprintln!` logging (not syslog) | Low | Step 6 |
| No PAM conversation API messages | Low | Step 6 |
| Manual `/etc/pam.d/sudo` edit; no `pam-auth-update` | Low | Step 6 |
| IR emitter not active (dark frames without ambient light) | Low | Step 5 |

---

## Consequences

- `sudo echo test` now attempts face authentication via `visaged` before prompting
  for a password. No password prompt appears if a face is matched.
- The daemon and CLI now require `sudo` for normal operation. Development without
  sudo is preserved via `VISAGE_SESSION_BUS=1`.
- The D-Bus policy file at `packaging/dbus/org.freedesktop.Visage1.conf` is now
  required for all deployments (previously only documented, not deployed).
- `cargo test --workspace` passes (42 tests). Two new tests in `pam-visage` cover
  PAM constant correctness and the daemon-not-running error path.
- `RUSTFLAGS="-D unsafe_op_in_unsafe_fn" cargo check` passes clean — the crate is
  Rust 2024 edition-forward-compatible on the unsafe front.

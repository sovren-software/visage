# ADR 007: Ubuntu Packaging & System Integration

**Status:** Accepted
**Date:** 2026-02-22

## Context

Steps 1-5 delivered a working face authentication pipeline: camera capture, ONNX inference,
D-Bus daemon, PAM module, and IR emitter integration. All components compile, 42 tests pass,
and face auth works end-to-end via manual D-Bus calls.

However, installation requires manual steps: building from source, copying files to system
paths, editing PAM configs, creating systemd units, and downloading ONNX models. Step 6
closes the gap between "works for developers" and "works for users."

**Success criteria:** `sudo apt install ./visage_*.deb` on clean Ubuntu 24.04 installs
everything. `sudo echo test` authenticates via face. `sudo apt remove visage` restores
password-only auth cleanly.

## Decisions

### 1. cargo-deb host crate: visaged

**Decision:** Use `visaged` crate's `Cargo.toml` for `[package.metadata.deb]` with `assets`
referencing the CLI binary and PAM .so from sibling crates.

**Rationale:** The daemon is the natural "main" package. cargo-deb's `assets` array handles
multi-binary packages without needing a separate packaging crate.

### 2. Daemon runs as root with systemd hardening

**Decision:** visaged runs as root, protected by `ProtectSystem=strict`, `ProtectHome=true`,
`NoNewPrivileges=true`, `PrivateTmp=true`, and `DeviceAllow=/dev/video* rw`.

**Rationale:** Matches fprintd precedent. Running as a dedicated user would require udev rules
for camera access and group management — complexity deferred to v3.

### 3. Model distribution via `visage setup` CLI command

**Decision:** Models are downloaded on-demand via `sudo visage setup`, not during package
installation (postinst).

**Rationale:** Offline installs should work. Users control when 182 MB downloads happen.
postinst prints a reminder if models are missing.

### 4. PAM logging via libc syslog (LOG_AUTHPRIV)

**Decision:** Use raw `libc::openlog/syslog` with `LOG_AUTHPRIV` facility.

**Rationale:** Standard PAM pattern. No new crate dependencies (libc already in scope).
Messages appear in auth log, not terminal output.

### 5. Daemon logging via tracing (journald captures)

**Decision:** Keep `tracing_subscriber::fmt()` — systemd's `StandardOutput=journal` captures
stdout/stderr automatically.

**Rationale:** Already works. No additional configuration needed.

### 6. D-Bus access control: root-only mutations

**Decision:** Default policy allows only `Verify` and `Status`. `Enroll`, `RemoveModel`, and
`ListModels` are implicitly restricted to root (no `<allow>` in default context).

**Rationale:** Sufficient for v2. In-method UID checks via `GetConnectionCredentials` are
deferred to v3.

### 7. PAM conversation: success feedback only

**Decision:** Send `PAM_TEXT_INFO "Visage: face recognized"` on successful match. Silent on
failure (password prompt speaks for itself).

**Rationale:** Matches fprintd pattern. Avoids confusing error messages when daemon is simply
not configured.

### 8. Client-side D-Bus timeout: 3 seconds

**Decision:** PAM module sets a 3-second `method_timeout` via
`zbus::blocking::connection::Builder::method_timeout()` — applied at connection creation,
not on the proxy.

**Rationale:** Most critical safety feature. Without it, a hung daemon blocks sudo for 25+
seconds (D-Bus default timeout). 3 seconds is enough for normal verification (~80ms) but
short enough that users perceive a quick fallback to password.

**Implementation note:** zbus 5 exposes `method_timeout` on the connection builder, not the
proxy builder. The timeout applies to all method calls on that connection, which is correct
since the PAM module makes exactly one call per authentication attempt.

## Deferred to v3

- Runtime quirk override directory (`/usr/share/visage/quirks/`)
- `visage discover --probe` (test activation pulse)
- `VISAGE_EMITTER_WARM_UP_MS` environment variable
- In-method D-Bus UID validation via `GetConnectionCredentials`
- Dedicated service user with udev rules

## Package Contents

```
/usr/bin/visaged                              — daemon binary
/usr/bin/visage                               — CLI tool
/usr/lib/security/pam_visage.so               — PAM module
/usr/share/dbus-1/system.d/org.freedesktop.Visage1.conf  — D-Bus policy
/usr/lib/systemd/system/visaged.service       — systemd unit
/usr/share/pam-configs/visage                 — pam-auth-update profile
/usr/share/doc/visage/README.md               — documentation
```

## Trade-offs

| Decision | Benefit | Cost |
|----------|---------|------|
| Daemon as root | Avoids udev/group complexity | Root process is a higher-value attack target |
| Models via `visage setup` | Offline-safe install; user controls 182MB download | Extra manual step after install |
| No dedicated service user | Simpler packaging | Weaker isolation than fprintd's dedicated uid |
| root-only D-Bus mutations | Easy to implement | `visage list` requires sudo; inconvenient for scripting |
| 3-second PAM timeout | Login never hangs | Face match has 3s total budget (not just inference time) |
| syslog via libc FFI | No extra crate deps | Raw FFI, no structured logging fields |

## Drawbacks and Known Limitations

1. **No apt-get install from a PPA.** Users must build the `.deb` from source with
   `cargo build --release --workspace && cargo deb -p visaged`. There is no Launchpad PPA
   or published release asset yet.

2. **No integration test of the full install lifecycle.** The `.deb` has been constructed
   and inspected but has not been tested via `sudo apt install` on a clean Ubuntu 24.04 VM.
   The acceptance criteria remain unverified.

3. **Model checksums are for HuggingFace's buffalo_l.** If InsightFace publishes updated
   model weights, the checksums in `setup.rs` will reject them. Users must build from source
   to update.

4. **`MemoryDenyWriteExecute=false` allows W+X pages.** Required for ONNX Runtime's JIT
   execution provider. This is a meaningful systemd hardening regression; the alternative
   would be to use the ORT CPU provider exclusively and disable JIT, which may be slower
   but would allow restoring this restriction.

5. **Daemon restarts require face re-enrollment if database is purged.** `apt purge`
   removes `/var/lib/visage/` including all enrolled models. Users must re-enroll after
   purge.

6. **PAM conversation message requires terminal display.** The "Visage: face recognized"
   feedback only appears if the PAM application (sudo, gdm, etc.) calls the conversation
   function. SSH sessions and headless contexts may not display it.

7. **No Debian changelog or proper package versioning.** The `.deb` uses workspace version
   0.1.0 directly. A `debian/changelog` file with proper version history is absent — needed
   for PPA submission.

## Remaining Work to Fully Complete

The following must be done before v0.1 can be publicly announced:

1. **End-to-end install test on Ubuntu 24.04 VM** — verify the full `apt install → visage setup → enroll → sudo` flow
2. **GitHub release asset** — build `.deb` in CI and attach to the v0.1 tag
3. **PPA or release binary** — users cannot currently install without building from source
4. **Launchpad PPA** (or GitHub Actions `.deb` build) — prerequisite for Ubuntu distribution strategy
5. **`systemd-tmpfiles.d` entry** — idiomatic alternative to `postinst mkdir` for `/var/lib/visage`
6. **Debian changelog** — required for PPA; track version changes
7. **`preinst` guard** — check if `pam-auth-update` is available before running it
8. **NixOS package** — AEGIS overlay integration per distribution-strategy.md Tier 1

## Consequences

- Users with build tooling can install with a single `apt install ./visage_*.deb`
- PAM configuration is automatic via `pam-auth-update`
- Clean removal restores password-only auth
- Model download is explicit and offline-safe
- Daemon hardening limits blast radius of potential vulnerabilities

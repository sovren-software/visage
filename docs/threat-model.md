# Visage Threat Model

## Scope

Visage provides **convenience authentication** — it reduces friction for common operations
(sudo, screen unlock) but does not replace password/FIDO2 as the root credential.

## Implementation Status

The threat model is organized by implementation tier. Items marked **(v2 — implemented)**
are active in the current codebase. Items marked **(roadmap)** are not yet present.

## Threat Tiers

### Tier 0 — Baseline

| Threat | Mitigation | Status |
|--------|------------|--------|
| Brute force (repeated attempts) | Rate limiting + lockout after N failures | ⬜ Roadmap — not implemented in v2 |
| Stolen photo (printed) | Multi-frame confirmation + IR-only pipeline | ✅ v2 — IR camera required; RGB webcam not accepted as input |
| Replay attack (recorded video) | IR strobe pattern detection (odd/even frame analysis) | ⬜ Roadmap — IR emitter is on but no strobe challenge |
| Unauthorized enrollment | Root-only enrollment via D-Bus policy | ✅ v2 — D-Bus policy restricts Enroll to root |
| Timing side channel | Constant-time embedding comparison | ✅ v2 — `CosineMatcher` always processes all gallery entries |
| Login hang (daemon crash) | 3-second PAM call timeout | ✅ v2 (Step 6) — `method_timeout(3s)` via zbus connection builder |
| Auth failure leaks user info | syslog at LOG_AUTHPRIV | ✅ v2 (Step 6) — goes to `/var/log/auth.log`, not terminal |

### Tier 1 — Liveness

| Threat | Mitigation | Status |
|--------|------------|--------|
| Static photo/mask in IR | Active challenge: random blink/turn request | ⬜ Roadmap |
| Screen replay | Motion parallax detection across frames | ⬜ Roadmap |

### Tier 2 — Advanced (roadmap)

| Threat | Mitigation | Status |
|--------|------------|--------|
| 3D mask | Depth sensing (hardware dependent) | ⬜ v3 |
| Deepfake video feed | Structured light verification | ⬜ v3 |

## Out of Scope

- Nation-state adversary with custom silicone mask
- Physical coercion (user forced to look at camera)
- Compromised kernel/root (game over regardless)

## Step 6: Security Controls Added

The packaging step added systemic hardening beyond the core auth logic:

### systemd Sandbox

`visaged.service` applies:

| Directive | Effect |
|-----------|--------|
| `ProtectSystem=strict` | Filesystem read-only except `/var/lib/visage` and runtime paths |
| `ProtectHome=true` | No access to any user home directory |
| `NoNewPrivileges=true` | Process and children cannot gain privileges via setuid/setcap |
| `PrivateTmp=true` | Isolated `/tmp` — prevents `/tmp` race attacks |
| `CapabilityBoundingSet=` (empty) | All Linux capabilities dropped — root with no capabilities |
| `DeviceAllow=/dev/video* rw` | Camera access is the only device permission |
| `MemoryDenyWriteExecute=false` | Intentionally disabled — ONNX Runtime requires W+X for JIT |

The `MemoryDenyWriteExecute=false` exception is the most significant hardening gap. It allows
the daemon to map writable+executable memory pages, which ONNX Runtime requires for its CPU
execution provider JIT compilation. Mitigations: the daemon has no network access, no inbound
connections, and is further sandboxed by all other directives.

### D-Bus Policy

`org.freedesktop.Visage1.conf` restricts the attack surface:

- **Verify, Status** — available to all local users (PAM module and CLI need these)
- **Enroll, RemoveModel, ListModels** — no `<allow>` in default context → blocked

This means a non-root user who gains code execution cannot enroll a fake face. They can call
`Verify` (which only reads, never writes) but cannot modify the face model store.

**Known gap:** The `user` parameter in `Verify` is caller-supplied. A compromised process
could call `Verify("other-user")` to probe whether that user's face matches. In-method UID
validation via `GetConnectionCredentials` is deferred to v3.

### PAM Module Security Properties

| Property | Implementation |
|----------|---------------|
| Never locks user out | All error paths return `PAM_IGNORE` (falls through to password) |
| No panic across FFI | `std::panic::catch_unwind` wraps all Rust logic |
| Login hang prevention | 3-second D-Bus connection timeout |
| Auth log only | `openlog(LOG_AUTHPRIV)` — messages go to `/var/log/auth.log` |
| No terminal leakage | `syslog(3)` replaces `eprintln!` in production build |
| Format string safety | syslog called as `syslog(priority, "%s", msg)` — no format injection |

## Audit Events

Authentication attempts are logged to `/var/log/auth.log` via `LOG_AUTHPRIV`:

```
pam_visage: face matched for user 'ccross'
pam_visage: no match for user 'ccross'
pam_visage: D-Bus error: ServiceUnknown (daemon not running)
pam_visage: pam_get_user failed (ret=4)
```

**Not yet logged:** match confidence score, camera device used, IR emitter status. These
require structured journal fields (sd_journal_send) rather than plain syslog — deferred to v3.

## Known Security Gaps (v2)

1. **No rate limiting.** Repeated failed face attempts are not counted or throttled. An
   attacker with physical access can make unlimited verification attempts.

2. **No active liveness detection.** A high-quality photograph in the IR band could
   potentially pass verification. The IR emitter increases the difficulty but does not
   eliminate the threat.

3. **No D-Bus caller authentication.** `Verify(user)` trusts the caller-supplied username.

4. **Root daemon with W+X pages.** `MemoryDenyWriteExecute=false` weakens sandbox.

5. **Model storage not encrypted.** Face embeddings in `/var/lib/visage/faces.db` are
   stored as raw float32 blobs, not encrypted at rest. See ADR 003, §5.

# Visage Threat Model

## Scope

Visage provides **convenience authentication** — it reduces friction for common operations
(sudo, screen unlock) but does not replace password/FIDO2 as the root credential.

## Implementation Status

The threat model is organized by implementation tier. Items marked **(v0.3 — implemented)**
are active in the current codebase. Items marked **(roadmap)** are not yet present.

## Threat Tiers

### Tier 0 — Baseline

| Threat | Mitigation | Status |
|--------|------------|--------|
| Brute force (repeated attempts) | Rate limiting + lockout after N failures | ✅ v0.3 — implemented |
| Stolen photo (printed) | Passive liveness (landmark stability) + IR emitter support | ✅ v0.3 — landmark stability rejects static images; IR recommended. ⚠️ Verdict predates the 2026-08-17 hardware validation and is **not** revised on it; see the note below. |
| Model tampering / substitution | Strict SHA-256 verification on download + daemon startup | ✅ v0.3 — implemented |
| Replay attack (recorded video) | IR strobe pattern detection (odd/even frame analysis) | ⛔ **Measured and rejected 2026-09-16** on `3277:0055` — a phone screen swings 1.71× MORE than a live face (specular glass vs diffuse skin), and the value is attacker-controllable by tilt. See the [hardware report](hardware-reports/asus-zenbook-um3406ha-3277-0055.md). |
| Unauthorized enrollment | Root-only enrollment via D-Bus policy | ✅ v0.3 — D-Bus policy restricts Enroll to root |
| Timing side channel | Constant-time embedding comparison | ✅ v0.3 — `CosineMatcher` always processes all gallery entries |
| Login hang (daemon crash) | 3-second PAM call timeout | ✅ v0.3 (Step 6) — `method_timeout(3s)` via zbus connection builder |
| Auth failure leaks user info | syslog at LOG_AUTHPRIV | ✅ v0.3 (Step 6) — goes to `/var/log/auth.log`, not terminal |

### Tier 1 — Liveness

| Threat | Mitigation | Status |
|--------|------------|--------|
| Static photo (printed or displayed) | Passive landmark stability: eye landmarks must shift between frames | ✅ v0.3 — `check_landmark_stability` in `visage-core`. ⚠️ See the note below. |
| Static photo/mask in IR | Active challenge: random blink/turn request | ⬜ Roadmap |
| Screen replay (video) | Motion parallax detection across frames | ⬜ Roadmap |

> ⚠️ **These two ✅ verdicts have not been re-established against hardware, and one measurement contradicts them.**
>
> On 2026-08-17 the first hardware validation of passive liveness, on an ASUS Zenbook 14
> UM3406HA with a Shinetech `3277:0055` IR module, found the metric **did not discriminate**:
> a hand-held phone-screen spoof displaced **0.681 px**, *higher* than two genuine live
> attempts (0.263, 0.670), so no threshold admits the live minimum while rejecting that
> spoof — and the identity stage matched the same photo at **0.9013**.
>
> The verdicts above are deliberately **not** revised on that result, because the evidence
> bar for changing a published threat-model claim has not been met: n=1 on the spoof side,
> one sample of unestablished provenance, and printed paper untested. Revising requires the
> full matrix in [`liveness-remaining-work.md`](liveness-remaining-work.md).
>
> They are annotated instead, so a ✅ here cannot be read as an unqualified claim that a
> displayed photo is rejected. On the hardware we have measured, it is not. What the metric
> still catches is a **rigidly mounted** photo (`liveness.rs` documents that case as
> "<0.3 px, sensor noise only"). See the [hardware report](hardware-reports/asus-zenbook-um3406ha-3277-0055.md)
> and [`STATUS.md`](STATUS.md).

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
| `DeviceAllow=char-video4linux rw` | Camera access is the only device permission |
| `PrivateNetwork=true` | Own network namespace, loopback only — the daemon cannot reach the network |
| `MemoryDenyWriteExecute=false` | Intentionally disabled — ONNX Runtime requires W+X for JIT |

The `MemoryDenyWriteExecute=false` exception is the most significant hardening gap. It allows
the daemon to map writable+executable memory pages, which ONNX Runtime requires for its CPU
execution provider JIT compilation. Mitigations: the daemon has no network access — enforced by
`PrivateNetwork=true`, which places it in its own network namespace with loopback only — and it
is further sandboxed by all other directives.

Until v0.4 that network claim was an assertion with nothing behind it: no unit directive
enforced isolation, so the compensating control this exception is justified by did not exist.
Reported as issue #78 and now enforced. The daemon makes no network calls of its own; the only
HTTP client in the workspace is `ureq`, used solely by `visage-cli` for `visage setup` model
downloads, which runs as a separate process and is unaffected.

### D-Bus Policy

`org.freedesktop.Visage1.conf` restricts the attack surface:

- **Verify, Status** — available to all local users (PAM module and CLI need these)
- **Enroll, RemoveModel, ListModels** — no `<allow>` in default context → blocked

This means a non-root user who gains code execution cannot enroll a fake face. They can call
`Verify` (which only reads, never writes) but cannot modify the face model store.

**Known gap:** In-method UID validation uses D-Bus UNIX UID lookup and a username→UID resolution.
This works for local users and NSS-backed identities (LDAP/SSSD/AD), but it does not yet use
`GetConnectionCredentials`.

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

## Known Security Gaps (v0.3)

1. **No active liveness detection.** Passive landmark stability blocks printed photos and
   static images, but a video replay attack (pre-recorded footage of the user) will pass
   the liveness check because landmarks move naturally in video. Active challenges (blink
   request, head turn) are required to address this — deferred to v0.4.

2. **UID validation is not based on `GetConnectionCredentials`.** The daemon validates the
   caller UNIX UID against the target username, but it does not yet use
   `GetConnectionCredentials`.

3. **Root daemon with W+X pages.** `MemoryDenyWriteExecute=false` weakens sandbox. The
   compensating control is `PrivateNetwork=true` (added in v0.4; see issue #78), which denies
   the daemon any route off the host.

4. **Passive liveness threshold is tunable.** `VISAGE_LIVENESS_MIN_DISPLACEMENT` defaults
   to 0.8 px. Cameras with very low frame rates or high sensor noise may require adjustment.
   Setting `VISAGE_LIVENESS_ENABLED=0` disables the check entirely — this is intentional
   for development but should not be used in production.

5. **The GPU kernel cache is outside model integrity.** With the optional `openvino` build
   and `VISAGE_INFERENCE_DEVICE=gpu`, OpenVINO compiles the SHA-256-verified ONNX files
   into kernels and caches them in `VISAGE_INFERENCE_CACHE_DIR`, then loads the cache on
   later starts without checking it. Anyone who can write that directory can change what
   runs. The default (`/var/lib/visage/gpu-cache`) is root-only, so this needs root
   already; a custom cache directory must keep that property. The CPU path is unaffected.

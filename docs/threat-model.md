# Visage Threat Model

## Scope

Visage provides **convenience authentication** — it reduces friction for common operations
(sudo, screen unlock) but does not replace password/FIDO2 as the root credential.

## Threat Tiers

### Tier 0 — Baseline (v1.0)

| Threat | Mitigation |
|--------|------------|
| Brute force (repeated attempts) | Rate limiting + lockout after N failures |
| Stolen photo (printed) | Multi-frame confirmation + IR-only pipeline |
| Replay attack (recorded video) | IR strobe pattern detection (odd/even frame analysis) |
| Unauthorized enrollment | Root-only enrollment via D-Bus policy |
| Timing side channel | Constant-time embedding comparison |

### Tier 1 — Liveness (v1.0)

| Threat | Mitigation |
|--------|------------|
| Static photo/mask in IR | Active challenge: random blink/turn request |
| Screen replay | Motion parallax detection across frames |

### Tier 2 — Advanced (roadmap)

| Threat | Mitigation |
|--------|------------|
| 3D mask | Depth sensing (hardware dependent) |
| Deepfake video feed | Structured light verification |

## Out of Scope

- Nation-state adversary with custom silicone mask
- Physical coercion (user forced to look at camera)
- Compromised kernel/root (game over regardless)

## Audit Events

All auth attempts (success/failure) logged to systemd journal with:
- Timestamp, user, PAM service name
- Match confidence score
- Camera device used
- Liveness challenge result (if applicable)

# Visage Architecture

## Design Principles

1. **Daemon owns hardware** — PAM module never touches the camera
2. **D-Bus for IPC** — Standard Linux desktop integration pattern (fprintd model)
3. **IR emitter absorbed** — No external dependency for emitter control
4. **Pluggable models** — ONNX Runtime for inference, swap models without recompilation
5. **Distribution-agnostic** — Ubuntu first, NixOS second, then Arch/Fedora

## Component Overview

```
┌───────────┐    D-Bus     ┌──────────┐    V4L2    ┌──────────┐
│ pam_visage│───────────▶ │ visaged  │──────────▶│ IR Camera│
│ (cdylib)  │             │ (daemon) │           └──────────┘
└───────────┘             └────┬─────┘
                               │
┌───────────┐    D-Bus    ┌────▼─────┐    ONNX    ┌──────────┐
│ visage    │───────────▶│ visage-  │──────────▶│ SCRFD    │
│ (CLI)     │            │ core     │           │ ArcFace  │
└───────────┘            └──────────┘           └──────────┘
```

## Authentication Flow

1. PAM stack triggers `pam_visage.so`
2. PAM module connects to `org.freedesktop.Visage1` D-Bus service
3. Calls `Verify(username)` with a timeout
4. Daemon activates IR emitter (if needed)
5. Captures N frames, skipping dark frames
6. SCRFD detects face bounding boxes
7. ArcFace extracts embedding from best detection
8. Compares embedding against enrolled models (cosine similarity)
9. Returns match/no-match to PAM module
10. PAM module returns PAM_SUCCESS or PAM_IGNORE (safe fallback)

## Security Model

See [threat-model.md](threat-model.md).

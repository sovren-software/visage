# ADR 009 — ONNX Model Integrity Verification

**Date:** 2026-02-23
**Status:** Accepted
**Deciders:** Sovren Software

---

## Context

Visage loads two ONNX model files at daemon startup:

| Model | File | Size | Purpose |
|-------|------|------|---------|
| SCRFD det_10g | `det_10g.onnx` | 16 MB | Face detection |
| ArcFace w600k_r50 | `w600k_r50.onnx` | 166 MB | Face recognition |

These files are downloaded by `visage setup` from HuggingFace and stored in
`/var/lib/visage/models/`. They are not bundled in the `.deb` package — the
package is ~1 MB; the models are ~182 MB.

Prior to v0.3.0, the integrity situation was:

1. **Download-time:** `visage setup` verified SHA-256 checksums on download.
   This was implemented in `visage-cli/src/setup.rs` with the model list and
   hashing logic duplicated inline.

2. **Startup-time:** `visaged` performed **no verification**. It passed the model
   paths directly to ONNX Runtime and relied on ONNX Runtime to reject malformed
   files. ONNX Runtime does not verify checksums — it parses whatever bytes it
   receives.

3. **Single source of truth:** The model list (URLs, filenames, SHA-256 checksums)
   existed only in `visage-cli/src/setup.rs`. The daemon had no knowledge of
   expected checksums.

This created a gap: a model file could be replaced or corrupted between download
and daemon startup, and the daemon would load it without complaint. If the
replacement was a valid ONNX file with different weights, the daemon would start
successfully but produce incorrect authentication results — potentially accepting
faces it should reject, or rejecting all faces.

The threat is not primarily remote: the daemon runs as root with
`ProtectSystem=strict`, and `/var/lib/visage/models/` is not writable by
non-root users. The realistic attack vectors are:

- A compromised package or update script that replaces model files
- A local root-level attacker who has already compromised the system
- Accidental corruption (filesystem error, interrupted download, disk failure)
- A developer or administrator who manually replaced a model file with an
  incompatible version

The last two are the most common in practice.

---

## Decision

### 1. Extract model manifest into a shared `visage-models` crate

Create `crates/visage-models` as a new library crate containing:

- `ModelFile` struct — name, URL, SHA-256, human-readable size
- `MODELS` constant — the authoritative list of required model files
- `ModelIntegrityError` enum — typed errors for missing, unreadable, and
  checksum-mismatched files
- `sha256_file_hex(path)` — streaming SHA-256 computation
- `verify_file_sha256(name, path, expected)` — single-file verification
- `verify_models_dir(dir)` — verify all required models in a directory

Both `visage-cli` and `visaged` depend on this crate. The model list and
checksums exist in exactly one place.

### 2. Verify model integrity at daemon startup (fail closed)

`visaged` calls `visage_models::verify_models_dir(&config.model_dir)` as the
first action after loading configuration, before opening the camera or loading
ONNX Runtime. If verification fails, the daemon exits immediately with a
structured error message:

```
Error: model integrity verification failed for /var/lib/visage/models;
       run `sudo visage setup` to download verified ONNX models

Caused by:
    model file not found: det_10g.onnx (/var/lib/visage/models/det_10g.onnx)
```

or, for a checksum mismatch:

```
Error: model integrity verification failed for /var/lib/visage/models;
       run `sudo visage setup` to download verified ONNX models

Caused by:
    model checksum mismatch for w600k_r50.onnx (...)
      expected: 4c06341c...
      got:      deadbeef...
```

The daemon **never starts** with unverified models. This is the fail-closed
principle: a security control that fails safe rather than failing open.

### 3. Retain download-time verification in `visage setup`

`visage setup` continues to verify checksums immediately after download, before
the atomic rename. This catches network corruption or truncated downloads before
the file reaches its final path. The implementation is refactored to use
`visage-models::verify_file_sha256` rather than duplicating the logic.

### 4. Pin checksums to HuggingFace Git LFS object IDs

The SHA-256 values are sourced from HuggingFace Git LFS pointer files
(`oid sha256:` field). These are content-addressed and stable for a given model
version. They are not derived by downloading and hashing — they are the upstream
content hash, verified against the LFS pointer before being committed to this
repository.

```
det_10g.onnx:    5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91
w600k_r50.onnx:  4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43
```

---

## Alternatives considered

### Alternative A: Verify only at download time (status quo)

Keep the existing download-time check and rely on ONNX Runtime to reject
malformed files at load time.

**Rejected:** ONNX Runtime validates file structure, not content. A valid ONNX
file with different weights loads silently. The gap between download and first
use (which may be seconds or months) is undetected. This leaves a window where
model substitution produces incorrect authentication results without any error.

### Alternative B: Verify at load time inside `visage-core`

Move the integrity check into the ONNX model loading code in `visage-core`,
so verification happens immediately before the model is passed to ONNX Runtime.

**Rejected:** `visage-core` is a pure inference library. It has no knowledge of
which specific model files are "correct" for a given release — that is release
metadata, not inference logic. Embedding checksums in `visage-core` would couple
the inference library to a specific release, making it harder to swap models for
testing or development. The daemon startup is the right place for this check.

### Alternative C: Cryptographic signing (GPG/minisign)

Sign model files with a Sovren Software private key. Verify the signature at
startup using the embedded public key.

**Considered but deferred:** Signing provides stronger guarantees than
checksums — it proves the file came from Sovren Software, not just that the
bytes match a known value. However:

- The models are third-party (InsightFace / buffalo_l). Sovren Software does not
  own the model weights. Signing them with a Sovren key would imply endorsement
  of the content in a way that may create confusion.
- The primary threat is accidental corruption and local substitution, not a
  supply-chain attack on the model files specifically. SHA-256 is sufficient for
  these cases.
- Signing infrastructure (key management, revocation, signature distribution)
  adds operational complexity that is not justified at this stage.

Signing is listed as a future improvement for v3 when the threat model warrants it.

### Alternative D: Bundle models in the `.deb` package

Include the ONNX files in the package, eliminating the separate download step.

**Rejected:** The models total ~182 MB. A `.deb` of that size is impractical for
distribution (GitHub Release assets, Launchpad PPA, mirrors). The separate
download step is intentional and documented. Bundling would also make it harder
to update models independently of the binary.

---

## Consequences

### Positive

- **Fail-closed startup:** The daemon cannot start with unverified models. Any
  corruption, substitution, or version mismatch is caught before authentication
  is possible.
- **Single source of truth:** Model metadata (URLs, filenames, checksums) lives
  in one crate. Adding a new model or updating a checksum requires one change
  in one file; both the CLI and daemon pick it up automatically.
- **Actionable errors:** Error messages include the recovery command
  (`sudo visage setup`). Operators do not need to diagnose the failure manually.
- **Regression protection:** Four unit tests in `visage-models` cover the
  missing-file, checksum-mismatch, checksum-match, and missing-directory cases.
  A future change that breaks verification will fail the test suite.
- **Audit trail:** The expected checksums are committed to the repository. Any
  change to the pinned model versions is visible in git history.

### Negative / Trade-offs

- **Startup latency:** Hashing two files (16 MB + 166 MB) adds ~50–150ms to
  daemon startup time on typical hardware (SSD, CPU-only). This is a one-time
  cost at service start, not per-authentication. The daemon is a persistent
  service; startup latency is not on the authentication hot path.
- **Checksum updates require a release:** When InsightFace publishes a new model
  version, updating to it requires updating the checksums in `visage-models` and
  cutting a new Visage release. There is no mechanism for out-of-band model
  updates. This is intentional — silent model updates are a supply-chain risk.
- **No signature verification:** SHA-256 checksums verify integrity (the bytes
  match a known value) but not authenticity (the file came from a trusted
  source). A compromised system that can replace both the model file and the
  Visage binary can also replace the expected checksum. This is an accepted
  limitation at this stage.
- **`visage setup` must be re-run after purge:** `apt purge` removes
  `/var/lib/visage/models/`. After reinstalling, models must be re-downloaded.
  The daemon will refuse to start until `sudo visage setup` is run. This is
  correct behavior but requires documentation.

### Known limitations that remain open

| Limitation | Impact | Mitigation | Future work |
|------------|--------|------------|-------------|
| No cryptographic signing | Cannot prove models came from Sovren Software | SHA-256 catches corruption and substitution | v3: minisign or sigstore |
| Checksum updates require a release | Cannot update models without a new Visage version | Intentional — prevents silent model swaps | Evaluate per-model versioning in v3 |
| Startup hash cost (~50–150ms) | Slightly slower daemon startup | One-time cost; not on auth hot path | Acceptable trade-off |
| No streaming verification during ONNX load | Model bytes are hashed before ONNX Runtime sees them, not during | Two-pass read (hash then load) | Acceptable; files are small relative to RAM |

---

## Implementation

**Commit:** `5d001c2` — release: v0.3.0
**Files changed:**
- `crates/visage-models/` — new crate (manifest, verification helpers, 4 tests)
- `crates/visage-models/Cargo.toml` — package definition
- `crates/visage-models/src/lib.rs` — `ModelFile`, `MODELS`, `ModelIntegrityError`,
  `sha256_file_hex`, `verify_file_sha256`, `verify_models_dir`
- `crates/visage-cli/Cargo.toml` — added `visage-models` dep, removed direct `sha2`
- `crates/visage-cli/src/setup.rs` — refactored to use shared manifest and helpers
- `crates/visaged/Cargo.toml` — added `visage-models` dep
- `crates/visaged/src/main.rs` — `verify_models_dir` call at startup (fail-closed)
- `Cargo.toml` — added `visage-models` to workspace members, `sha2` to workspace deps

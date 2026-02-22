# ADR 003: Daemon Integration Architecture

**Status:** Accepted
**Date:** 2026-02-22
**Step:** 3 — visaged daemon, SQLite storage, D-Bus API

---

## Context

Steps 1 and 2 produced production-quality hardware abstraction (`visage-hw`) and
inference engine (`visage-core`). Step 3 wires them together into a functional daemon
(`visaged`) with persistent storage, an IPC interface, and a command-line client
(`visage-cli`).

The key constraints shaping the architecture:

- `Camera`, `FaceDetector`, and `FaceRecognizer` all take `&mut self` on their
  primary methods and are not `Sync`. They cannot be shared across async tasks.
- D-Bus method handlers are async and run on the tokio runtime.
- SQLite operations must not block the async executor.
- Cross-user data access must be prevented at the storage layer, not just the API layer.

---

## Decisions

### 1. Dedicated OS thread for inference

**Decision:** Camera, detector, and recognizer live on a single `std::thread::spawn`
thread. D-Bus handlers communicate via `mpsc::channel` + `oneshot` reply channels.

**Rationale:** The inference objects are `!Sync` and take `&mut self`. Running them
on a dedicated OS thread avoids `Arc<Mutex<_>>` contention on hot paths and matches
the objects' natural ownership model. The channel depth of 4 prevents unbounded
request queueing while allowing bursting.

**Alternative considered:** `tokio::task::spawn_blocking` per request. Rejected because
it would require the camera and models to be `Send + 'static` (achievable via `Arc<Mutex>`)
but would create per-request overhead of acquiring the mutex and potentially allocating
new sessions. The dedicated thread is simpler and avoids all of this.

### 2. Session bus for development; system bus migration in Step 4

**Decision (Step 3):** `visaged` registers on the session bus (`zbus::Connection::session()`).
No D-Bus policy file is required for session bus operation.

**Rationale:** Session bus access requires no policy configuration, eliminating a
packaging dependency during development. PAM integration requires the system bus because
PAM modules execute as root in a separate session context.

**Step 4 resolution:** `visaged` and `visage-cli` now default to the system bus.
`VISAGE_SESSION_BUS=1` provides a development fallback. The daemon logs which bus is
active at startup. See [ADR 005](005-pam-system-bus-migration.md).

### 3. SQLite with WAL mode via tokio-rusqlite

**Decision:** `tokio-rusqlite` wraps SQLite on a blocking thread internally. WAL
journal mode is set at open time. Embeddings are stored as raw little-endian f32 bytes
(512 × 4 = 2048 bytes per embedding).

**Rationale:** SQLite is sufficient for single-machine face enrollment (expected
O(10–100) rows). WAL mode enables concurrent reads without blocking writes. Raw f32
bytes are more compact than JSON (~4× smaller) and eliminate a deserialization step on
gallery fetch.

**v3 data plane:** Two extra columns (`quality_score REAL`, `pose_label TEXT`) are
included in the schema now with defaults. This avoids a breaking schema migration when
pose-indexed enrollment is added.

### 4. Per-user scoping at the storage layer

**Decision:** All store mutations include a `WHERE user = ?` clause. `remove()` takes
both `user` and `model_id` and returns `false` if the model belongs to a different user.

**Rationale:** Defense in depth. The D-Bus interface passes the caller-supplied `user`
string to the store, so a compromised or misbehaving caller cannot delete another
user's enrollment by guessing an ID. This is not a substitute for proper D-Bus sender
authentication (future work), but it limits blast radius.

### 5. Locking protocol for AppState

**Decision:** `AppState` is behind `Arc<Mutex<_>>`. The protocol for every D-Bus handler is:
1. Lock → copy config values and clone `EngineHandle` → unlock
2. Call engine (no lock held; this is blocking I/O via channel)
3. Lock → write to store → unlock

**Rationale:** Holding the mutex across an engine call would serialize all D-Bus
requests behind a single blocking operation (camera capture + inference). By releasing
the lock before the engine call, concurrent status/list calls can proceed. The tradeoff
is a second lock acquisition for the store write, which is negligible.

### 6. Fail-fast at startup

**Decision:** `spawn_engine()` opens the camera and loads both ONNX models
synchronously before returning. Warmup frames are discarded inline.

**Rationale:** A daemon that starts successfully but fails on the first request is
worse than one that refuses to start. Fail-fast makes the error surface at boot time
where it's visible in the journal, not at enrollment/verification time where a user
is waiting.

---

## Consequences

- Smoke testing requires a physical IR camera and downloaded ONNX model files. Unit
  tests (store roundtrip, cross-user protection, embedding fidelity) run without hardware.
- ~~Step 4 (PAM module) will need to switch to the system bus.~~ **Resolved in Step 4** —
  daemon now defaults to system bus; policy file deployed to `/usr/share/dbus-1/system.d/`.
- The `best_quality` field on `VerifyResult` is currently unused by the D-Bus handler.
  Preserved as a v3 hook for surfacing quality metadata to callers without a schema change.
- No authentication on D-Bus callers in v2. The `user` parameter is caller-supplied
  and not validated against the D-Bus sender identity. Deferred to Step 6.

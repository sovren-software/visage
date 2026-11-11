# Changelog

## Unreleased

### Security

- **`/var/lib/visage` is now root-only on the AUR and RPM paths.** `faces.db` keeps
  usernames, labels and enrollment times in clear (embeddings stay AES-GCM encrypted under
  a 0600 key), so a world-readable state directory told every local user who enrolled what.
  Debian and NixOS were already 0700. The AUR package ran `install -dm700 .../visage/models`,
  which makes only `models` 0700 and creates `/var/lib/visage` 0755; the RPM never created
  the directory at all. Both now create it 0700 and repair an existing one on upgrade or
  install. The shared unit also sets `StateDirectory=visage` with `StateDirectoryMode=0700`,
  and `visaged` and `visage setup` create any missing directory 0700, for source installs.
  `state_dir_mode_contract.rs` reads all four packaging formats so they cannot drift again.

  **Existing AUR and RPM installs:** `sudo chmod 700 /var/lib/visage`. The package upgrade
  does this, but an install from a source build does not.

- **Seven RustSec advisories in the TLS stack, cleared by lockfile updates.**
  `rustls` 0.23.45 → 0.23.46 (RUSTSEC-2026-0346, -0347, -0348, -0349),
  `rustls-webpki` 0.103.15 → 0.103.16 (RUSTSEC-2026-0382), `webpki-roots` and
  `webpki-root-certs` 1.0.6 → 1.0.11 (RUSTSEC-2026-0381, -0383). The stack reaches the tree
  only through `ureq`, which `visage-cli` uses to download models and `ort`/`ort-sys` use
  at build time. The daemon has no network access, so none of these were reachable from
  `visaged`. The scheduled Security Audit had failed every week since 2026-10-19.

- **RUSTSEC-2026-0459 — bundled SQLite 3.54.1 via `rusqlite` 0.32 → 0.41.**
  `libsqlite3-sys` 0.30.1 bundled a SQLite that can return uninitialized page-cache memory
  when it opens a database together with a crafted `-wal` file. Exposure here was low:
  `faces.db` lives in a root-only directory, and the attack needs a less trusted party to
  write both files. The fix ships only with `rusqlite` 0.41.

  `tokio-rusqlite` 0.8 pins `rusqlite` ^0.40, so it is **removed**. A 60-line handle in
  `visaged/src/db.rs` takes its place and keeps its three properties: calls are serialised,
  the async executor never blocks, and a panic fails closed for every later call. Tests
  cover each property. `count_all` now reads `COUNT(*)` as `i64`, because `rusqlite` 0.38
  made `u64: FromSql` opt-in.

### Changed

- **rand 0.8 → 0.9; an OS random-source failure is now an error, not a panic.** rand 0.9
  makes `OsRng` fallible (`TryRngCore`), so the encryption key and every AES-GCM nonce
  come from `try_fill_bytes`, and a failure returns `StoreError::Random` before any key or
  nonce is used. The lockfile drops `rand 0.8.8` and `rand_chacha 0.3.1`. Two new tests:
  16 encryptions get 16 distinct, non-zero nonces, and a generated key is random, 0600,
  and read back unchanged. Supersedes dependabot #146, which did not compile.

- **MSRV 1.88 → 1.89.** `rusqlite` 0.41.0 and 0.41.1 both declare 1.89 in their manifests,
  although the 0.41.0 release note says it lowered the MSRV to 1.88. The `msrv` CI job
  checks the new value.

- **`pam_visage` retries until its `timeout=N` budget runs out** (#127, thanks
  @ProjectInitiative). `timeout=N` used to bound one `Verify` call; it is now the total
  budget for one attempt. Transient failures (no face in frame, dark frames, the daemon
  restarting after resume, #123) are retried every 200 ms, and a face that was seen but did
  not match gets one more try after 1.5 s. Visaged counts only real non-matches toward its
  lockout, so the fast retries cost nothing there. ⚠️ A stopped daemon now costs the full
  `timeout=N` on every authentication instead of failing at once.

  Errors no retry can fix end the attempt at once: an active lockout, a missing
  enrollment, an unknown user, or a refused caller (`AccessDenied`). They live in one
  table, `PERMANENT_ERRORS`, and `pam_retry_contract.rs` fails CI if visaged stops
  producing any of them, so a reworded message cannot turn a lockout into a retry loop.

- **A verify examines frames as they arrive, on one camera stream.** It used to capture
  a whole burst before examining any of it. It now streams frames and examines each
  one as it arrives, so it can stop at the frame that settles it: with the change
  below, a match with liveness off ends the verify on its first matching frame. The
  verify reads at most what a burst did (`frames_per_verify` usable frames, or three
  times that many attempts when frames come out dark), and its outcomes are the
  burst's: no face, no usable frames (still counted toward the camera self-heal),
  matched, not matched, and the liveness gate over every frame. On a ThinkPad X12
  Detachable Gen 1 (Syntek `174f:118f`), capture for a verify in view went from
  ~363 to ~296 ms. `Camera::stream_frames` is the new entry point; the capture loop's
  observer may now end it early, and the counted and timed entry points are unchanged.

- **A successful verify no longer runs ArcFace on frames after the first match.** The
  outcome is a match if any frame matches, so once one has, the remaining frames'
  extractions can only change the reported similarity, never the decision. That was
  two of three extractions per successful verify at the default
  `frames_per_verify=3`, the waste `extract_n` in the timing line was added to expose.
  With liveness on, the remaining frames are still run through detection, because the
  landmark check needs every frame and fails closed without them; with liveness off
  they are skipped entirely.

  The reported similarity is now that of the first matching frame rather than the
  best one. Failed verifies are unchanged: every frame is still examined.

### Added

- **`VISAGE_FACE_WAIT_MS`: keep looking for a face on the same stream.** A verify often
  starts before the user is in front of the camera. `pam_visage` retries it until its
  `timeout=N` budget runs out (#127), but every retry starts the camera stream again.
  With a window set, a verify that has seen no face keeps reading the stream, one group
  of `frames_per_verify` frames after another, until a face appears or the window ends.
  A face that is seen ends the verify as before, matched or not, so a request never
  becomes several attempts against the gallery; its group is counted from the face's
  frame, so the liveness check gets `frames_per_verify` frames even when the face first
  appears late in a group. "No usable frames" is still reported only when every frame of
  the verify was dark. `bursts` in the timing line
  counts the groups. Default `0` (one group, as before); `services.visage.faceWaitMs` on
  NixOS. On a ThinkPad X12 Detachable Gen 1 (GPU inference), waiting for a face went
  from a new verify every ~618 ms (3 frames each, ~4.9 frames/s examined) to one verify
  examining ~26.5 frames/s, so a face should be noticed within about one frame instead
  of up to ~0.55 s later.

- **A verify that sees only dark frames now logs why it might have.** One warning lists
  each rejected frame's sequence number and its mean and 99th-percentile brightness. On one
  ASUS UM3406HA (`3277:0055`), stretches of all-dark verifies have lasted from seconds to
  four days, survived camera reopens and cleared only at reboot, cause unknown. Values at
  the sensor's black level on every frame point at something over the lens; a p99 well
  above it means the sensor sees a scene the emitter did not light.

- **Optional GPU inference through OpenVINO (`openvino` cargo feature,
  `VISAGE_INFERENCE_DEVICE=gpu`).** Both models run on the GPU instead of ONNX Runtime
  on the CPU. The default build and the default setting are unchanged.

  The GPU path cannot take face authentication down. OpenVINO is loaded at runtime
  rather than linked, so a build with the feature still starts where it is not
  installed; and if the runtime, its GPU plugin or the GPU itself is missing, or a
  model fails to compile, loading logs a warning and falls back to the CPU.

  Compiled GPU kernels are cached in `VISAGE_INFERENCE_CACHE_DIR` (default `gpu-cache`
  beside the model directory). Without the cache every daemon start, including the
  restart after each resume, recompiles both models, which takes seconds on an
  integrated GPU.

  The packaged unit is unchanged: GPU users add `DeviceAllow=char-drm rw` with
  `systemctl edit visaged`, so nobody else's sandbox grows. On NixOS,
  `services.visage.inference.device = "gpu"` sets both.

- **`VISAGE_CPU_THREADS`: ONNX Runtime threads per model**, previously hard-coded to 2.
  The default is unchanged. On a 4-core, 8-thread i7-1160G7 (plugged in, balanced
  profile), 4 threads took SCRFD from about 165 to 125 ms and ArcFace from about 135 to
  100 ms per frame. Also `services.visage.inference.cpuThreads` on NixOS and
  `cpu_threads` in `visage status`.

- **A framing phase before each enrollment capture.** The user gets a moment to see
  themselves and get centred before anything is captured, instead of being told to
  look at a lens and hoping.

  Frames stream to the enrolling client for `VISAGE_FRAMING_MS` (default 1500) and
  are then discarded — they never reach the model. Setting it to `0` disables the
  phase. A client that does not ask for previews skips it entirely, so scripted and
  headless enrollments pay none of its latency.

  ⚠️ This is not purely additive. Sensor auto-gain only adapts while the camera is
  streaming, so holding the stream open for framing acts as an extended warmup and
  leaves AGC in a different state than a bare capture would. That is very likely an
  improvement — it is the same mechanism behind the #104 fix — but it is a real
  change to capture conditions rather than a no-op.

  Bounded by time rather than by a frame count on purpose: the counted capture path
  credits only non-dark frames toward its target, so on hardware where most frames
  read dark — an unquirked emitter, which is exactly where a preview helps most —
  asking for N frames can block for `N * 3` dequeues.

- **`visage-enroll` — enrollment with a live view of the camera.** Shipped in the
  `.deb`, the RPM and the AUR package alongside `visaged` and `visage`.

  Enrollment used to be blind: look at a lens, captures happen, and nothing tells you
  whether you were too dark, off-centre or out of frame. `visage-enroll` subscribes to
  `PreviewFrame` and renders the feed as half-block Unicode with truecolor, which needs
  no terminal graphics protocol. A dark frame is drawn and **labelled** rather than
  dropped, and "too bright" is computed client-side — the daemon's `is_dark_frame` only
  counts pixels below 32, so the white-out that #104 fixed produces frames it considers
  good and never flags.

  Validated on real hardware 2026-09-16 against a Shinetech `3277:0055` IR module:
  **4 of 4 angles enrolled**, best-face confidence 0.8657, the preview legible at the
  160 px cap, and `require_root_caller` exercised on the system bus rather than skipped.
  It adds no new runtime dependency to any package — ratatui and crossterm are pure Rust.

- **`visage-ipc` — one shared D-Bus proxy.** The `#[zbus::proxy]` definition lived
  inline in the CLI; three clients would have meant three copies, which is the
  duplicate-definition drift this repo has already shipped twice. One definition
  also gives `dbus_contract.rs` a single place to look — it now skips
  `#[zbus(signal)]` declarations, which it would otherwise scrape as methods and
  fail on arity, since the server's `preview_frame` carries a `SignalEmitter` the
  client never sees.

- **`services.visage.framingMs`** — the framing phase is now configurable on NixOS.
  It shipped in the daemon reading `VISAGE_FRAMING_MS` with no module option at all, so
  on the one platform Visage is the default authentication layer for, the phase could
  neither be tuned nor disabled. `0` disables it; `null` keeps the compiled 1500 ms.

- **`visage test --strobe` — an instrument for measuring the IR emitter's lit/unlit swing.**
  Streams raw frames (no CLAHE, one stream) and reports the brightness delta between adjacent
  halves, on the brightest decile of pixels as a face proxy. Writes every frame plus a TSV
  manifest per labelled run so two conditions can be compared. It gates nothing.

  Needed a new camera method: `capture_frames_observed` CLAHE-equalises lit frames while dark
  ones stay raw, so comparing them measures the enhancement rather than the sensor, and
  `capture_frame` builds a new stream per call, which samples disconnected points in the
  strobe. `Camera::stream_raw_frames_for` does neither.

  ⛔ Its first result killed a roadmap item. `threat-model.md` listed IR strobe analysis as a
  replay-attack countermeasure and the hardware report called it "an unused anti-spoof
  primitive". Measured on `3277:0055`: a phone screen swings **1.71× more** than a live face,
  because glass is specular and skin is diffuse — and the value is attacker-controllable by
  tilt. Both documents are corrected and the raw data is committed.

- **A contract test tying the daemon's knobs to the NixOS module**
  (`tests/nixos_options_contract.rs`), in both directions: every `VISAGE_*` the daemon
  reads must be settable from the module, and the module must not set one the daemon
  never reads. The second direction catches a rename leaving an option silently inert.
  `VISAGE_SESSION_BUS` is the one deliberate exclusion — it also skips
  `require_root_caller`, so an option for it would be an option for weakening the
  service's own access control.

  This is the test that was missing when `framingMs` went unexposed: a missing option is
  not a build error, not a warning, and not visible in `visage status`.

### Removed

- **`visage-gui`, the egui enrollment prototype.** It existed to settle whether a
  graphical front-end or a terminal one better helps a first-time user, by use rather
  than by argument. Answered: the terminal one, and for a structural reason rather than
  a matter of taste — `Enroll` is root-only, and a GUI under `sudo` fights Wayland/X
  authorization and `XDG_RUNTIME_DIR`, while a TUI under `sudo` is ordinary. Making the
  GUI viable would need polkit or a root helper, which is real work to answer a question
  that is now closed.

  Dropping it removes **199 packages** from the lockfile — the whole `eframe` / `winit` /
  `glutin` / `wgpu` tree — with nothing added and no version change to any surviving
  dependency. That is a meaningful reduction in dependency surface for a security
  component, and it is the concrete argument for deleting a prototype rather than
  leaving it parked in the tree.

### Fixed

- **`pam_visage` could hold the password prompt 1.5 s past `timeout=N`.** After a face
  that did not match, the retry loop from #127 slept 1.5 s before its one slow retry
  without checking the budget, then found no time left and gave up. On the-first, where
  a full verify takes about 2.3 s and the lock screen uses the 3 s default, a wrong or
  unrecognised face held the prompt for about 3.8 s and gained nothing. The loop now
  skips any wait that would end at or after the deadline, so no path runs past
  `timeout=N`. The loop is a separate function with the clock passed in, and six
  tests drive it on a fake clock, including a sweep over 360 cases that asserts the
  budget is never exceeded.

- **`visage-enroll` destroyed its own verdict on exit.** ratatui runs in the terminal's
  alternate screen, which restores the previous contents when it leaves — so the reason
  an enrollment failed vanished the moment the user pressed `q`. Measured on real
  hardware 2026-09-16: a run that never reached `Enroll` spent 26 seconds displaying its
  diagnosis and left **nothing** in the scrollback, so the failure could not be reported
  or acted on. For a tool whose entire purpose is saying why enrollment failed, that
  defeated the point of building it.

  The capture outcomes and closing summary now print to stdout after the terminal is
  restored. Extracted as `report_lines()` rather than printed inline so the property
  that matters — the reason reaches the user — can be asserted in a test rather than
  only observed.

- **The IR emitter warning asserted something it cannot know, and was wrong.** It read
  `no IR emitter quirk for device; proceeding without illumination`. On Shinetech
  `3277:0055` the emitter strobes lit/unlit every frame by firmware default with no quirk
  present — measured at 10 good / 9 dark frames, mean brightness 54.8 — so the
  illumination claim is false on shipped hardware. Two separate documents in this repo
  already carried a correction for that one line, and it still sent a reader chasing a
  missing quirk as the cause of a capture failure it had nothing to do with. It now
  states what Visage does (it will not control the emitter) and points at `visage test`.

- **The CLI could talk to a different bus than the daemon.** It decided with
  `env::var("VISAGE_SESSION_BUS").is_ok()`, so `VISAGE_SESSION_BUS=0` meant *yes*
  to the client and *no* to the daemon: the daemon served the system bus, the
  client connected to the session bus, and the user was told "is visaged
  running?". The daemon had already fixed this in `parse_session_bus`; both sides
  now call the same function.

- **`nix build` ran half the test suite and reported success.** The package's
  `checkPhase` ran `cargo test --workspace --lib`, and `--lib` selects *only*
  library targets — `visaged` and `visage-tui` are binary-only crates (as was
  `visage-gui`, removed above), and every `tests/*.rs` is a `--test` target. Of 127
  test functions it
  ran roughly 67 and silently skipped the rest, including **every contract test**.
  It now runs the whole workspace. The three camera-dependent tests remain
  `#[ignore]`d and are unaffected.

## v0.4.0 — 2026-09-16

The first stable 0.4.0. It carries everything in v0.4.0-rc.1 below — one-command
onboarding via `visage onboard`, the configurable PAM timeout, the first integration
tests, and the repaired tag-triggered release pipeline — plus the changes listed here.

### Added

- **`PreviewFrame` — the daemon can show a client what the camera sees during an
  enrollment.** Enrollment was previously blind: four captures happened and nothing
  said whether the user was too dark, off-centre or out of frame.

  There is deliberately **no method to request a camera frame**. The only producer is
  an `Enroll` the caller started, frames are unicast to that caller's own bus name
  rather than broadcast, and the channel lives for exactly the duration of that call —
  so the preview cannot be used as a general camera tap, and it adds no privilege
  boundary beyond `Enroll`'s existing root-only check.

  Frames are 8-bit grayscale, downscaled **in the engine thread before crossing any
  channel** to at most 160px on the longest edge: enough to check framing, and
  deliberately not enough to be a useful biometric capture. Frames the capture loop
  rejects as too dark are emitted too, flagged — "too dark" is the most actionable
  thing a user can be told, and it is only knowable inside that loop.

- **IR emitter quirk for the Lenovo ThinkPad P14s Gen 4 — Syntek `174f:11a8`.** The
  project's third externally contributed quirk, and the second from this contributor.
  ([#102](https://github.com/sovren-software/visage/pull/102))

- **Fedora RPM packaging via `cargo-generate-rpm`.** `cargo generate-rpm -p crates/visaged`
  now produces an RPM carrying the same assets as the `.deb` at Fedora's paths, with the PAM
  module in `/usr/lib64/security`.

  Fedora has no `pam-auth-update` and `authselect` owns `system-auth`, so PAM configuration
  stays manual there. The package ships the one-line snippet using the same
  `[success=done default=ignore]` control as every other packaging channel, and the
  `pam_control_contract` test now covers that file too — so the control keyword cannot drift
  in the RPM without failing the suite.

  Tested on Fedora 44 x86_64: build, `dnf install`, `dnf` upgrade, `sudo`, polkit, and GDM
  unlock by face.

  This does **not** close [#101](https://github.com/sovren-software/visage/issues/101). That
  issue asks for an RPM *and/or* COPR and calls COPR preferable; this delivers
  `cargo-generate-rpm` metadata, not a `.spec` or a COPR repository, and the RPM is not yet
  wired into the release workflow — a Fedora user still builds it themselves.
  ([#103](https://github.com/sovren-software/visage/pull/103))

- **`contrib/pam/visage-has-session` — a session gate for GNOME.** GNOME unlocks the login
  keyring from the password entered at first login, so authenticating that first login by
  face leaves the keyring locked. The helper distinguishes a first login from a later unlock,
  letting a PAM stack ask for the password once and accept a face every time after.
  ([#105](https://github.com/sovren-software/visage/pull/105))

### Fixed

- **The first verify after every daemon start failed on cameras with a quirked emitter.**
  In practice: the first `sudo` after boot, and the first unlock after every resume, since
  `visage-resume.service` restarts the daemon.

  Sensor auto-gain only adapts while the camera is streaming, drops within about two seconds
  when a quirked emitter saturates it, and recovers slowly. `capture_frame()` built and tore
  down a fresh `MmapStream` on **every call**, so the startup warmup never held a continuous
  stream and gain settled against ambient light with the emitter off — leaving the first real
  capture blown out to white and undetectable.

  The warmup now activates the emitter first and runs as one continuous `capture_frames`
  stream. `VISAGE_WARMUP_FRAMES` counts *usable* frames, skipping the unlit half of a strobing
  emitter's cycle, and is now documented.

  Measured A/B on Syntek `174f:11a8` from the same starting state, reproducing each path's
  exact capture sequence — the detector received frame means `[33, 255, 33]` before the fix
  (silhouette, white-out, silhouette) and `[24, 24, 24]` after, with the face crisply lit.
  ([#104](https://github.com/sovren-software/visage/pull/104))

- **The published MSRV was false, and CI now verifies it.** The workspace declared
  `rust-version = "1.75"` while 59 packages in the dependency tree require more — `ort` and
  `image` both at 1.88 — so the crate could not build on the version it advertised. Corrected
  to 1.88, with a CI job that keeps the claim true.
  ([#93](https://github.com/sovren-software/visage/pull/93))

- **The quirk contribution workflow was stale, and one shipped quirk was undocumented.** The
  hardware compatibility table listed four quirks while five shipped, omitting the project's
  first external hardware contribution.
  ([#94](https://github.com/sovren-software/visage/pull/94))

### Security

- **RUSTSEC-2026-0204 — `crossbeam-epoch` 0.9.18 → 0.9.21.** Invalid pointer dereference in
  the `fmt::Pointer` implementation for `Atomic` and `Shared`; the advisory requires ≥ 0.9.20.
  It reaches the tree transitively and undeclared, via
  `crossbeam-deque ← rayon-core ← exr/rayon ← image`.

  This single crate is what had made the scheduled **Security Audit workflow fail every run
  from 2026-07-13 to 2026-09-14 — ten consecutive weeks.** There is no `audit.toml`, so the
  nine "allowed warnings" the job also reports never failed it.
  ([#109](https://github.com/sovren-software/visage/pull/109))

- **RUSTSEC-2026-0285 — `rustls` 0.23.36 → 0.23.45**, carrying `rustls-webpki`
  0.103.13 → 0.103.15. rustls incorrectly accepts TLS 1.3 handshake messages across encryption
  level boundaries; severity 5.3 (medium). Transitive via
  `ureq ← visage-cli` and `ort-sys ← ort ← visage-core ← visaged`.

  Published 2026-09-14 — the day after the last scheduled audit run — and it entered this tree
  through the `ureq 3.4.1` bump in this same batch. `cargo audit` against the merged lockfile
  returned exit 1 before this change and exit 0 after.
  ([#112](https://github.com/sovren-software/visage/pull/112))

### Changed

- **`audit.yml`'s toolchain is pinned, mirroring `ci.yml`.** It was still floating on
  `dtolnay/rust-toolchain@stable` — the exact pin whose six-day outage in August blocked every
  merge in the repository, where an identical commit passed CI on 08-18 and failed on 08-24
  because the action resolved onto a clippy carrying a new lint. `ci.yml` was pinned in
  response; `audit.yml` was missed and floated for three more weeks, in the one job whose
  purpose is noticing problems. `rust-toolchain.toml` is now the single source of truth for
  both workflows.

  The trade-off runs the other way for this job, and the file says so in a comment:
  `cargo install cargo-audit --locked` needs a toolchain new enough to *build* cargo-audit, so
  a pin that falls behind its MSRV breaks the job. Headroom at the time of pinning was 1.95.0
  against cargo-audit 0.22.2's required 1.88 — and that failure would be loud, attributable,
  and fixed by editing one file.

- Dependency bumps: `anyhow` 1.0.103 → 1.0.104, `serde` 1.0.228 → 1.0.229, `ureq` 3.4.0 →
  3.4.1, `toml` 1.1.3 → 1.1.6, `uuid` 1.24.0 → 1.26.1.

## v0.4.0-rc.1 — 2026-08-24

### Added

- **`visage onboard` — one command from nothing to working face auth.** Downloads
  the ONNX models, enrolls several labelled angles with a prompt between each, and
  verifies against the daemon before claiming success. Replaces the `setup` →
  `enroll` → `list` → test sequence, which had two traps: `enroll` defaulted to the
  wrong user under sudo (below), and a single capture is fragile on hardware where
  the IR emitter strobes or has no quirk entry.

  It refuses to onboard `root` unless `--user root` is given explicitly, keeps
  whichever captures succeeded if one fails, and exits non-zero if verification does
  not recognise the enrolled face — so a failed onboarding cannot look like a
  successful one.

- **First hardware validation of passive liveness — and it did not discriminate.**
  New hardware report for the ASUS Zenbook 14 UM3406HA / Shinetech `3277:0055` IR
  module, executing the blocking §2 checklist in `docs/liveness-remaining-work.md`
  that had been open since 2026-02-25.

  A hand-held phone-screen spoof produced landmark displacement of **0.681 px** —
  *higher* than two genuine live attempts (0.263, 0.670) — so no threshold on that
  metric admits the live minimum while rejecting the spoof. The identity stage
  matched the same photo at **0.9013**. Live users were falsely rejected 13–17% of
  the time at the default 0.8 px floor — 20/23–20/24 live pass as of the journal at
  2026-08-17 13:43 local, so the ≥9-in-10 reliability criterion is not met. `DEFAULT_MIN_EYE_DISPLACEMENT` is documented
  against 640×480@30fps; this sensor is 640×360, and its "printed photo <0.3 px"
  figure assumes a *rigidly mounted* photo rather than one held in a hand.

  Sample is **n=1** on the spoof side, so `threat-model.md`'s "static photo" claim is
  deliberately left unrevised. Do not lower `liveness.minDisplacement` on this
  evidence — it would widen the hole rather than close it.

  Also documented: this model ships **more than one camera module**, so the
  `04f2:b6d9` quirk does not apply to `3277:0055` units, and a *missing* quirk does
  not imply *missing illumination* — the emitter strobes by firmware default here.

- **`pam_visage.so` now accepts a `timeout=N` module argument**, exposed as
  `services.visage.pam.timeoutSeconds`. The D-Bus method timeout was hard-coded
  at 3 seconds.

  That is not only a hang guard — it is an upper bound on how long
  authentication may take. A verify that exceeds it makes PAM fall through to
  the password prompt, and the outcome is **indistinguishable from a failed
  match**: both return `PAM_IGNORE`, and nothing in the PAM log says "timed
  out". On CPU-only hardware a verify can legitimately take seconds; measured
  median on an ASUS Zenbook 14 was **2273 ms**, leaving under 700 ms of
  headroom against a 3-second ceiling. The symptom of getting that wrong is
  intermittent password prompts that look exactly like recognition failures.

  A malformed, zero, or unparseable value logs a warning and falls back to the
  3-second default rather than failing — a typo in a PAM line must never be
  able to lock a user out.

- **Five daemon settings are now NixOS options** — `framesPerVerify`,
  `framesPerEnroll`, `warmupFrames`, `verifyTimeoutSeconds`, and
  `emitter.enable`. The daemon has always read the corresponding
  `VISAGE_*` environment variables, but the module exposed none of them, so the
  only way to tune a deployment was a hand-written systemd drop-in.

  `framesPerVerify` is the main latency knob, and its documentation now records
  both interactions that make it non-obvious: the PAM timeout above, and the
  fact that passive liveness fails closed below two frames with a detected
  face — so lowering it to 2 can reintroduce false rejects.

- **The hardware quirks database is now tested.** `crates/visage-hw/src/quirks.rs` had no
  tests, and `quirk_db()` skips a malformed TOML rather than panicking — correct for a daemon
  that authenticates logins, but it means an embedded-but-broken quirk compiles, ships, and
  silently never fires. At runtime that is indistinguishable from a camera with no quirk.

  Four assertions now run in CI: every embedded source parses (count compared against
  `QUIRK_SOURCES.len()`, so it cannot drift as cameras are added), every parsed quirk is
  reachable via `lookup_quirk`, no two entries claim the same VID:PID, and emitter payloads are
  coherent (non-empty `control_bytes`; `off_bytes` matching its length). Each was verified to
  **fail** on a deliberately broken input, not merely to pass.

  `contrib/hw/README.md` also gained the registration step it was missing. It previously went
  from "create a TOML file" straight to "submit a PR", never mentioning that a file must be
  added to `QUIRK_SOURCES` to be read at all — so a contributor following it literally would
  ship a quirk that does nothing.

### Changed

- **`services.visage.pam.enable`'s description no longer overstates what it
  wires.** It claimed face auth for "sudo, login, and screen lock"; it wires
  `sudo` and `login` only. Screen lock is covered *indirectly* and only for
  lockers that derive their PAM stack from `login` — DMS/quickshell generates a
  config with an identical module set, and so inherits face auth for free.
  Lockers declaring their own PAM service get nothing and must be wired
  explicitly; the description now shows how, and warns that grepping
  `/etc/pam.d/` alone will report the lock screen as unwired when it is not,
  because a locker's config may live in the user's state directory.

- **`visage onboard` no longer verifies the instant the last capture returns.**
  It paused for zero milliseconds, so `[3/3]` fired while the user was still
  holding the pose they struck to press Enter — the posture passive liveness is
  most likely to reject, since it looks for landmark movement. Onboarding could
  enroll perfectly and then fail its own verification for a reason unrelated to
  enrollment quality. Now waits two seconds and says what to do with them.

- **`visage onboard`'s failure message no longer asserts a cause it cannot
  know.** It told users to check lighting and the IR emitter quirk. The daemon
  deliberately collapses every failure into a plain non-match — distinguishing
  "wrong face" from "identity matched but liveness rejected" would tell an
  attacker holding a photograph that the photograph was recognised — so the CLI
  genuinely does not know why a verify failed. It now says so, points at
  `journalctl -u visaged`, and lists the causes in rough order of likelihood
  with the liveness one first.

### Fixed

- **Nix package: `bindgen` could not find libclang.** `v4l2-sys-mit` (pulled in by
  `visage-hw` for camera capture) runs `bindgen` in its build script, which dlopens
  libclang. `packaging/nix/default.nix` declared only `pkg-config` in
  `nativeBuildInputs`, so `nix build .#visage` failed with *"Unable to find libclang
  … set the `LIBCLANG_PATH` environment variable"*. Added `rustPlatform.bindgenHook`.

  `flake.nix` has carried `llvmPackages.libclang` + `LIBCLANG_PATH` for the devShell
  since it was written, so `cargo build` in a dev shell always worked — the *package*
  never did. Nothing caught the difference because no consumer referenced
  `pkgs.visage`: `services.visage.enable` is set on no host and the package is in no
  `systemPackages`, so the derivation was never realised and every build stayed green
  for reasons unrelated to it.

- **Nix package: version label was three patch releases stale.** The derivation
  hardcoded `version = "0.3.3"` while `[workspace.package]` was at `0.3.6`, so any
  built artifact carried a wrong externally-visible version. Now read from
  `Cargo.toml` so the two cannot drift.

- **Nix package: ONNX Runtime is now fetched hermetically.** `ort-sys` downloads a
  prebuilt runtime from `cdn.pyke.io` in its build script, which a sandboxed build
  correctly blocks — so `nix build .#visage` could never work offline. Pointing
  `ORT_LIB_LOCATION` at nixpkgs' `onnxruntime` does not help either: `ort` links
  **statically** by default and nixpkgs ships only `.so`, giving *"could not link to
  the ONNX Runtime build in …"*.

  Now the same archive `ort-sys` wants is fetched via `fetchurl` with a pinned hash
  and unpacked into a small derivation that `ORT_LIB_LOCATION` points at.
  Version-matched by construction (1.23.2, what `ort 2.0.0-rc.11` expects), so no
  version skew, and reproducible.

  The archive is **raw LZMA2, not an `.xz` container**, and needs
  `xz --format=raw --lzma2=dict=64MiB`. The 64 MiB dictionary is required and is not
  the default: plain `--lzma2` decodes only ~8.9 MB of the ~93 MB payload and exits
  non-zero, which — piped into `tar` — looks like a clean extraction, because a
  truncated `ar` archive still yields a plausible `libonnxruntime.a`. A size floor
  now refuses any decode under 80 MB.

- **Nix package: PAM module path no longer hardcoded.** `postInstall` looked for
  `target/release/libpam_visage.so`, but current `rustPlatform.buildRustPackage`
  passes `--target`, so cargo emits to `target/<triple>/release/`. It is now located
  at install time and the build fails loudly if it is absent or ambiguous — which it
  was, three times over: `release/deps/`, `release-tmp/`, and the real one.

- **`clippy::excessive_precision` on two pre-existing test literals**, newly flagged
  by a more recent clippy and failing `cargo clippy -- -D warnings`. In
  `alignment.rs` the value carried a trailing zero and was trimmed. In `store.rs` the
  excess precision is deliberate — that test asserts a bit-exact f32 round-trip — so
  it is annotated with a justified `#[allow]` rather than truncated, which would have
  quietly weakened the case it exists to test.

- **`--user` defaulted to `root` under `sudo`, silently enrolling the wrong account.**
  Every privileged subcommand is root-only by the D-Bus policy, so they are always
  run as `sudo visage …` — where `$USER` is `root`. `sudo visage enroll --label x`
  therefore registered the face against **root** while PAM went on looking up the
  real user, found nothing, and fell through to the password prompt.

  The failure was silent in the worst way: enrollment printed *"Enrolled
  successfully"*, `sudo` kept asking for a password, and nothing indicated the two
  were about different users. It also left a face credential attached to the most
  privileged account on the machine, created by accident.

  `current_user()` now prefers `SUDO_USER`, falling back to `$USER` when not under
  sudo. The help text on `enroll`, `verify`, `list` and `remove` said "defaults to
  `$USER`" and has been corrected too.

### Developer experience
- **The first integration tests.** `crates/visaged/tests/` — 12 hermetic tests, plus 3
  hardware tests that CI skips. Added because **both of the worst bugs in this project's
  history were structurally invisible to unit tests**: `success=end`, an invalid PAM control
  keyword that libpam silently treats as `ignore`, which made face auth a no-op on the
  documented setup path from v0.1.0 to v0.3.2; and the V4L2 format cache (#48). Neither was a
  logic error in a function — both were contracts *between artifacts*, which is the seam a
  unit test cannot reach.

  `pam_control_contract` asserts the control string is a valid libpam control and identical
  across the Debian, Arch and NixOS packaging — it rejects `success=end` by name.
  `systemd_hardening_contract` asserts every directive `threat-model.md` claims exists in
  **both** systemd definitions, which is issue #78's class generalised.
  `packaging_paths_contract` asserts the deb ships artifacts the workspace actually builds and
  that `ExecStart` matches the install path. `dbus_contract` asserts the interface the daemon
  serves is the one its three independently-compiled clients call — a rename there currently
  compiles clean and breaks only at an authentication prompt.

  Hermetic: no hardware, no root, no D-Bus, no models, **no new dependencies**, and no CI
  changes — `cargo test --workspace` already picks them up. Each guard was proven to fail on
  the defect it exists for before being committed.

- **Fixed: `TimeoutStopSec` was missing from the NixOS module.** Found by the new
  `systemd_hardening_contract`, which failed on the tree as it stood. The directive was added
  for #26 to bound a stuck v4l2 capture on `systemctl stop|restart`, cutting a ~90s
  post-hibernate hang to ~10s. It was present in the Debian unit only, so **NixOS hosts never
  got the fix** — a running host reported systemd's 90s default, exactly the hang it was meant
  to remove.


- **Releases are cut from tags again, and cannot publish empty.** v0.3.4, v0.3.5
  and v0.3.6 shipped with no `.deb` attached (issue #75). The release job gated
  on the head commit message starting with `release:`, and adopting Conventional
  Commits (`chore(release): 0.3.4`) silently severed it — nothing failed,
  because each release was then created by hand and looked correct. The job is
  now triggered by a `v*` tag, asserts the tag matches `Cargo.toml`, attaches
  only that version's changelog section rather than the whole file, derives
  pre-release status from the version string, and refuses to publish at all if
  no `.deb` is present.

- **The toolchain is pinned, so CI cannot change behaviour without a commit.**
  The identical commit passed CI on 2026-08-18 and failed on 2026-08-24, purely
  because `dtolnay/rust-toolchain@stable` floated onto a compiler whose clippy
  added a new lint; with `-D warnings` that halted every merge in the repository
  for six days. `rust-toolchain.toml` now pins the version — tracking what
  `nix develop` provides, so a local `cargo clippy` reproduces CI exactly — and
  the workflow reads it as the single source of truth. A new non-blocking
  `clippy-latest` job still runs against current stable, so new lints surface
  early without being able to block a merge.

### Security

- **The daemon's network isolation is now enforced, not merely asserted.**
  `threat-model.md` justifies its largest documented hardening gap —
  `MemoryDenyWriteExecute=false`, required by ONNX Runtime's JIT — by stating
  that the daemon "has no network access, no inbound connections". No unit
  directive enforced that, so the compensating control for that exception did
  not exist. Reported as issue #78.

  `visaged.service` now sets `PrivateNetwork=true` in both the packaged unit
  and the NixOS module, placing the daemon in its own network namespace with
  loopback only. `visaged` makes no network calls of its own; the only HTTP
  client in the workspace is `ureq`, used solely by `visage-cli` for `visage
  setup` model downloads, which runs as a separate process and is unaffected.
  D-Bus is an AF_UNIX filesystem socket and is not namespaced by
  `PrivateNetwork`.

### Notes

- `nix build .#visage` now succeeds end-to-end: compile, unit tests, and install of
  the daemon, CLI, PAM module, D-Bus policy and both systemd units.

## v0.3.6 — 2026-07-07

Security hardening batch — defense-in-depth on the D-Bus authorization surface,
a fail-open → fail-closed correction in passive liveness, and a runtime-safety
pin on the async executor. No public API or wire-format changes.

### Security

- **In-process root check on the privileged D-Bus methods** (`Enroll`,
  `RemoveModel`, `ListModels`). These were root-only by the system-bus policy
  file (`org.freedesktop.Visage1.conf`) alone; a missing, mis-scoped, or
  overly-permissive policy — or running on the session bus — could let a
  non-root caller invoke enrollment mutation or the enrollment listing. The
  daemon now re-verifies the caller is root (UID 0) inside each handler (skipped
  on the session bus, development mode), mirroring the defense-in-depth `Verify`
  already applied.
- **`VISAGE_SESSION_BUS=0` no longer enables session-bus mode.** Session-bus
  mode *skips* D-Bus caller-UID validation (development only). The flag was read
  with `env::var(..).is_ok()`, so *any* value — including `VISAGE_SESSION_BUS=0`,
  the natural way to turn it off — enabled it and silently disabled UID
  validation (fail-open). It now enables only on a non-empty, non-`"0"` value;
  unset, empty, and `"0"` all keep the secure system-bus default.
- **Passive liveness now fails closed on insufficient landmark data.**
  `check_landmark_stability` reported "live" when fewer than two landmark frames
  were available (fail-open), so a match backed by only a single detectable
  landmark frame bypassed the liveness gate entirely. It now returns not-live in
  that case; the daemon surfaces it as a (rate-limited) non-match and the user
  retries. `frames_per_verify` defaults to 3, so a live subject in normal
  lighting is unaffected.

### Changed

- **Pinned `zbus` to the `tokio` executor** (`default-features = false,
  features = ["tokio"]`; `pam-visage` re-adds `blocking-api`), following zbus's
  own recommendation for tokio integration. `visaged` awaits tokio primitives
  inside `#[zbus::interface]` handlers; on zbus's default `async-io` executor any
  reactor-bound tokio call added later would panic with "no reactor running", and
  a single transitive dependency could silently revert the whole process to
  `async-io` via Cargo feature unification. This also removes the `async-io` /
  `smol` executor stack from the dependency tree.

### Added

- **AES-256-GCM known-answer test + on-disk blob-format guard** (`visaged`).
  Locks the embedding-encryption primitive against the NIST GCM test vector and
  the stored blob layout (12-byte nonce ‖ ciphertext ‖ 16-byte GCM tag, with
  AEAD tamper rejection), so a future `aes-gcm` upgrade cannot silently change
  the on-disk format and orphan existing enrollments.

## v0.3.5 — 2026-07-07

### Added

- **Hardware support: HP OmniBook X Flip IR camera** (`30c9:0120`, Luxvisions).
  Contributed by @mocha in #47. Adds an IR-emitter quirk-schema extension
  (`off_bytes`, `reset_on_close`) for devices whose emitter rejects an all-zero
  "off" write (`ERANGE`); existing quirk files are unaffected (both fields
  default). Quirk file: `contrib/hw/30c9-0120.toml`.
- **Hardware support: Lenovo ThinkBook 14 MP2PQAZG IR camera** (`30c9:00c2`).
  Contributed in #45 (previously not captured in the changelog). Quirk file:
  `contrib/hw/30c9-00c2.toml`.

### Security

- **`openssl` 0.10.75 → 0.10.81** and **`rustls-webpki` 0.103.9 → 0.103.13**
  (Dependabot security updates, #60 / #59) — pull RustSec-advisory fixes into
  the TLS dependency chain (`ort` → `ureq`).

## v0.3.4 — 2026-07-07

### Fixed

- **NixOS / Nix flake build: add `openssl` to `buildInputs`** (issue #38). The
  Nix derivation failed to build because `ort` (ONNX Runtime) pulls in `ureq` →
  `native-tls` → `openssl-sys`, whose build script needs the system OpenSSL
  library at link time. `nativeBuildInputs` already provided `pkg-config`, but
  `buildInputs` was missing `openssl`, so `openssl-sys` could not locate it.
  A follow-up can drop the C TLS dependency entirely by building `ort` with
  `default-features = false, features = ["load-dynamic"]` against
  `pkgs.onnxruntime`.
- **Camera capture no longer degrades over long sessions on a shared webcam**
  (issue #48). `visaged` negotiated the V4L2 capture format only once, at
  `Camera::open`, and never re-asserted it. On a webcam shared with other
  applications (e.g. a video-conferencing app), another process could change the
  device's format via `VIDIOC_S_FMT` and leave it there; `visaged` then captured
  wrong-format frames it decoded as garbage through its stale format cache —
  surfacing as "no face detected" until a manual `systemctl restart`. The daemon
  now re-asserts its format before each capture (a cheap `VIDIOC_G_FMT`; `S_FMT`
  only fires when the device actually drifted) and, as a safety net, re-opens the
  camera in-process after repeated capture failures instead of requiring a
  restart. The per-capture stream is retained, so the camera is still released
  between verifies and remains usable by other applications.
- **AUR install hook: corrected PAM keyword `success=end` → `success=done`.**
  `packaging/aur/visage.install` still printed the setup guidance with the
  invalid `[success=end …]` action — the same bug fixed everywhere else in
  v0.3.2. libpam treats the unknown `end` as `ignore`, so a user following the
  printed line verbatim would get a silent face-auth no-op. Now prints
  `[success=done default=ignore]`.

### Security

- **CI: added a scheduled `cargo audit` workflow** (`.github/workflows/audit.yml`).
  It scans `Cargo.lock` against the RustSec advisory database weekly and on
  demand, surfacing dependency advisories without waiting for a manual check.

## v0.3.3 — 2026-05-28

### Added

- **Hardware support: Lenovo ThinkPad X1 Carbon Gen 9 20XW00FPUS IR camera** (`174f:2454`).
  Verified on hardware. Quirk file at `contrib/hw/174f-2454.toml`. Contributed by
  @themariusus in #29.

### Packaging

- **AUR `PKGBUILD` disables LTO and debug** (`options=(!lto !debug)`). LTO operates on
  LLVM IR, but `ring` ships hand-written assembly via `cc` and `libsqlite3-sys`
  compiles `sqlite3.c` via `cc` — those `.o` files have no LTO-compatible IR, so the
  final link drops or fails to resolve their symbols. Without this, `makepkg -si`
  on a stock Arch system fails at link time with `undefined symbol:
  ring_core_0_17_14__LIMBS_window5_split_window` (and many more from both `ring`
  and `libsqlite3-sys`). Reported and fixed by @SomeCodecat in #25.

### Developer experience

- **`nix develop` shell now ships `rustfmt`, `clippy`, and `libclang`.**
  `inputsFrom = [ visage ]` brought the compiler but not these auxiliaries, so
  contributors hit `error: no such command: fmt` and bindgen failed to find
  `libclang.so`. Devshell now sets `LIBCLANG_PATH` and exposes both cargo
  subcommands matching CI's `dtolnay/rust-toolchain@stable` gates. (#32)

### Dependencies

- `tokio` 1.49.0 → 1.50.0
- `nix` 0.31.1 → 0.31.2
- `uuid` 1.21.0 → 1.23.0
- `image` 0.25.9 → 0.25.10
- `actions/checkout` v4 → v6 (CI)
- `actions/upload-artifact` v4 → v7 (CI)
- `actions/download-artifact` v4 → v8 (CI)

## v0.3.2 — 2026-05-28

### Fixed

- **PAM control keyword corrected: `success=end` → `success=done` across all 9 sites.**
  `pam.conf(5)` documents exactly `ignore | bad | die | ok | done | reset | N` —
  `end` is not a valid keyword. libpam logged a warning and treated it as
  `ignore`, meaning a successful face match silently fell through to the next
  rule (typically `pam_unix.so` → password prompt) instead of terminating the
  auth stack with success. Affected: `README.md`, `docs/operations-guide.md`,
  `docs/architecture.md`, `packaging/debian/pam-auth-update` (Ubuntu),
  `packaging/nix/module.nix` (NixOS — `sudo` and `login` rules), and several
  research docs. Caught by @SelfRef in #27. **Note for existing users:** if your
  PAM stack still references the old keyword (e.g. you manually edited
  `/etc/pam.d/system-auth` on Arch from the prior README, or you're on an old
  Debian/Ubuntu install that hasn't re-run `pam-auth-update`), face auth has
  been working as if Visage weren't installed — replace `success=end` with
  `success=done` and re-test.
- **`visaged` now handles SIGTERM correctly.** The shutdown signal handler in
  `crates/visaged/src/main.rs` previously relied on `tokio::signal::ctrl_c()`,
  which is SIGINT-only on Unix. `systemctl stop` / `systemctl restart` (and
  `visage-resume.service` after suspend/hibernate) send SIGTERM, which the daemon
  ignored — systemd then waited the default `TimeoutStopSec=90s` before escalating
  to SIGKILL, manifesting as a ~90s hang on `systemctl restart visaged.service`
  after hibernate resume. Visaged now installs handlers for both SIGINT and SIGTERM
  via `tokio::signal::unix::signal` and shuts down cleanly. Fixes #26.
- **`visaged.service` adds `TimeoutStopSec=10s`** as defense in depth — covers the
  edge case where a v4l2 capture is mid-flight and not promptly interruptible
  (e.g. a stale camera fd after hibernate resume). Fixes #26.

### Documentation

- Added ASUS ExpertBook B3302FEA/B5302FEA hardware validation showing the built-in
  Azurewave/IMC `13d3:56ea` UVC webcam is RGB-only and not compatible with
  Visage's secure IR-backed PAM authentication path.

## v0.3.0 — 2026-02-23

### What's changed

- **Security-first model integrity** — ONNX model files are now verified via pinned SHA-256.
  `visage setup` verifies checksums on download and `visaged` verifies the model directory at
  startup (fails closed on missing/mismatched models).
- **Shared model manifest** — added `visage-models` crate containing the model list and
  verification helpers used by both the CLI and daemon.
- **OSS contribution governance** — added `SECURITY.md` (private vulnerability reporting
  via GitHub Security Advisories), branch protection on `main` (required PR + CI + review),
  `CODEOWNERS`, issue/PR templates, DCO sign-off policy, Dependabot for dependency updates,
  and documented merge strategy with review timeline commitments. See ADR 010.

## v0.2.0 — 2026-02-23

### What's changed

- **Enterprise identity compatibility** — D-Bus `Verify(user)` caller validation now resolves user IDs via NSS (LDAP/SSSD/AD compatible) instead of parsing `/etc/passwd`.
- **CLI reliability** — `visage` CLI sets a D-Bus method timeout aligned with `VISAGE_VERIFY_TIMEOUT_SECS` (default 10s) to avoid indefinite hangs.
- **Enrollment quality** — enrollment now averages embeddings across captured frames (confidence-weighted) and re-normalizes the result.
- **Store hardening** — face DB blob parsing validates size/dimension and rejects NaN/Inf safely (no panics on corrupted blobs).
- **Status output** — `Status()` JSON includes additional config fields (paths, timeouts, frame counts, emitter/session flags).

## v0.1.0 — 2026-02-23

Initial release. All six implementation steps complete and end-to-end tested on Ubuntu 24.04.4 LTS.

### What's included

- **Camera pipeline** — V4L2 capture with GREY, YUYV, and Y16 format support. CLAHE preprocessing. Dark frame detection and rejection.
- **ONNX inference** — SCRFD face detection + ArcFace recognition via ONNX Runtime. CPU-capable, no CUDA required. Models download via `visage setup` with SHA-256 verification.
- **Persistent daemon** — `visaged` holds camera and model weights across auth requests. D-Bus IPC (`org.freedesktop.Visage1`). SQLite model store with WAL mode.
- **PAM module** — `pam-visage` integrates with any PAM-based application (sudo, login, screen lock). `PAM_IGNORE` fallback — face unavailable always falls through to password. Never blocks.
- **IR emitter control** — UVC extension unit control for Windows Hello-compatible IR cameras. Hardware quirks database (TOML). ASUS Zenbook 14 UM3406HA tested and confirmed.
- **Ubuntu packaging** — `.deb` with `pam-auth-update` integration, systemd hardening (`ProtectSystem=strict`, `NoNewPrivileges=yes`), and clean install/remove/purge lifecycle.
- **Security** — AES-256-GCM embedding encryption at rest, rate limiting (5 failures/60s → 5-min lockout), D-Bus caller UID validation.

### Known limitations

- **Ubuntu 24.04 only** — NixOS, AUR, and COPR packages are in progress.
- **~1.4s verify latency** on CPU-only ONNX with USB webcam. Target <500ms requires IR camera and hardware acceleration.
- **No active liveness detection** — IR emitter and multi-frame capture reduce spoofing risk; active challenge-response (blink detection) is planned for a future release.
- **`MemoryDenyWriteExecute=false`** — required for ONNX Runtime JIT compilation. All other sandbox directives are applied.

### Installation

```bash
# Download visage_0.1.0_amd64.deb from the release assets
sudo apt install ./visage_0.1.0_amd64.deb
sudo visage setup       # downloads ONNX models (~182 MB)
visage enroll           # enroll your face
sudo echo test          # verify PAM integration
```

See [docs/hardware-compatibility.md](docs/hardware-compatibility.md) for camera compatibility tiers and IR emitter setup.

### Requirements

- Ubuntu 24.04 LTS (amd64)
- V4L2-compatible camera (UVC preferred)
- libpam0g, libdbus-1-3 (installed automatically via .deb)

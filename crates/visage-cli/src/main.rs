mod setup;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::time::Duration;

use visage_ipc::VisageProxy;

#[derive(Parser)]
#[command(name = "visage", about = "Visage biometric authentication CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Enroll a new face model
    Enroll {
        /// Label for this face model (e.g., "normal", "glasses")
        #[arg(short, long)]
        label: String,

        /// User to enroll for. Defaults to the invoking user — SUDO_USER under sudo, NOT root.
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Verify your face against enrolled models
    Verify {
        /// User to verify as. Defaults to the invoking user — SUDO_USER under sudo, NOT root.
        #[arg(short, long)]
        user: Option<String>,
    },
    /// List enrolled face models
    List {
        /// User whose models to list. Defaults to the invoking user — SUDO_USER under sudo, NOT root.
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Remove an enrolled face model
    Remove {
        /// Model ID to remove
        id: String,

        /// User who owns the model. Defaults to the invoking user — SUDO_USER under sudo, NOT root.
        #[arg(short, long)]
        user: Option<String>,
    },
    /// One command: download models, enroll several angles, and verify.
    ///
    /// This is the path most people want. Running `setup` then a bare `enroll`
    /// leaves two traps: enroll defaults to `$USER` (which is `root` under the
    /// sudo these commands require), and a single capture is fragile on
    /// hardware where the IR emitter strobes or has no quirk.
    Onboard {
        /// User to enroll for. Defaults to the invoking user (SUDO_USER).
        #[arg(short, long)]
        user: Option<String>,

        /// Comma-separated capture labels, one prompt per label.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "normal,left,right,glasses"
        )]
        labels: Vec<String>,

        /// Model directory override, passed through to setup.
        #[arg(long)]
        model_dir: Option<String>,

        /// Skip the interactive prompt between captures.
        #[arg(long)]
        no_prompt: bool,
    },
    /// Download ONNX models required for face detection and recognition
    Setup {
        /// Model directory (default: /var/lib/visage/models when root, ~/.local/share/visage/models otherwise)
        #[arg(short, long)]
        model_dir: Option<String>,
    },
    /// Show daemon status
    Status,
    /// Recent authentication attempts (newest first). Root-only.
    History {
        /// User to filter by. Defaults to the invoking user — SUDO_USER under sudo, NOT root.
        /// Pass an empty string explicitly to see all users.
        #[arg(short, long)]
        user: Option<String>,

        /// Max rows to show (1-500).
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },
    /// List cameras and their IR emitter quirk status
    Discover,
    /// Run camera diagnostics
    Test {
        /// Camera device path
        #[arg(short, long, default_value = "/dev/video2")]
        device: String,

        /// Number of frames to capture
        #[arg(short = 'n', long, default_value = "10")]
        frames: usize,

        /// Measure the IR emitter's lit/unlit strobe instead of the normal
        /// diagnostic. Streams RAW frames (no CLAHE) and reports how far
        /// brightness swings between the two halves.
        #[arg(long)]
        strobe: bool,

        /// Seconds to stream when --strobe is given.
        #[arg(long, default_value = "3")]
        strobe_secs: u64,

        /// Label for this run, e.g. "live" or "phone-spoof". Recorded in the
        /// manifest and used as the output subdirectory, so two runs can be
        /// compared without one overwriting the other.
        #[arg(long, default_value = "unlabelled")]
        label: String,
    },
}

/// The human who invoked us — NOT `root` when running under `sudo`.
///
/// Every privileged subcommand (enroll, remove, list) is root-only by the D-Bus
/// policy, so in practice they are always run as `sudo visage …`. Reading plain
/// `$USER` there yields `root`, so `sudo visage enroll --label normal` enrolled
/// the face against **root** while PAM went on looking up the real user, found
/// nothing, and fell through to the password prompt.
///
/// That failure is silent in the worst way: enrollment prints "Enrolled
/// successfully", `sudo` keeps asking for a password, and nothing anywhere says
/// the two are about different users. It also leaves a face credential attached
/// to the most privileged account on the machine, created by accident.
///
/// `SUDO_USER` is what sudo itself records about the invoker, so prefer it and
/// fall back to `$USER` when not under sudo. Pass `--user` to override.
fn current_user() -> String {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root")
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string())
}

fn verify_timeout_secs() -> u64 {
    std::env::var("VISAGE_VERIFY_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
}

async fn connect_proxy() -> Result<VisageProxy<'static>> {
    // Was `env::var(..).is_ok()`, which read VISAGE_SESSION_BUS=0 as a *yes*
    // and sent the client to the session bus while the daemon served the
    // system one. Both sides read the same function now.
    let use_session = visage_ipc::session_bus_from_env();
    let timeout = Duration::from_secs(verify_timeout_secs());
    let conn = if use_session {
        zbus::connection::Builder::session()?
    } else {
        zbus::connection::Builder::system()?
    }
    .method_timeout(timeout)
    .build()
    .await
    .map_err(|e| anyhow::anyhow!("failed to connect to D-Bus: {e}"))?;

    let proxy = VisageProxy::new(&conn)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create proxy: {e} — is visaged running?"))?;
    Ok(proxy)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Enroll { label, user } => {
            let user = user.unwrap_or_else(current_user);
            let proxy = connect_proxy().await?;
            println!("Enrolling face model '{label}' for user '{user}'...");
            match proxy.enroll(&user, &label).await {
                Ok(model_id) => println!("Enrolled successfully. Model ID: {model_id}"),
                Err(e) => {
                    eprintln!("Enrollment failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Verify { user } => {
            let user = user.unwrap_or_else(current_user);
            let proxy = connect_proxy().await?;
            println!("Verifying face for user '{user}'...");
            match proxy.verify(&user).await {
                Ok(true) => {
                    println!("Match: verified");
                    // Exit 0 on match (shell-friendly)
                }
                Ok(false) => {
                    println!("No match");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Verification failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::List { user } => {
            let user = user.unwrap_or_else(current_user);
            let proxy = connect_proxy().await?;
            match proxy.list_models(&user).await {
                Ok(json) => {
                    let models: Vec<serde_json::Value> = serde_json::from_str(&json)?;
                    if models.is_empty() {
                        println!("No models enrolled for user '{user}'");
                    } else {
                        println!("Enrolled models for '{user}':");
                        for m in &models {
                            println!(
                                "  {} — label: {}, quality: {:.3}, created: {}",
                                m["id"].as_str().unwrap_or("?"),
                                m["label"].as_str().unwrap_or("?"),
                                m["quality_score"].as_f64().unwrap_or(0.0),
                                m["created_at"].as_str().unwrap_or("?"),
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to list models: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Remove { id, user } => {
            let user = user.unwrap_or_else(current_user);
            let proxy = connect_proxy().await?;
            match proxy.remove_model(&user, &id).await {
                Ok(true) => println!("Model {id} removed"),
                Ok(false) => {
                    eprintln!("Model {id} not found (or not owned by user '{user}')");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Failed to remove model: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Onboard {
            user,
            labels,
            model_dir,
            no_prompt,
        } => {
            let user = user.unwrap_or_else(current_user);
            if user == "root" {
                eprintln!(
                    "Refusing to onboard 'root'. Face auth for root is almost never what you\n\
                     want, and it is the exact mistake this command exists to prevent.\n\
                     Pass --user <name> explicitly if you really mean it."
                );
                std::process::exit(1);
            }
            if labels.is_empty() {
                eprintln!("No labels given — nothing to enroll.");
                std::process::exit(1);
            }

            println!("Onboarding face authentication for user '{user}'.\n");

            // 1. Models. setup::run is idempotent — it reports what is already present.
            println!("[1/3] ONNX models");
            setup::run(model_dir)?;
            println!();

            // 2. Captures. visaged self-heals within its restart interval once the
            //    models land, so connect AFTER setup rather than before.
            println!("[2/3] Face captures ({} to take)", labels.len());
            let proxy = connect_proxy().await?;
            let mut enrolled: Vec<(String, String)> = Vec::new();
            for (i, label) in labels.iter().enumerate() {
                if !no_prompt {
                    println!(
                        "\n  ({}/{}) '{}' — look at the camera{}, then press Enter.",
                        i + 1,
                        labels.len(),
                        label,
                        match label.as_str() {
                            "left" => " and turn your head slightly LEFT",
                            "right" => " and turn your head slightly RIGHT",
                            "glasses" => " wearing your glasses (skip with Ctrl-C if none)",
                            _ => " straight on",
                        }
                    );
                    let mut _l = String::new();
                    let _ = std::io::stdin().read_line(&mut _l);
                }
                match proxy.enroll(&user, label).await {
                    Ok(id) => {
                        println!("  ✓ {label}: {id}");
                        enrolled.push((label.clone(), id));
                    }
                    // One bad capture should not discard the ones that worked.
                    Err(e) => eprintln!("  ✗ {label}: {e}"),
                }
            }
            if enrolled.is_empty() {
                eprintln!("\nNo models enrolled — not going to claim this worked.");
                std::process::exit(1);
            }

            // 3. Prove it against the daemon rather than trusting the enroll return.
            //
            // Pause first. Without it this fires within a millisecond of the last
            // capture returning, while the user is still holding the pose they
            // struck to press Enter — which is the posture passive liveness is
            // most likely to reject, since it looks for landmark movement. The
            // result was an onboarding that enrolled perfectly and then failed
            // its own verification for a reason that had nothing to do with
            // enrollment quality.
            println!("\n[3/3] Verifying — look at the camera and behave normally.");
            if !no_prompt {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            match proxy.verify(&user).await {
                Ok(true) => {
                    println!("  ✓ recognised '{user}' from {} model(s)", enrolled.len());
                    println!(
                        "\nDone. Test the real path with:  sudo -k && sudo true\n\
                         Password remains available as a fallback on every PAM service."
                    );
                }
                Ok(false) => {
                    println!("  ✗ enrolled, but verification did not recognise you.");
                    println!(
                        "\n{} model(s) are stored, so enrollment itself worked — this is the\n\
                         verify step.",
                        enrolled.len()
                    );
                    // Deliberately does NOT assert a cause. The daemon collapses
                    // every failure into a plain non-match so that a caller cannot
                    // distinguish "wrong face" from "identity matched but liveness
                    // rejected" — that distinction would tell an attacker holding a
                    // photograph that the photograph was recognised. The CLI
                    // therefore genuinely does not know why this failed, and
                    // guessing sends people to the wrong subsystem.
                    println!(
                        "\nThe daemon does not report WHY a verify failed (distinguishing a\n\
                         wrong face from a rejected-but-matching one would leak information),\n\
                         so read its log for the actual reason:\n\
                         \n    journalctl -u visaged -n 20\n\
                         \nWhat that log will usually show, in rough order of likelihood:\n\
                         \n  * 'liveness rejected a face that matched identity' — you were\n\
                         recognised and the anti-spoof gate vetoed it. Tune it with\n\
                         services.visage.liveness.minDisplacement; its default is\n\
                         calibrated for a 640x480 30fps sensor and does not fit every\n\
                         camera.\n\
                         \n  * 'no face detected in any captured frame' — out of frame,\n\
                         looking away, or too dark.\n\
                         \n  * a low similarity score — genuinely not recognised. Re-run to\n\
                         add more angles."
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("  ✗ verification call failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Setup { model_dir } => {
            setup::run(model_dir)?;
        }
        Commands::Discover => {
            cmd_discover();
        }
        Commands::Status => {
            let proxy = connect_proxy().await?;
            match proxy.status().await {
                Ok(json) => {
                    let status: serde_json::Value = serde_json::from_str(&json)?;
                    println!("visaged status:");
                    println!(
                        "  version:    {}",
                        status["version"].as_str().unwrap_or("?")
                    );
                    println!("  camera:     {}", status["camera"].as_str().unwrap_or("?"));
                    if let Some(model_dir) = status.get("model_dir").and_then(|v| v.as_str()) {
                        println!("  model_dir:  {model_dir}");
                    }
                    if let Some(db_path) = status.get("db_path").and_then(|v| v.as_str()) {
                        println!("  db_path:    {db_path}");
                    }
                    println!(
                        "  models:     {}",
                        status["models_enrolled"].as_u64().unwrap_or(0)
                    );
                    println!(
                        "  threshold:  {:.2}",
                        status["similarity_threshold"].as_f64().unwrap_or(0.0)
                    );
                    if let Some(v) = status.get("verify_timeout_secs").and_then(|v| v.as_u64()) {
                        println!("  timeout:    {v}s");
                    }
                    if let Some(v) = status.get("frames_per_verify").and_then(|v| v.as_u64()) {
                        println!("  verify_n:   {v} frame(s)");
                    }
                    if let Some(v) = status.get("frames_per_enroll").and_then(|v| v.as_u64()) {
                        println!("  enroll_n:   {v} frame(s)");
                    }
                    if let Some(v) = status.get("emitter_enabled").and_then(|v| v.as_bool()) {
                        println!("  emitter:    {}", if v { "enabled" } else { "disabled" });
                    }
                    if let Some(v) = status.get("session_bus").and_then(|v| v.as_bool()) {
                        println!("  bus:        {}", if v { "session" } else { "system" });
                    }
                }
                Err(e) => {
                    eprintln!("visaged: not reachable — {e}");
                    eprintln!("Is visaged running?");
                    std::process::exit(1);
                }
            }
        }
        Commands::History { user, limit } => {
            let user = user.unwrap_or_else(current_user);
            let proxy = connect_proxy().await?;
            match proxy.history(&user, limit).await {
                Ok(json) => {
                    let attempts: Vec<serde_json::Value> = serde_json::from_str(&json)?;
                    if attempts.is_empty() {
                        println!("No authentication attempts recorded for user '{user}'");
                    } else {
                        println!("Recent logins for '{user}' (newest first):");
                        for a in &attempts {
                            let mark = if a["matched"].as_bool().unwrap_or(false) {
                                "✓"
                            } else {
                                "✗"
                            };
                            println!(
                                "  {} {}  sim={:.3}  reason={}  model={}",
                                mark,
                                a["created_at"].as_str().unwrap_or("?"),
                                a["similarity"].as_f64().unwrap_or(0.0),
                                a["reason"].as_str().unwrap_or("?"),
                                a["model_label"].as_str().unwrap_or("-"),
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to fetch history: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Test {
            device,
            frames,
            strobe,
            strobe_secs,
            label,
        } => {
            if strobe {
                run_strobe_measurement(&device, strobe_secs, &label)?;
            } else {
                run_camera_test(&device, frames)?;
            }
        }
    }

    Ok(())
}

fn cmd_discover() {
    use visage_hw::quirks::{get_driver, get_usb_ids, is_ipu6_camera, lookup_quirk};

    let mut entries: Vec<_> = std::fs::read_dir("/dev")
        .expect("cannot read /dev")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("video"))
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        println!("No /dev/video* devices found.");
        return;
    }

    let mut ipu6_detected = false;

    for entry in entries {
        let path = format!("/dev/{}", entry.file_name().to_string_lossy());
        let driver = get_driver(&path);

        if is_ipu6_camera(&path) {
            ipu6_detected = true;
            let driver_name = driver.as_deref().unwrap_or("intel_ipu6");
            println!("{path}  driver={driver_name}  [NOT SUPPORTED — IPU6 camera, not UVC]");
            continue;
        }

        match get_usb_ids(&path) {
            Some((vid, pid)) => {
                let driver_label = driver.as_deref().unwrap_or("unknown");
                let quirk_status = match lookup_quirk(vid, pid) {
                    Some(q) => format!("quirk: {} \u{2713}", q.device.name),
                    None => format!("no quirk (VID={vid:#06x} PID={pid:#06x})"),
                };
                println!(
                    "{path}  driver={driver_label}  VID={vid:#06x} PID={pid:#06x}  {quirk_status}"
                );
            }
            None => {
                let driver_label = driver.as_deref().unwrap_or("unknown");
                println!("{path}  driver={driver_label}  (not USB or no sysfs entry)");
            }
        }
    }

    if ipu6_detected {
        eprintln!();
        eprintln!("WARNING: Intel IPU6 camera(s) detected.");
        eprintln!("  IPU6 cameras use Intel's proprietary camera HAL and require libcamera,");
        eprintln!("  not the V4L2/UVC stack that Visage uses. They are not supported in v0.1.");
        eprintln!();
        eprintln!("  If your laptop has a separate USB IR camera, it may still appear above");
        eprintln!("  under a different /dev/videoN node with driver=uvcvideo.");
        eprintln!();
        eprintln!("  See: https://github.com/sovren-software/visage/blob/main/docs/hardware-compatibility.md");
    }
}

fn run_camera_test(device_path: &str, frame_count: usize) -> Result<()> {
    println!("Camera diagnostics");
    println!("==================");

    // List available devices
    let devices = visage_hw::Camera::list_devices();
    println!("\nDiscovered capture devices:");
    if devices.is_empty() {
        println!("  (none)");
    }
    for dev in &devices {
        println!("  {} — {} [{}]", dev.path, dev.name, dev.driver);
    }

    // Open target device
    println!("\nOpening {device_path}...");
    let camera = visage_hw::Camera::open(device_path)?;
    println!(
        "  Format: {:?} {}x{}",
        camera.fourcc, camera.width, camera.height
    );

    // Prepare output directory
    let out_dir = std::path::PathBuf::from("/tmp/visage-test");
    std::fs::create_dir_all(&out_dir)?;

    // Capture frames
    println!("\nCapturing {frame_count} frames...");
    let (captured_frames, dark_skipped) = camera.capture_frames(frame_count)?;
    println!(
        "  Captured: {} good, {} dark skipped",
        captured_frames.len(),
        dark_skipped
    );

    // Save as PGM and compute stats
    for (i, frame) in captured_frames.iter().enumerate() {
        let filename = out_dir.join(format!("frame-{:03}.pgm", i));
        save_pgm(&filename, &frame.data, frame.width, frame.height)?;
        println!(
            "  [{}] seq={} brightness={:.1} -> {}",
            i,
            frame.sequence,
            frame.avg_brightness(),
            filename.display()
        );
    }

    // Summary
    if !captured_frames.is_empty() {
        let avg: f32 = captured_frames
            .iter()
            .map(|f| f.avg_brightness())
            .sum::<f32>()
            / captured_frames.len() as f32;
        println!("\nAverage brightness: {avg:.1}");
    }

    println!("\nDone. Frames saved to {}", out_dir.display());
    Ok(())
}

/// Mean of the brightest 10% of pixels — a face proxy that needs no ONNX.
///
/// Whole-frame mean dilutes the signal badly: a face occupies roughly 10% of
/// this sensor's field (measured 24,745 of 230,400 pixels above 100 on a lit
/// frame), so the other 90% of background drags the average toward the room.
/// At close range under IR the face IS the brightest thing, so the top decile
/// approximates it without loading a detector into a camera diagnostic.
fn top_decile_mean(data: &[u8]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    // Counting sort over 256 buckets — O(n), no allocation of a sorted copy.
    let mut hist = [0u32; 256];
    for &b in data {
        hist[b as usize] += 1;
    }
    let want = (data.len() / 10).max(1);
    let mut taken = 0usize;
    let mut sum = 0f64;
    for value in (0..256).rev() {
        let n = hist[value] as usize;
        if n == 0 {
            continue;
        }
        let take = n.min(want - taken);
        sum += (value as f64) * (take as f64);
        taken += take;
        if taken >= want {
            break;
        }
    }
    (sum / taken as f64) as f32
}

/// Measure the IR emitter's lit/unlit strobe.
///
/// Why this exists: `liveness.minDisplacement` is set near zero on hardware
/// where the landmark-stability metric provably cannot separate a phone-screen
/// spoof from a live face (ADR 011). The emitter strobe is a candidate physical
/// discriminator — a live face reflects IR and should swing hard between the
/// halves, a self-emissive display should not — but that has never been
/// measured. This produces the number. It does NOT gate anything.
fn run_strobe_measurement(device_path: &str, secs: u64, label: &str) -> Result<()> {
    println!("IR strobe measurement");
    println!("=====================");
    println!("  label:  {label}");

    let camera = visage_hw::Camera::open(device_path)?;
    println!(
        "  device: {device_path}  {}x{}  {:?}",
        camera.width, camera.height, camera.fourcc
    );

    let out_dir = std::path::PathBuf::from("/tmp/visage-strobe").join(label);
    std::fs::create_dir_all(&out_dir)?;

    struct Sample {
        seq: u32,
        dark: bool,
        mean: f32,
        top: f32,
    }
    let mut samples: Vec<Sample> = Vec::new();

    println!("\nStreaming RAW frames for {secs}s — look at the camera and hold still.");
    let budget = std::time::Duration::from_secs(secs);
    let mut save_err: Option<String> = None;
    camera.stream_raw_frames_for(budget, |frame| {
        let kind = if frame.is_dark { "dark" } else { "lit" };
        let path = out_dir.join(format!("{kind}-{:05}.pgm", frame.sequence));
        if let Err(e) = save_pgm(&path, &frame.data, frame.width, frame.height) {
            if save_err.is_none() {
                save_err = Some(e.to_string());
            }
        }
        samples.push(Sample {
            seq: frame.sequence,
            dark: frame.is_dark,
            mean: frame.avg_brightness(),
            top: top_decile_mean(&frame.data),
        });
    })?;

    if let Some(e) = save_err {
        // Do not let a write failure masquerade as a measurement.
        anyhow::bail!("failed to save at least one frame: {e}");
    }
    if samples.is_empty() {
        anyhow::bail!("captured no frames at all — nothing to measure");
    }

    // Manifest: the real output. The PGMs are evidence; this is the data.
    let manifest = out_dir.join("frames.tsv");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&manifest)?;
        writeln!(f, "label\tseq\tis_dark\tmean\ttop_decile")?;
        for s in &samples {
            writeln!(
                f,
                "{label}\t{}\t{}\t{:.2}\t{:.2}",
                s.seq, s.dark, s.mean, s.top
            )?;
        }
    }

    let lit: Vec<&Sample> = samples.iter().filter(|s| !s.dark).collect();
    let dark: Vec<&Sample> = samples.iter().filter(|s| s.dark).collect();
    let avg = |v: &[&Sample], f: fn(&Sample) -> f32| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        v.iter().map(|s| f(*s)).sum::<f32>() / v.len() as f32
    };

    println!("\n  frames: {} total — {} lit, {} dark", samples.len(), lit.len(), dark.len());

    if lit.is_empty() || dark.is_empty() {
        println!(
            "\n  ⚠️  Only one half of the strobe was seen, so there is no delta to report.\n     \
             This sensor may not strobe, or the emitter may be held on. Nothing is wrong with \
             the measurement — there is simply nothing to measure here."
        );
        println!("\n  manifest: {}", manifest.display());
        return Ok(());
    }

    let lit_mean = avg(&lit, |s| s.mean);
    let dark_mean = avg(&dark, |s| s.mean);
    let lit_top = avg(&lit, |s| s.top);
    let dark_top = avg(&dark, |s| s.top);

    println!("\n  whole frame   lit {lit_mean:6.2}   dark {dark_mean:6.2}   delta {:6.2}", lit_mean - dark_mean);
    println!("  top decile    lit {lit_top:6.2}   dark {dark_top:6.2}   delta {:6.2}", lit_top - dark_top);

    // Adjacent pairs control for drift: a slow exposure ramp would move both
    // halves together and inflate a naive difference of group means.
    let mut pair_deltas: Vec<f32> = Vec::new();
    for w in samples.windows(2) {
        if w[0].dark != w[1].dark {
            let (l, d) = if w[0].dark { (&w[1], &w[0]) } else { (&w[0], &w[1]) };
            pair_deltas.push(l.top - d.top);
        }
    }
    if pair_deltas.is_empty() {
        println!("\n  no adjacent lit/dark pairs — the strobe is not alternating frame to frame");
    } else {
        let mean_pair = pair_deltas.iter().sum::<f32>() / pair_deltas.len() as f32;
        let mut sorted = pair_deltas.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        println!(
            "\n  ADJACENT PAIRS (top decile, drift-controlled)\n    \
             n={}  mean {:+.2}  min {:+.2}  max {:+.2}",
            pair_deltas.len(),
            mean_pair,
            sorted[0],
            sorted[sorted.len() - 1]
        );
    }

    println!("\n  frames:   {}", out_dir.display());
    println!("  manifest: {}", manifest.display());
    println!(
        "\n  ⚠️  One run is one condition. A delta here means nothing on its own — \
         \n      run again with --label phone-spoof holding a photo of yourself, and compare."
    );
    Ok(())
}

/// Write a grayscale image as PGM (Portable Gray Map) — no extra deps needed.
fn save_pgm(path: &std::path::Path, data: &[u8], width: u32, height: u32) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    write!(f, "P5\n{width} {height}\n255\n")?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The metric must track the BRIGHT REGION, not the frame.
    ///
    /// This is the whole reason it exists: a face is roughly a tenth of this
    /// sensor's field, so a whole-frame mean buries the signal under
    /// background. If the top decile moved with the background it would be a
    /// worse version of `avg_brightness` and the strobe measurement built on it
    /// would be meaningless.
    #[test]
    fn the_top_decile_tracks_the_bright_region_and_ignores_the_background() {
        // 10% at 200, 90% at 10 — a face-sized bright patch on a dark field.
        let mut frame = vec![10u8; 900];
        frame.extend(std::iter::repeat(200u8).take(100));

        let whole = frame.iter().map(|&b| b as f32).sum::<f32>() / frame.len() as f32;
        let top = top_decile_mean(&frame);

        assert!(
            (top - 200.0).abs() < 1.0,
            "top decile should be ~200 (the bright patch), got {top}"
        );
        assert!(
            whole < 40.0,
            "control: the whole-frame mean should be dragged down by background, got {whole}"
        );
        assert!(
            top > whole * 4.0,
            "the two metrics must actually differ, or this one adds nothing: \
             top {top} vs whole {whole}"
        );
    }

    /// Negative controls: degenerate inputs must not fabricate a reading.
    #[test]
    fn the_top_decile_is_honest_about_empty_and_uniform_frames() {
        assert_eq!(
            top_decile_mean(&[]),
            0.0,
            "an empty frame has no brightness to report"
        );

        // A uniform frame has no bright region — the answer is that value, not
        // something higher. A metric that reported more would invent contrast.
        let flat = vec![77u8; 1000];
        let top = top_decile_mean(&flat);
        assert!(
            (top - 77.0).abs() < 0.001,
            "a uniform frame's top decile is its own value, got {top}"
        );
    }
}

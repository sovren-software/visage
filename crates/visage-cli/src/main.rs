mod setup;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[zbus::proxy(
    interface = "org.freedesktop.Visage1",
    default_service = "org.freedesktop.Visage1",
    default_path = "/org/freedesktop/Visage1"
)]
trait Visage {
    async fn enroll(&self, user: &str, label: &str) -> zbus::fdo::Result<String>;
    async fn verify(&self, user: &str) -> zbus::fdo::Result<bool>;
    async fn status(&self) -> zbus::fdo::Result<String>;
    async fn list_models(&self, user: &str) -> zbus::fdo::Result<String>;
    async fn remove_model(&self, user: &str, model_id: &str) -> zbus::fdo::Result<bool>;
}

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

        /// User to enroll for (defaults to $USER)
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Verify your face against enrolled models
    Verify {
        /// User to verify as (defaults to $USER)
        #[arg(short, long)]
        user: Option<String>,
    },
    /// List enrolled face models
    List {
        /// User whose models to list (defaults to $USER)
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Remove an enrolled face model
    Remove {
        /// Model ID to remove
        id: String,

        /// User who owns the model (defaults to $USER)
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Download ONNX models required for face detection and recognition
    Setup {
        /// Model directory (default: /var/lib/visage/models when root, ~/.local/share/visage/models otherwise)
        #[arg(short, long)]
        model_dir: Option<String>,
    },
    /// Show daemon status
    Status,
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
    },
}

fn current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

async fn connect_proxy() -> Result<VisageProxy<'static>> {
    let use_session = std::env::var("VISAGE_SESSION_BUS").is_ok();
    let conn = if use_session {
        zbus::Connection::session().await
    } else {
        zbus::Connection::system().await
    }
    .map_err(|e| anyhow::anyhow!("failed to connect to D-Bus: {e}"))?;

    let proxy = VisageProxy::new(&conn).await.map_err(|e| {
        anyhow::anyhow!("failed to create proxy: {e} — is visaged running?")
    })?;
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
                    println!("  version:    {}", status["version"].as_str().unwrap_or("?"));
                    println!("  camera:     {}", status["camera"].as_str().unwrap_or("?"));
                    println!(
                        "  models:     {}",
                        status["models_enrolled"].as_u64().unwrap_or(0)
                    );
                    println!(
                        "  threshold:  {:.2}",
                        status["similarity_threshold"].as_f64().unwrap_or(0.0)
                    );
                }
                Err(e) => {
                    eprintln!("visaged: not reachable — {e}");
                    eprintln!("Is visaged running?");
                    std::process::exit(1);
                }
            }
        }
        Commands::Test { device, frames } => {
            run_camera_test(&device, frames)?;
        }
    }

    Ok(())
}

fn cmd_discover() {
    use visage_hw::quirks::{get_usb_ids, is_ipu6_camera, lookup_quirk, get_driver};

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
                println!("{path}  driver={driver_label}  VID={vid:#06x} PID={pid:#06x}  {quirk_status}");
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
        eprintln!("  See: https://github.com/Aigusta-Labs/visage/blob/main/docs/hardware-compatibility.md");
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
        let avg: f32 = captured_frames.iter().map(|f| f.avg_brightness()).sum::<f32>()
            / captured_frames.len() as f32;
        println!("\nAverage brightness: {avg:.1}");
    }

    println!("\nDone. Frames saved to {}", out_dir.display());
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

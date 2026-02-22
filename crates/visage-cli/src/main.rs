use anyhow::Result;
use clap::{Parser, Subcommand};

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
    },
    /// Verify your face against enrolled models
    Verify,
    /// List enrolled face models
    List,
    /// Remove an enrolled face model
    Remove {
        /// Model ID to remove
        id: String,
    },
    /// Show daemon status
    Status,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Enroll { label } => {
            println!("Enrolling face model with label: {label}");
            // TODO: Call visaged D-Bus Enroll()
            println!("Not yet implemented");
        }
        Commands::Verify => {
            println!("Verifying face...");
            // TODO: Call visaged D-Bus Verify()
            println!("Not yet implemented");
        }
        Commands::List => {
            // TODO: Call visaged D-Bus ListModels()
            println!("No models enrolled");
        }
        Commands::Remove { id } => {
            println!("Removing model: {id}");
            // TODO: Call visaged D-Bus RemoveModel()
            println!("Not yet implemented");
        }
        Commands::Status => {
            // TODO: Call visaged D-Bus Status()
            println!("visaged: not connected");
        }
        Commands::Test { device, frames } => {
            run_camera_test(&device, frames)?;
        }
    }

    Ok(())
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

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
    Test,
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
        Commands::Test => {
            println!("Running camera diagnostics...");
            // TODO: Direct camera test (bypass daemon for diagnostics)
            println!("Not yet implemented");
        }
    }

    Ok(())
}

//! `visage setup` — downloads ONNX models required for face detection and recognition.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use visage_models::{verify_file_sha256, ModelIntegrityError, MODELS};

// libc is a workspace dep (already used by pam-visage)
extern crate libc;

/// Determine the model directory.
///
/// When running as root (UID 0), defaults to `/var/lib/visage/models` (system-wide).
/// Otherwise defaults to `$XDG_DATA_HOME/visage/models` (~/.local/share/visage/models).
fn default_model_dir() -> PathBuf {
    if is_root() {
        PathBuf::from("/var/lib/visage/models")
    } else {
        let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{home}/.local/share")
        });
        PathBuf::from(data_home).join("visage/models")
    }
}

fn is_root() -> bool {
    // SAFETY: geteuid is always safe to call.
    unsafe { libc::geteuid() == 0 }
}

/// Download a single model file with progress output.
fn download_model(model: &visage_models::ModelFile, dest: &Path) -> Result<()> {
    let tmp_path = dest.with_extension("onnx.part");

    println!("  downloading {} ({})...", model.name, model.size_display);

    let resp = ureq::get(model.url)
        .call()
        .with_context(|| format!("failed to download {}", model.url))?;

    let content_length = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut reader = resp.into_body().into_reader();
    let mut file = fs::File::create(&tmp_path)
        .with_context(|| format!("failed to create {}", tmp_path.display()))?;

    let mut buf = [0u8; 65536];
    let mut total: u64 = 0;
    let mut last_pct: u64 = 0;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        total += n as u64;

        // Print progress every 10%
        if let Some(len) = content_length {
            let pct = (total * 100) / len;
            if pct / 10 > last_pct / 10 {
                print!("  {pct}%\r");
                io::stdout().flush().ok();
                last_pct = pct;
            }
        }
    }

    file.flush()?;
    drop(file);

    // Verify checksum
    print!("  verifying checksum... ");
    io::stdout().flush().ok();
    if let Err(err) = verify_file_sha256(model.name, &tmp_path, model.sha256) {
        fs::remove_file(&tmp_path).ok();
        bail!("{err}");
    }
    println!("ok");

    // Atomic rename
    fs::rename(&tmp_path, dest).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            tmp_path.display(),
            dest.display()
        )
    })?;

    Ok(())
}

/// Run the setup command: download and verify ONNX models.
pub fn run(model_dir: Option<String>) -> Result<()> {
    let dir = match model_dir {
        Some(d) => PathBuf::from(d),
        None => default_model_dir(),
    };

    println!("Model directory: {}", dir.display());

    // Root-only, like the daemon's state directory it usually sits in: on a
    // fresh RPM or source install `visage setup` can be the first thing to
    // create /var/lib/visage, and a 0755 directory there stays 0755.
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("failed to create directory {}", dir.display()))?;
    }

    let mut downloaded = 0;
    let mut skipped = 0;

    for model in MODELS {
        let dest = dir.join(model.name);
        if dest.exists() {
            // Verify existing file
            match verify_file_sha256(model.name, &dest, model.sha256) {
                Ok(()) => {
                    println!("  {} already present (checksum ok)", model.name);
                    skipped += 1;
                    continue;
                }
                Err(ModelIntegrityError::ChecksumMismatch { .. }) => {
                    println!(
                        "  {} exists but checksum differs — re-downloading",
                        model.name
                    );
                }
                Err(ModelIntegrityError::Open { .. } | ModelIntegrityError::Read { .. }) => {
                    println!("  {} exists but unreadable — re-downloading", model.name);
                }
                Err(ModelIntegrityError::MissingModel { .. }) => {}
            }
        }

        download_model(model, &dest)?;
        downloaded += 1;
    }

    println!();
    if downloaded > 0 {
        println!("Setup complete: {downloaded} model(s) downloaded, {skipped} already present.");
    } else {
        println!("All models already present. Nothing to download.");
    }

    Ok(())
}

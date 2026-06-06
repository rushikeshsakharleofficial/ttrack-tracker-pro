use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn playback_hash_path(central_dir: &PathBuf) -> PathBuf {
    central_dir.join(".playback.hash")
}

pub fn prune_hash_path(central_dir: &PathBuf) -> PathBuf {
    central_dir.join(".prune.hash")
}

pub fn set_password(hash_file: &Path) -> Result<()> {
    let pass = rpassword::prompt_password("New password (min 8 chars): ")
        .context("read password")?;
    if pass.len() < 8 {
        bail!("password must be at least 8 characters");
    }
    let confirm = rpassword::prompt_password("Confirm password: ")
        .context("read confirmation")?;
    if pass != confirm {
        bail!("passwords do not match");
    }
    let hash = bcrypt::hash(&pass, 12).context("bcrypt hash")?;
    if let Some(parent) = hash_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(hash_file, hash.as_bytes())?;
    Ok(())
}

pub fn verify_password(hash_file: &Path, prompt: &str, max_attempts: u32) -> Result<()> {
    if !hash_file.exists() {
        return Ok(());
    }
    let hash = fs::read_to_string(hash_file).context("read hash file")?;
    for attempt in 0..max_attempts {
        let pass = rpassword::prompt_password(prompt).context("read password")?;
        if bcrypt::verify(&pass, &hash).unwrap_or(false) {
            return Ok(());
        }
        let remaining = max_attempts - attempt - 1;
        if remaining > 0 {
            eprintln!("incorrect password, {remaining} attempt(s) remaining");
        }
    }
    bail!("too many incorrect password attempts")
}

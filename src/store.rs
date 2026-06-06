use crate::cast::{read_event, read_header};
use zeroize::Zeroizing;
use crate::config::Config;
use crate::crypto;
use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use nix::unistd::{Uid, User};
use std::fs::{self, File};
use std::io::{BufReader, Cursor};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn local_dir(cfg: &Config) -> PathBuf {
    cfg.local_dir.clone()
}

pub fn central_dir(cfg: &Config) -> PathBuf {
    cfg.central_dir.clone()
}

pub fn new_local_path(cfg: &Config) -> Result<PathBuf> {
    let dir = local_dir(cfg);
    fs::create_dir_all(&dir)?;
    Ok(dir.join(new_name()))
}

pub fn new_name() -> String {
    format!("{}-{}.cast", Local::now().format("%Y%m%dT%H%M%S%.9f"), std::process::id())
}

pub fn list_local(cfg: &Config) -> Result<()> {
    let dir = local_dir(cfg);
    if !dir.exists() {
        println!("no recordings yet (dir: {})", dir.display());
        return Ok(());
    }
    println!("STATUS   FILE                          STARTED              DURATION   COMMAND");
    for path in cast_files(&dir)? {
        print_session_row(&path, &path.file_name().unwrap().to_string_lossy())?;
    }
    Ok(())
}

pub fn users(cfg: &Config) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if !cfg.central_dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(&cfg.central_dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

pub fn user_sessions(cfg: &Config, user: &str) -> Result<Vec<PathBuf>> {
    cast_files(&cfg.central_dir.join(user))
}

pub fn find_central(cfg: &Config, id: &str) -> Result<(PathBuf, String)> {
    let id = id.trim_end_matches(".cast");
    for user in users(cfg)? {
        for path in user_sessions(cfg, &user)? {
            if path.file_stem().map(|s| s.to_string_lossy() == id).unwrap_or(false) {
                return Ok((path, user));
            }
        }
    }
    anyhow::bail!("session {id:?} not found in {}", cfg.central_dir.display())
}

pub fn read_plain_cast(path: &Path, cfg: &Config) -> Result<Vec<u8>> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if crypto::is_encrypted(&data) {
        let key = Zeroizing::new(fs::read(&cfg.key_file).with_context(|| format!("read key {}", cfg.key_file.display()))?);
        let mut out = Vec::new();
        crypto::decrypt_stream(Cursor::new(data), &mut out, &key)?;
        Ok(out)
    } else {
        Ok(data)
    }
}

pub fn cast_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let path = entry.path();
        if path.extension().map(|x| x == "cast").unwrap_or(false) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

pub fn ingest_local(cfg: &Config, key: &[u8]) -> Result<usize> {
    let local = local_dir(cfg);
    if !local.exists() {
        return Ok(0);
    }
    let uid = nix::unistd::getuid().as_raw();
    let user = User::from_uid(Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_else(|| uid.to_string());
    let dest_dir = cfg.central_dir.join(&user);
    fs::create_dir_all(&dest_dir)?;
    fs::set_permissions(&dest_dir, fs::Permissions::from_mode(0o700))?;
    let mut count = 0usize;
    for entry in fs::read_dir(&local)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let path = entry.path();
        if !path.extension().map(|x| x == "cast").unwrap_or(false) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let dest = dest_dir.join(&name);
        let data = fs::read(&path)?;
        if crypto::is_encrypted(&data) {
            fs::rename(&path, &dest)?;
        } else {
            let mut out = Vec::new();
            crypto::encrypt_stream(Cursor::new(&data), &mut out, key)?;
            fs::write(&dest, &out)?;
            fs::remove_file(&path)?;
        }
        count += 1;
    }
    Ok(count)
}

pub fn print_session_row(path: &Path, display_name: &str) -> Result<()> {
    let file = File::open(path)?;
    let mut br = BufReader::new(file);
    let header = read_header(&mut br).ok();
    let started = header
        .as_ref()
        .and_then(|h| Local.timestamp_opt(h.timestamp, 0).single())
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "?".to_string());
    let command = header.map(|h| h.command).unwrap_or_else(|| "(unreadable)".to_string());
    let dur = duration(path).unwrap_or_else(|_| "-".to_string());
    println!("{:<8} {:<29} {:<20} {:<10} {}", "SAVED", display_name, started, dur, command);
    Ok(())
}

pub fn duration(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut br = BufReader::new(file);
    let _ = read_header(&mut br)?;
    let mut last = 0.0;
    while let Some(ev) = read_event(&mut br)? {
        last = ev.0;
    }
    Ok(human_duration(last))
}

fn human_duration(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

use crate::cast::{read_event, read_header};
use crate::config::Config;
use crate::store;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{self, BufReader, Cursor, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub fn play(cfg: &Config, target: &str, speed: f64) -> Result<()> {
    let path = PathBuf::from(target);
    let data = if path.exists() {
        fs::read(&path)?
    } else {
        let (path, user) = store::find_central(cfg, target)?;
        eprintln!("--- session {target} (user {user}) ---");
        store::read_plain_cast(&path, cfg)?
    };
    play_bytes(&data, speed)
}

pub fn play_bytes(data: &[u8], speed: f64) -> Result<()> {
    let mut br = BufReader::new(Cursor::new(data));
    let _ = read_header(&mut br)?;
    let mut last = 0.0;
    while let Some(ev) = read_event(&mut br)? {
        let delay = ((ev.0 - last).max(0.0) / speed.max(0.01)).min(2.0);
        std::thread::sleep(std::time::Duration::from_secs_f64(delay));
        if ev.1 == "o" {
            print!("{}", ev.2);
            io::stdout().flush()?;
        }
        last = ev.0;
    }
    Ok(())
}

pub fn ls_all(cfg: &Config) -> Result<()> {
    let users = store::users(cfg)?;
    println!("USER                 SESSIONS");
    for user in users {
        let sessions = store::user_sessions(cfg, &user)?;
        println!("{:<20} {}", user, sessions.len());
    }
    Ok(())
}

pub fn ls_user(cfg: &Config, user: &str) -> Result<()> {
    println!("STATUS   SESSION                       STARTED              DURATION   COMMAND");
    for path in store::user_sessions(cfg, user)? {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        store::print_session_row(&path, &name)?;
    }
    Ok(())
}

pub fn tree(cfg: &Config) -> Result<()> {
    println!("{}", cfg.central_dir.display());
    let users = store::users(cfg)?;
    for (idx, user) in users.iter().enumerate() {
        let last_user = idx + 1 == users.len();
        let branch = if last_user { "└─" } else { "├─" };
        let indent = if last_user { "   " } else { "│  " };
        let sessions = store::user_sessions(cfg, user)?;
        println!("{branch} {user:<20}  ({} sessions)", sessions.len());
        for (sidx, path) in sessions.iter().enumerate() {
            let sbranch = if sidx + 1 == sessions.len() { "└─" } else { "├─" };
            let stem = path.file_stem().unwrap().to_string_lossy();
            println!("{indent}{sbranch} {stem}");
        }
    }
    Ok(())
}

pub fn export(cfg: &Config, id: &str, out: Option<PathBuf>) -> Result<()> {
    let (path, _) = store::find_central(cfg, id)?;
    let data = store::read_plain_cast(&path, cfg)?;
    if let Some(out) = out {
        fs::write(&out, data)?;
        eprintln!("exported plaintext cast to {}", out.display());
    } else {
        io::stdout().write_all(&data)?;
    }
    Ok(())
}

pub fn tail_static(cfg: &Config, id: &str, n: usize) -> Result<()> {
    let (path, _) = store::find_central(cfg, id)?;
    let data = store::read_plain_cast(&path, cfg)?;
    let mut br = BufReader::new(Cursor::new(data));
    let _ = read_header(&mut br)?;
    let mut lines: Vec<String> = Vec::new();
    while let Some(ev) = read_event(&mut br)? {
        if ev.1 == "o" {
            for line in ev.2.lines() {
                lines.push(line.to_string());
                if lines.len() > n {
                    lines.remove(0);
                }
            }
        }
    }
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

pub fn tail_live(cfg: &Config, id: &str) -> Result<()> {
    let mut stream = UnixStream::connect(&cfg.socket_path).context("ttrackd not reachable")?;
    stream.write_all(format!("TAIL {id}\n").as_bytes())?;
    let mut data = String::new();
    stream.read_to_string(&mut data)?;
    if data.starts_with("ERR ") {
        anyhow::bail!(data.trim().trim_start_matches("ERR ").to_string());
    }
    print!("{data}");
    Ok(())
}

pub fn search(cfg: &Config, pattern: &str, user_filter: Option<String>, insensitive: bool) -> Result<()> {
    let needle = if insensitive { pattern.to_lowercase() } else { pattern.to_string() };
    for user in store::users(cfg)? {
        if user_filter.as_ref().map(|u| u != &user).unwrap_or(false) {
            continue;
        }
        for path in store::user_sessions(cfg, &user)? {
            let data = store::read_plain_cast(&path, cfg)?;
            let mut br = BufReader::new(Cursor::new(data));
            let header = read_header(&mut br)?;
            let cmd_match = if insensitive { header.command.to_lowercase().contains(&needle) } else { header.command.contains(&needle) };
            let mut matched = cmd_match;
            while let Some(ev) = read_event(&mut br)? {
                let hay = if insensitive { ev.2.to_lowercase() } else { ev.2.clone() };
                if hay.contains(&needle) {
                    matched = true;
                    break;
                }
            }
            if matched {
                println!("{}/{}  {}", user, path.file_name().unwrap().to_string_lossy(), header.command);
            }
        }
    }
    Ok(())
}

pub fn prune(cfg: &Config, yes: bool) -> Result<()> {
    if !yes {
        anyhow::bail!("refusing to prune without --yes in this Rust pro scaffold");
    }
    let mut count = 0usize;
    for user in store::users(cfg)? {
        for path in store::user_sessions(cfg, &user)? {
            fs::remove_file(&path)?;
            count += 1;
        }
    }
    println!("pruned {count} session(s)");
    Ok(())
}

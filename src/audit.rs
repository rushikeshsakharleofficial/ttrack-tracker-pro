use crate::cast::{read_event, read_header};
use crate::config::Config;
use crate::store;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, BufReader, Cursor, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn play(cfg: &Config, target: &str, speed: f64) -> Result<()> {
    let hash_path = crate::auth::playback_hash_path(&cfg.central_dir);
    crate::auth::verify_password(&hash_path, "Playback password: ", 3)?;
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
        store::print_session_row(&path, &name, cfg)?;
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
    let mut buf = [0u8; 8192];
    let mut first = true;
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 { break; }
        if first {
            first = false;
            if buf[..n].starts_with(b"ERR ") {
                let msg = String::from_utf8_lossy(&buf[..n]);
                anyhow::bail!(msg.trim().trim_start_matches("ERR ").to_string());
            }
        }
        io::stdout().write_all(&buf[..n])?;
        io::stdout().flush()?;
    }
    Ok(())
}

fn scan_session(cfg: &Config, user: &str, path: &Path, needle: &str, insensitive: bool, from: Option<i64>, to: Option<i64>) -> Option<String> {
    let data = store::read_plain_cast(path, cfg).ok()?;
    let mut br = BufReader::new(Cursor::new(data));
    let header = read_header(&mut br).ok()?;
    if from.map(|f| header.timestamp < f).unwrap_or(false) { return None; }
    if to.map(|t| header.timestamp > t).unwrap_or(false) { return None; }
    let cmd_hay = if insensitive { header.command.to_lowercase() } else { header.command.clone() };
    let cmd_match = cmd_hay.contains(needle);
    let mut snippets: Vec<String> = Vec::new();
    let mut output_matched = false;
    while let Some(ev) = read_event(&mut br).ok()?.or(None) {
        if ev.1 != "o" { continue; }
        let hay = if insensitive { ev.2.to_lowercase() } else { ev.2.clone() };
        if hay.contains(needle) {
            output_matched = true;
            for line in ev.2.lines() {
                let line_hay = if insensitive { line.to_lowercase() } else { line.to_string() };
                if line_hay.contains(needle) && snippets.len() < 5 {
                    snippets.push(format!("  > {line}"));
                }
            }
        }
    }
    if !cmd_match && !output_matched { return None; }
    let fname = path.file_name().unwrap().to_string_lossy();
    let mut out = format!("{user}/{fname}  {}", header.command);
    for s in snippets {
        out.push('\n');
        out.push_str(&s);
    }
    Some(out)
}

pub fn search(cfg: &Config, pattern: &str, user_filter: Option<String>, insensitive: bool, from: Option<i64>, to: Option<i64>) -> Result<()> {
    let needle = if insensitive { pattern.to_lowercase() } else { pattern.to_string() };
    let mut jobs: Vec<(String, PathBuf)> = Vec::new();
    for user in store::users(cfg)? {
        if user_filter.as_ref().map(|u| u != &user).unwrap_or(false) { continue; }
        for path in store::user_sessions(cfg, &user)? {
            jobs.push((user.clone(), path));
        }
    }
    let n_workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(jobs.len().max(1));
    let results: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(vec![None; jobs.len()]));
    let (tx, rx) = std::sync::mpsc::channel::<(usize, String, PathBuf)>();
    for (i, (user, path)) in jobs.into_iter().enumerate() {
        tx.send((i, user, path)).ok();
    }
    drop(tx);
    let rx = Arc::new(Mutex::new(rx));
    let cfg_arc = Arc::new(cfg.clone());
    let needle_arc = Arc::new(needle);
    let mut handles = Vec::new();
    for _ in 0..n_workers {
        let rx = rx.clone();
        let results = results.clone();
        let cfg = cfg_arc.clone();
        let needle = needle_arc.clone();
        let h = std::thread::spawn(move || {
            loop {
                let job = rx.lock().unwrap().recv();
                match job {
                    Err(_) => break,
                    Ok((i, user, path)) => {
                        let r = scan_session(&cfg, &user, &path, &needle, insensitive, from, to);
                        results.lock().unwrap()[i] = r;
                    }
                }
            }
        });
        handles.push(h);
    }
    for h in handles { let _ = h.join(); }
    let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    for r in results.into_iter().flatten() {
        println!("{r}");
    }
    Ok(())
}

pub fn prune(cfg: &Config, yes: bool) -> Result<()> {
    let hash_path = crate::auth::prune_hash_path(&cfg.central_dir);
    crate::auth::verify_password(&hash_path, "Prune password: ", 3)?;
    if !yes {
        let mut total = 0usize;
        for user in store::users(cfg)? {
            total += store::user_sessions(cfg, &user)?.len();
        }
        eprint!("about to delete {total} session(s). type 'yes' to confirm: ");
        io::stdout().flush()?;
        let mut ans = String::new();
        io::stdin().read_line(&mut ans)?;
        if ans.trim() != "yes" {
            anyhow::bail!("prune aborted");
        }
    }
    let mut count = 0usize;
    for user in store::users(cfg)? {
        for path in store::user_sessions(cfg, &user)? {
            eprintln!("ttrack: prune: deleting {}", path.display());
            fs::remove_file(&path)?;
            count += 1;
        }
    }
    println!("pruned {count} session(s)");
    Ok(())
}

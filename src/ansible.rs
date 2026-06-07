use crate::config::Config;
use crate::crypto;
use crate::store;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use zeroize::Zeroizing;

const MAX_OUTPUT: usize = 8 * 1024;

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct RawEvent {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    playbook: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    started: f64,
    #[serde(default)]
    controller: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    play: String,
    #[serde(default)]
    module: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    rc: i64,
    #[serde(default)]
    t: f64,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    ok: u64,
    #[serde(default)]
    changed: u64,
    #[serde(default)]
    failed: u64,
    #[serde(default)]
    unreachable: u64,
    #[serde(default)]
    skipped: u64,
}

#[derive(Debug)]
struct TaskInfo {
    name: String,
    module: String,
    host: String,
    status: String,
    rc: i64,
    duration: f64,
    stdout_snippet: String,
}

#[derive(Debug)]
struct HostStats {
    ok: u64,
    changed: u64,
    failed: u64,
    unreachable: u64,
    skipped: u64,
}

#[derive(Debug)]
pub struct ParsedRun {
    pub run_id: String,
    pub playbook: String,
    pub user: String,
    pub controller: String,
    pub started: f64,
    pub plays: Vec<String>,
    tasks: Vec<TaskInfo>,
    host_stats: std::collections::HashMap<String, HostStats>,
}

pub fn parse_run(data: &[u8]) -> Result<ParsedRun> {
    let mut run = ParsedRun {
        run_id: String::new(),
        playbook: String::new(),
        user: String::new(),
        controller: String::new(),
        started: 0.0,
        plays: Vec::new(),
        tasks: Vec::new(),
        host_stats: std::collections::HashMap::new(),
    };
    for line in data.split(|&b| b == b'\n') {
        if line.is_empty() { continue; }
        let ev: RawEvent = match serde_json::from_slice(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match ev.kind.as_str() {
            "run" => {
                run.run_id = ev.id;
                run.playbook = ev.playbook;
                run.user = ev.user;
                run.controller = ev.controller;
                run.started = ev.started;
            }
            "play" => {
                if !ev.name.is_empty() {
                    run.plays.push(ev.name);
                }
            }
            "task" => {
                let snippet = if ev.stdout.len() > MAX_OUTPUT {
                    let mut end = MAX_OUTPUT;
                    while !ev.stdout.is_char_boundary(end) { end -= 1; }
                    format!("{}...", &ev.stdout[..end])
                } else {
                    ev.stdout.clone()
                };
                run.tasks.push(TaskInfo {
                    name: ev.name,
                    module: ev.module,
                    host: ev.host,
                    status: ev.status,
                    rc: ev.rc,
                    duration: ev.t,
                    stdout_snippet: snippet,
                });
            }
            "stats" => {
                run.host_stats.insert(ev.host.clone(), HostStats {
                    ok: ev.ok,
                    changed: ev.changed,
                    failed: ev.failed,
                    unreachable: ev.unreachable,
                    skipped: ev.skipped,
                });
            }
            _ => {}
        }
    }
    Ok(run)
}

pub fn valid_run_id(id: &str) -> bool {
    let len = id.len();
    if len < 5 || len > 64 { return false; }
    id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn ansible_local_dir(cfg: &Config) -> std::path::PathBuf {
    cfg.local_dir.join("ansible")
}

fn open_run(cfg: &Config, user: &str, id: &str) -> Result<ParsedRun> {
    let path = store::ansible_central_dir(cfg, user).join(format!("{id}.ajsonl"));
    let data = store::read_plain_cast(&path, cfg)?;
    parse_run(&data)
}

pub fn cmd_list(cfg: &Config, user: &str) -> Result<()> {
    let runs = store::ansible_runs(cfg, user).unwrap_or_default();
    if runs.is_empty() {
        println!("no ansible runs for {user}");
        return Ok(());
    }
    println!("{:<40} {}", "RUN ID", "PLAYBOOK");
    for id in &runs {
        let playbook = open_run(cfg, user, id)
            .map(|r| r.playbook)
            .unwrap_or_else(|_| "(unreadable)".to_string());
        println!("{:<40} {}", id, playbook);
    }
    Ok(())
}

pub fn cmd_show(cfg: &Config, user: &str, run_id: &str) -> Result<()> {
    if !valid_run_id(run_id) {
        anyhow::bail!("invalid run_id");
    }
    let run = open_run(cfg, user, run_id)?;
    println!("Run:        {}", run.run_id);
    println!("Playbook:   {}", run.playbook);
    println!("User:       {}", run.user);
    println!("Controller: {}", run.controller);
    if run.started > 0.0 {
        use chrono::{Local, TimeZone};
        let ts = run.started as i64;
        let dt = Local.timestamp_opt(ts, 0).single();
        if let Some(dt) = dt {
            println!("Started:    {}", dt.format("%Y-%m-%d %H:%M:%S"));
        }
    }
    if !run.plays.is_empty() {
        println!("Plays:      {}", run.plays.join(", "));
    }
    if !run.tasks.is_empty() {
        println!();
        println!("TASKS:");
        println!("{:<30} {:<20} {:<15} {:<10} {:<8} {}", "NAME", "HOST", "MODULE", "STATUS", "RC", "DURATION");
        for t in &run.tasks {
            println!("{:<30} {:<20} {:<15} {:<10} {:<8} {:.2}s",
                truncate(&t.name, 29), truncate(&t.host, 19), truncate(&t.module, 14),
                t.status, t.rc, t.duration);
            if !t.stdout_snippet.is_empty() {
                for line in t.stdout_snippet.lines().take(3) {
                    println!("  {}", line);
                }
            }
        }
    }
    if !run.host_stats.is_empty() {
        println!();
        println!("STATS:");
        println!("{:<20} {:>6} {:>8} {:>7} {:>12} {:>8}", "HOST", "OK", "CHANGED", "FAILED", "UNREACHABLE", "SKIPPED");
        let mut hosts: Vec<_> = run.host_stats.iter().collect();
        hosts.sort_by_key(|(h, _)| h.as_str());
        for (host, s) in hosts {
            println!("{:<20} {:>6} {:>8} {:>7} {:>12} {:>8}", host, s.ok, s.changed, s.failed, s.unreachable, s.skipped);
        }
    }
    Ok(())
}

pub fn cmd_incoming(cfg: &Config, user: &str) -> Result<()> {
    use regex::Regex;
    let ansible_tmp = Regex::new(r"ansible-tmp-(\d+\.\d+)-(\d+)-\d+").unwrap();
    let ansiball = Regex::new(r"AnsiballZ_(\w+)\.py").unwrap();

    let sessions = store::user_sessions(cfg, user)?;
    if sessions.is_empty() {
        println!("no sessions for {user}");
        return Ok(());
    }

    struct Group {
        tmpdir: String,
        modules: Vec<String>,
        first_ts: i64,
        last_ts: i64,
    }

    let mut groups: Vec<Group> = Vec::new();
    let gap_secs: i64 = 120;

    for path in &sessions {
        let data = store::read_plain_cast(path, cfg).unwrap_or_default();
        let mut br = std::io::BufReader::new(std::io::Cursor::new(data));
        let header = match crate::cast::read_header(&mut br) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let mut output = String::new();
        while let Ok(Some(ev)) = crate::cast::read_event(&mut br) {
            if ev.1 == "o" { output.push_str(&ev.2); }
        }
        let ts = header.timestamp;
        for cap in ansible_tmp.captures_iter(&output) {
            let tmpdir = cap[0].to_string();
            let mut modules_here = Vec::new();
            for mcap in ansiball.captures_iter(&output) {
                modules_here.push(mcap[1].to_string());
            }
            let placed = groups.iter_mut().find(|g| g.tmpdir == tmpdir && (ts - g.last_ts).abs() < gap_secs);
            if let Some(g) = placed {
                g.modules.extend(modules_here);
                g.modules.dedup();
                g.last_ts = g.last_ts.max(ts);
            } else {
                groups.push(Group { tmpdir: tmpdir.clone(), modules: modules_here, first_ts: ts, last_ts: ts });
            }
        }
    }

    if groups.is_empty() {
        println!("no ansible activity found in sessions for {user}");
        println!("hint: ttrack ansible-ingest imports runs from a callback plugin");
        return Ok(());
    }

    use chrono::{Local, TimeZone};
    println!("{:<40} {:<22} {}", "TMPDIR", "FIRST SEEN", "MODULES");
    for g in &groups {
        let dt = Local.timestamp_opt(g.first_ts, 0).single()
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "?".to_string());
        println!("{:<40} {:<22} {}", truncate(&g.tmpdir, 39), dt, g.modules.join(", "));
    }
    Ok(())
}

pub fn cmd_ingest(cfg: &Config) -> Result<()> {
    let mut data = Vec::new();
    io::stdin().read_to_end(&mut data).context("read stdin")?;

    let run_id = {
        let ev: RawEvent = data.split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .find_map(|l| serde_json::from_slice(l).ok().filter(|e: &RawEvent| e.kind == "run"))
            .ok_or_else(|| anyhow::anyhow!("no 'run' event found in input"))?;
        ev.id
    };

    if !valid_run_id(&run_id) {
        anyhow::bail!("invalid run_id in data: {:?}", run_id);
    }

    match UnixStream::connect(&cfg.socket_path) {
        Ok(mut stream) => {
            stream.write_all(format!("ANSIBLE {run_id}\n").as_bytes())?;
            stream.write_all(&data)?;
            stream.shutdown(Shutdown::Write)?;
            let mut resp = String::new();
            stream.read_to_string(&mut resp)?;
            if resp.trim().starts_with("ERR") {
                anyhow::bail!("daemon error: {}", resp.trim());
            }
            println!("ansible run {run_id} stored via daemon");
        }
        Err(_) => {
            let local_dir = ansible_local_dir(cfg);
            fs::create_dir_all(&local_dir)?;
            let path = local_dir.join(format!("{run_id}.ajsonl"));
            let key = Zeroizing::new(fs::read(&cfg.key_file).context("read key")?);
            let mut encrypted = Vec::new();
            crypto::encrypt_stream(Cursor::new(&data), &mut encrypted, &key)?;
            fs::write(&path, &encrypted)?;
            println!("ansible run {run_id} stored locally (daemon not reachable)");
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max.min(s.len());
        while !s.is_char_boundary(end) { end -= 1; }
        format!("{}…", &s[..end])
    }
}

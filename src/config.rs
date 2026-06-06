use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub socket_path: PathBuf,
    pub central_dir: PathBuf,
    pub key_file: PathBuf,
    pub local_dir: PathBuf,
    pub dial_timeout: Duration,
    pub eof_grace: Duration,
    pub log_file: PathBuf,
    pub session_cap: usize,
}

impl Default for Config {
    fn default() -> Self {
        let central_dir = PathBuf::from("/var/lib/ttrack");
        let local_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/ttrack");
        Self {
            socket_path: PathBuf::from("/run/ttrackd.sock"),
            key_file: central_dir.join(".ttrack.key"),
            central_dir,
            local_dir,
            dial_timeout: Duration::from_secs(1),
            eof_grace: Duration::from_millis(500),
            log_file: PathBuf::from("/var/log/ttrack/ttrack.log"),
            session_cap: 10,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let mut cfg = Config::default();
        let path = std::env::var("TTRACK_CONFIG").unwrap_or_else(|_| "/etc/ttrack/ttrack.conf".to_string());
        if let Ok(content) = fs::read_to_string(path) {
            let kv = parse_kv(&content);
            apply_kv(&mut cfg, &kv);
        }
        apply_env(&mut cfg);
        if cfg.key_file.is_relative() {
            cfg.key_file = cfg.central_dir.join(&cfg.key_file);
        }
        cfg
    }
}

fn parse_kv(content: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let mut v = v.trim().to_string();
        if let Some((before, _)) = v.split_once(" #") {
            v = before.trim().to_string();
        }
        out.insert(k.trim().to_string(), v);
    }
    out
}

fn apply_kv(cfg: &mut Config, kv: &HashMap<String, String>) {
    if let Some(v) = kv.get("socket_path").filter(|v| !v.is_empty()) {
        cfg.socket_path = PathBuf::from(v);
    }
    if let Some(v) = kv.get("central_dir").filter(|v| !v.is_empty()) {
        cfg.central_dir = PathBuf::from(v);
    }
    if let Some(v) = kv.get("key_file") {
        cfg.key_file = PathBuf::from(v);
    }
    if let Some(v) = kv.get("log_file") {
        cfg.log_file = PathBuf::from(v);
    }
    if let Some(v) = kv.get("dial_timeout_sec").and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0) {
        cfg.dial_timeout = Duration::from_secs_f64(v);
    }
    if let Some(v) = kv.get("eof_grace_ms").and_then(|v| v.parse::<u64>().ok()) {
        cfg.eof_grace = Duration::from_millis(v);
    }
    if let Some(v) = kv.get("session_cap").and_then(|v| v.parse::<usize>().ok()).filter(|v| *v > 0) {
        cfg.session_cap = v;
    }
}

fn apply_env(cfg: &mut Config) {
    if let Ok(v) = std::env::var("TTRACKD_SOCK") {
        if !v.is_empty() { cfg.socket_path = PathBuf::from(v); }
    }
    if let Ok(v) = std::env::var("TTRACK_CENTRAL_DIR") {
        if !v.is_empty() { cfg.central_dir = PathBuf::from(v); }
    }
    if let Ok(v) = std::env::var("TTRACK_KEY_FILE") {
        if !v.is_empty() { cfg.key_file = PathBuf::from(v); }
    }
    if let Ok(v) = std::env::var("TTRACK_DIR") {
        if !v.is_empty() { cfg.local_dir = PathBuf::from(v); }
    }
    if let Ok(v) = std::env::var("TTRACK_SESSION_CAP") {
        if let Ok(n) = v.parse::<usize>() {
            if n > 0 { cfg.session_cap = n; }
        }
    }
}

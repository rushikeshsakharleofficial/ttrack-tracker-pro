use crate::config::Config;
use crate::crypto;
use anyhow::{Context, Result};
use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use nix::unistd::{Uid, User};
use zeroize::Zeroizing;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone)]
pub struct Registry {
    live: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { live: Arc::new(Mutex::new(HashMap::new())) }
    }
}

pub fn run(cfg: Config) -> Result<()> {
    fs::create_dir_all(&cfg.central_dir).context("create central dir")?;
    fs::set_permissions(&cfg.central_dir, fs::Permissions::from_mode(0o700))?;
    let key: Zeroizing<Vec<u8>> = Zeroizing::new(crypto::ensure_key(&cfg.key_file)?);

    if cfg.socket_path.exists() {
        let _ = fs::remove_file(&cfg.socket_path);
    }
    let listener = UnixListener::bind(&cfg.socket_path).with_context(|| format!("listen {}", cfg.socket_path.display()))?;
    fs::set_permissions(&cfg.socket_path, fs::Permissions::from_mode(0o666))?;
    eprintln!("ttrackd: listening on {}, storing in {}", cfg.socket_path.display(), cfg.central_dir.display());

    let registry = Registry::new();
    for conn in listener.incoming() {
        match conn {
            Ok(conn) => {
                let cfg = cfg.clone();
                let key = key.clone();
                let registry = registry.clone();
                thread::spawn(move || {
                    if let Err(e) = handle(conn, cfg, key, registry) {
                        eprintln!("ttrackd: {e:#}");
                    }
                });
            }
            Err(e) => eprintln!("ttrackd: accept: {e}"),
        }
    }
    Ok(())
}

fn handle(mut conn: UnixStream, cfg: Config, key: Zeroizing<Vec<u8>>, registry: Registry) -> Result<()> {
    let cred = getsockopt(&conn, PeerCredentials).context("peer credentials")?;
    let uid = cred.uid();
    let pid = cred.pid();

    let mut br = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    br.read_line(&mut line)?;
    let line = line.trim();

    if line == "REC" {
        if registry.live.lock().unwrap().len() >= cfg.session_cap {
            use std::io::Write;
            conn.write_all(b"ERR session cap reached\n")?;
            return Ok(());
        }
        handle_rec(br, cfg, key, registry, uid, pid)
    } else if let Some(id) = line.strip_prefix("TAIL ") {
        if uid != 0 {
            use std::io::Write;
            conn.write_all(b"ERR tail requires root\n")?;
            return Ok(());
        }
        handle_tail(conn, cfg, registry, id.trim())
    } else {
        use std::io::Write;
        conn.write_all(b"ERR unknown command\n")?;
        Ok(())
    }
}

fn handle_rec<R: Read>(mut input: R, cfg: Config, key: Zeroizing<Vec<u8>>, registry: Registry, uid: u32, pid: i32) -> Result<()> {
    let user = username_for_uid(uid).unwrap_or_else(|| uid.to_string());
    let dir = cfg.central_dir.join(&user);
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let id = format!("{}-{}", chrono::Local::now().format("%Y%m%dT%H%M%S%.9f"), pid);
    let path = dir.join(format!("{id}.cast"));
    let out = File::create(&path).with_context(|| format!("create {}", path.display()))?;
    registry.live.lock().unwrap().insert(id.clone(), path.clone());
    eprintln!("ttrackd: session started user={user} id={id}");
    let result = crypto::encrypt_stream(&mut input, out, &key);
    registry.live.lock().unwrap().remove(&id);
    eprintln!("ttrackd: session closed user={user} id={id}");
    result
}

fn handle_tail(mut conn: UnixStream, cfg: Config, registry: Registry, id: &str) -> Result<()> {
    let id = id.trim_end_matches(".cast");
    let maybe_live = registry.live.lock().unwrap().get(id).cloned();
    let Some(path) = maybe_live else {
        use std::io::Write;
        conn.write_all(format!("ERR no active session {id}\n").as_bytes())?;
        return Ok(());
    };
    let data = crate::store::read_plain_cast(&path, &cfg)?;
    use std::io::Write;
    conn.write_all(&data)?;
    Ok(())
}

fn username_for_uid(uid: u32) -> Option<String> {
    User::from_uid(Uid::from_raw(uid)).ok()?.map(|u| u.name)
}

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
struct LiveSession {
    path: PathBuf,
    uid: u32,
    subscribers: Vec<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

struct TeeReader<R: Read> {
    inner: R,
    registry: Registry,
    id: String,
}

impl<R: Read> Read for TeeReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            let chunk = buf[..n].to_vec();
            let mut live = self.registry.live.lock().unwrap();
            if let Some(session) = live.get_mut(&self.id) {
                session.subscribers.retain(|tx| {
                    match tx.try_send(chunk.clone()) {
                        Ok(_) => true,
                        Err(std::sync::mpsc::TrySendError::Full(_)) => true,
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
                    }
                });
            }
        }
        Ok(n)
    }
}

#[derive(Clone)]
pub struct Registry {
    live: Arc<Mutex<HashMap<String, LiveSession>>>,
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
    match crate::store::ingest_local(&cfg, &key) {
        Ok(n) if n > 0 => eprintln!("ttrackd: ingested {n} local session(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("ttrackd: ingest warning: {e:#}"),
    }

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
        let uid_count = registry.live.lock().unwrap().values().filter(|s| s.uid == uid).count();
        if uid_count >= cfg.session_cap {
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
    } else if let Some(id) = line.strip_prefix("ANSIBLE ") {
        handle_ansible(br, conn, cfg, key, uid, id.trim())
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
    registry.live.lock().unwrap().insert(id.clone(), LiveSession { path: path.clone(), uid, subscribers: Vec::new() });
    eprintln!("ttrackd: session started user={user} id={id}");
    let mut tee = TeeReader { inner: &mut input, registry: registry.clone(), id: id.clone() };
    let result = crypto::encrypt_stream(&mut tee, out, &key);
    registry.live.lock().unwrap().remove(&id);
    eprintln!("ttrackd: session closed user={user} id={id}");
    result
}

fn handle_tail(mut conn: UnixStream, cfg: Config, registry: Registry, id: &str) -> Result<()> {
    use std::io::Write;
    let id = id.trim_end_matches(".cast");
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
    let maybe_path = {
        let mut live = registry.live.lock().unwrap();
        match live.get_mut(id) {
            None => {
                conn.write_all(format!("ERR no active session {id}\n").as_bytes())?;
                return Ok(());
            }
            Some(session) => {
                session.subscribers.push(tx);
                session.path.clone()
            }
        }
    };
    let data = crate::store::read_plain_cast(&maybe_path, &cfg)?;
    conn.write_all(&data)?;
    while let Ok(chunk) = rx.recv() {
        conn.write_all(&chunk)?;
    }
    Ok(())
}

fn handle_ansible<R: Read>(mut input: R, mut conn: UnixStream, cfg: Config, key: Zeroizing<Vec<u8>>, uid: u32, run_id: &str) -> Result<()> {
    use std::io::Write;
    if !crate::ansible::valid_run_id(run_id) {
        conn.write_all(b"ERR invalid run_id\n")?;
        return Ok(());
    }
    let user = username_for_uid(uid).unwrap_or_else(|| uid.to_string());
    let dir = cfg.central_dir.join(&user).join("ansible");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{run_id}.ajsonl"));
    let out = File::create(&path).with_context(|| format!("create {}", path.display()))?;
    crypto::encrypt_stream(&mut input, out, &key)?;
    eprintln!("ttrackd: ansible run stored user={user} id={run_id}");
    conn.write_all(b"OK\n")?;
    Ok(())
}

fn username_for_uid(uid: u32) -> Option<String> {
    User::from_uid(Uid::from_raw(uid)).ok()?.map(|u| u.name)
}

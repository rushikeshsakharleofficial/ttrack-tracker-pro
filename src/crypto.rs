use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use rand::RngCore;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

pub const MAGIC: &[u8] = b"TTEC1\n";
pub const KEY_SIZE: usize = 32;
const MAX_FRAME: usize = 1024 * 1024;

pub fn ensure_key(path: &Path) -> Result<Vec<u8>> {
    if path.exists() {
        let key = fs::read(path).with_context(|| format!("read key {}", path.display()))?;
        if key.len() != KEY_SIZE {
            bail!("key {} has wrong size {}", path.display(), key.len());
        }
        return Ok(key);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut key = vec![0u8; KEY_SIZE];
    rand::thread_rng().fill_bytes(&mut key);
    fs::write(path, &key).with_context(|| format!("write key {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

pub fn encrypt_stream<R: Read, W: Write>(mut r: R, mut w: W, key: &[u8]) -> Result<()> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid key"))?;
    w.write_all(MAGIC)?;
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher.encrypt(nonce, &buf[..n]).map_err(|_| anyhow!("encrypt frame"))?;
        let frame_len = nonce_bytes.len() + ct.len();
        w.write_u32::<BigEndian>(frame_len as u32)?;
        w.write_all(&nonce_bytes)?;
        w.write_all(&ct)?;
    }
    w.flush()?;
    Ok(())
}

pub fn decrypt_stream<R: Read, W: Write>(mut r: R, mut w: W, key: &[u8]) -> Result<()> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid key"))?;
    let mut magic = vec![0u8; MAGIC.len()];
    r.read_exact(&mut magic)?;
    if magic != MAGIC {
        w.write_all(&magic)?;
        io::copy(&mut r, &mut w)?;
        return Ok(());
    }

    loop {
        let len = match r.read_u32::<BigEndian>() {
            Ok(v) => v as usize,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if !(12..=MAX_FRAME).contains(&len) {
            bail!("corrupt encrypted stream: invalid frame length {len}");
        }
        let mut frame = vec![0u8; len];
        if let Err(e) = r.read_exact(&mut frame) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                break;
            }
            return Err(e.into());
        }
        let nonce = Nonce::from_slice(&frame[..12]);
        let pt = cipher
            .decrypt(nonce, &frame[12..])
            .map_err(|_| anyhow!("corrupt encrypted stream: authentication failed"))?;
        w.write_all(&pt)?;
    }
    w.flush()?;
    Ok(())
}

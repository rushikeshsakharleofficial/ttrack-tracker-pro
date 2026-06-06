use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use rand::RngCore;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

/// v2 format: TTEC2\n | prefix(4) | (frame_len_u32be | ciphertext)+
/// Nonce for frame k: prefix[0..4] || k.to_be_bytes()[0..8]
pub const MAGIC: &[u8] = b"TTEC2\n";
/// v1 legacy: TTEC1\n | (frame_len_u32be | nonce(12) | ciphertext)+
pub const MAGIC_V1: &[u8] = b"TTEC1\n";
pub const KEY_SIZE: usize = 32;
const CHUNK_SIZE: usize = 32 * 1024;
const MAX_CT_LEN: usize = CHUNK_SIZE + 16;
const MAX_FRAME_V1: usize = 1024 * 1024;

pub fn is_encrypted(data: &[u8]) -> bool {
    data.starts_with(MAGIC) || data.starts_with(MAGIC_V1)
}

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

    let mut prefix = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut prefix);
    w.write_all(&prefix)?;

    let mut counter: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[..4].copy_from_slice(&prefix);
        nonce_bytes[4..].copy_from_slice(&counter.to_be_bytes());
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher.encrypt(nonce, &buf[..n]).map_err(|_| anyhow!("encrypt frame"))?;
        w.write_u32::<BigEndian>(ct.len() as u32)?;
        w.write_all(&ct)?;
        counter += 1;
    }
    w.flush()?;
    Ok(())
}

pub fn decrypt_stream<R: Read, W: Write>(mut r: R, mut w: W, key: &[u8]) -> Result<()> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("invalid key"))?;
    let mut magic = vec![0u8; MAGIC.len()];
    r.read_exact(&mut magic)?;

    if magic.as_slice() == MAGIC_V1 {
        return decrypt_v1_frames(r, w, &cipher);
    }
    if magic.as_slice() != MAGIC {
        eprintln!(
            "ttrack: warning: file is not encrypted (no magic header); treating as plaintext"
        );
        w.write_all(&magic)?;
        io::copy(&mut r, &mut w)?;
        return Ok(());
    }

    let mut prefix = [0u8; 4];
    r.read_exact(&mut prefix)?;
    decrypt_v2_frames(r, w, &cipher, prefix)
}

fn decrypt_v2_frames<R: Read, W: Write>(
    mut r: R,
    mut w: W,
    cipher: &Aes256Gcm,
    prefix: [u8; 4],
) -> Result<()> {
    let mut counter: u64 = 0;
    loop {
        let ct_len = match r.read_u32::<BigEndian>() {
            Ok(v) => v as usize,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if ct_len < 16 || ct_len > MAX_CT_LEN {
            bail!("corrupt encrypted stream: invalid frame length {ct_len}");
        }
        let mut ct = vec![0u8; ct_len];
        if let Err(e) = r.read_exact(&mut ct) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                break;
            }
            return Err(e.into());
        }
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[..4].copy_from_slice(&prefix);
        nonce_bytes[4..].copy_from_slice(&counter.to_be_bytes());
        let nonce = Nonce::from_slice(&nonce_bytes);
        let pt = cipher
            .decrypt(nonce, ct.as_slice())
            .map_err(|_| anyhow!("corrupt encrypted stream: authentication failed at frame {counter}"))?;
        w.write_all(&pt)?;
        counter += 1;
    }
    w.flush()?;
    Ok(())
}

fn decrypt_v1_frames<R: Read, W: Write>(mut r: R, mut w: W, cipher: &Aes256Gcm) -> Result<()> {
    loop {
        let len = match r.read_u32::<BigEndian>() {
            Ok(v) => v as usize,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if !(12..=MAX_FRAME_V1).contains(&len) {
            bail!("corrupt encrypted stream (v1): invalid frame length {len}");
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
            .map_err(|_| anyhow!("corrupt encrypted stream (v1): authentication failed"))?;
        w.write_all(&pt)?;
    }
    w.flush()?;
    Ok(())
}

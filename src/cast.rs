use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub version: u8,
    pub width: u16,
    pub height: u16,
    pub timestamp: i64,
    pub command: String,
    #[serde(default)]
    pub env: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event(pub f64, pub String, pub String);

pub struct CastWriter<W: Write> {
    inner: W,
}

impl<W: Write> CastWriter<W> {
    pub fn new(mut inner: W, command: String) -> Result<Self> {
        let header = Header {
            version: 2,
            width: 80,
            height: 24,
            timestamp: now_ts(),
            command,
            env: serde_json::json!({
                "SHELL": std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
                "TERM": std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string())
            }),
        };
        serde_json::to_writer(&mut inner, &header).context("write cast header")?;
        inner.write_all(b"\n")?;
        Ok(Self { inner })
    }

    pub fn write_output(&mut self, elapsed: f64, data: &[u8]) -> Result<()> {
        let data = String::from_utf8_lossy(data).to_string();
        let ev = Event(elapsed, "o".to_string(), data);
        serde_json::to_writer(&mut self.inner, &ev).context("write cast event")?;
        self.inner.write_all(b"\n")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.inner.flush().context("flush cast writer")
    }
}

pub fn read_header<R: BufRead>(r: &mut R) -> Result<Header> {
    let mut line = String::new();
    r.read_line(&mut line).context("read cast header")?;
    serde_json::from_str(line.trim_end()).context("parse cast header")
}

pub fn read_event<R: BufRead>(r: &mut R) -> Result<Option<Event>> {
    let mut line = String::new();
    let n = r.read_line(&mut line).context("read cast event")?;
    if n == 0 {
        return Ok(None);
    }
    let ev = serde_json::from_str(line.trim_end()).context("parse cast event")?;
    Ok(Some(ev))
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

use crate::cast::CastWriter;
use crate::config::Config;
use crate::store;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

pub struct RecordOptions {
    pub out: Option<PathBuf>,
    pub quiet: bool,
    pub cmd: Vec<String>,
}

pub fn run(cfg: &Config, opts: RecordOptions) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let cmd = if opts.cmd.is_empty() { vec![shell] } else { opts.cmd };
    let command_string = cmd.join(" ");

    let (sink, dest): (Box<dyn Write + Send>, String) = open_sink(cfg, opts.out.as_ref())?;
    if !opts.quiet {
        eprintln!("ttrack: recording to {dest} — type 'exit' or Ctrl-D to stop");
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .context("open pty")?;

    let mut builder = CommandBuilder::new(&cmd[0]);
    for arg in &cmd[1..] {
        builder.arg(arg);
    }
    let mut child = pair.slave.spawn_command(builder).context("spawn command")?;
    drop(pair.slave);

    let mut writer = pair.master.take_writer().context("pty writer")?;
    let mut reader = pair.master.try_clone_reader().context("pty reader")?;

    let stdin_thread = thread::spawn(move || {
        let mut stdin = io::stdin();
        let _ = io::copy(&mut stdin, &mut writer);
        thread::sleep(Duration::from_millis(200));
    });

    let stdout_thread = thread::spawn(move || -> Result<()> {
        let mut cw = CastWriter::new(sink, command_string)?;
        let start = Instant::now();
        let mut buf = [0u8; 8192];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            io::stdout().write_all(&buf[..n])?;
            io::stdout().flush()?;
            cw.write_output(start.elapsed().as_secs_f64(), &buf[..n])?;
            cw.flush()?;
        }
        Ok(())
    });

    let _ = child.wait();
    let _ = stdin_thread.join();
    let _ = stdout_thread.join().map_err(|_| anyhow::anyhow!("recording thread panicked"))??;

    if !opts.quiet {
        eprintln!("\nttrack: session saved to {dest}");
    }
    Ok(())
}

fn open_sink(cfg: &Config, out: Option<&PathBuf>) -> Result<(Box<dyn Write + Send>, String)> {
    if let Some(path) = out {
        let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        return Ok((Box::new(file), path.display().to_string()));
    }

    if let Ok(mut stream) = UnixStream::connect(&cfg.socket_path) {
        if stream.write_all(b"REC\n").is_ok() {
            return Ok((Box::new(stream), "ttrackd (central)".to_string()));
        }
    }

    let path = store::new_local_path(cfg)?;
    let file = File::create(&path).with_context(|| format!("create {}", path.display()))?;
    Ok((Box::new(file), path.display().to_string()))
}

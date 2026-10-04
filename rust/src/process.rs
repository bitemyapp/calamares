// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixed-argument subprocesses. Call only from a worker, never the GUI thread.
use anyhow::{Context, Result, ensure};
use std::os::unix::process::CommandExt;
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env(
            "PATH",
            option_env!("CALAMARES_TOOL_PATH")
                .unwrap_or("/run/current-system/sw/bin:/usr/bin:/bin"),
        )
        .env("HOME", "/root")
        .env("LC_ALL", "C.UTF-8")
        .env("NIX_REMOTE", "daemon");
    command
}

pub fn output(program: &str, args: &[&str], seconds: u64) -> Result<String> {
    let mut child = command(program)
        .args(args)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Starting {program}"))?;
    // Drain concurrently and cap retained output. Nix builds can produce many MB.
    fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
        thread::spawn(move || {
            let mut retained = Vec::new();
            let mut buf = [0; 8192];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                retained.extend_from_slice(&buf[..n]);
                if retained.len() > 131072 {
                    retained.drain(..retained.len() - 65536);
                }
            }
            retained
        })
    }
    let stdout = drain(child.stdout.take().unwrap());
    let stderr = drain(child.stderr.take().unwrap());
    let end = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= end {
            // A group created for this child only; do not leave partitioning or
            // installation descendants running after reporting a timeout.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            anyhow::bail!("{program} timed out; installation is incomplete");
        }
        thread::sleep(Duration::from_millis(100));
    };
    let out = String::from_utf8_lossy(&stdout.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
    ensure!(
        status.success(),
        "{program} failed ({status}): {err}\n{out}"
    );
    Ok(out)
}

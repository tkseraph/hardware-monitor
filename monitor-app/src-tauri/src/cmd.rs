//! System command execution adapter (R5/A04).
//!
//! Every external command goes through `run` which enforces a timeout and an
//! output-size cap, so a hung or runaway system tool can no longer stall a
//! collection pass indefinitely. Commands are invoked by absolute, trusted
//! paths with a scrubbed environment and a C locale so output parsing is
//! deterministic.
//!
//! Implementation: a reader thread drains stdout while the main thread polls
//! `try_wait` against a deadline. A blocking `read` on the pipe can otherwise
//! stall past the timeout when a child produces no output and never exits; the
//! reader-thread + channel pattern bounds the wait to the timeout.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Default per-command timeout. sysctl/ioreg/diskutil answer in well under a
/// second; 5s is generous headroom without being unbounded.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on captured stdout to bound memory from a malformed/verbose tool.
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

pub struct CmdOutput {
    pub status_success: bool,
    pub stdout: Vec<u8>,
    /// True if the command was killed for exceeding the timeout.
    pub timed_out: bool,
}

impl CmdOutput {
    /// Borrow stdout as a lossy string if the command succeeded and did not
    /// time out; otherwise an Err describing the failure.
    pub fn to_result(&self) -> Result<String, String> {
        if self.timed_out {
            return Err("command timed out".to_string());
        }
        if !self.status_success {
            return Err("command exited non-zero".to_string());
        }
        Ok(String::from_utf8_lossy(&self.stdout).into_owned())
    }
}

enum ReadMsg {
    Chunk(Vec<u8>),
    Eof,
    Error,
}

/// Run `program` (absolute path) with `args`, enforcing `timeout` and an
/// output cap. Returns Err on spawn failure; a timed-out command is killed and
/// reported with `timed_out = true` (status_success false).
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, String> {
    run_with_env(program, args, timeout, &[])
}

/// As `run`, with explicit extra env pairs (kept minimal). The child always
/// gets a scrubbed base environment plus LC_ALL=C for stable parsing.
pub fn run_with_env(
    program: &str,
    args: &[&str],
    timeout: Duration,
    extra_env: &[(&str, &str)],
) -> Result<CmdOutput, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C")
        .env("HOME", "/var/empty")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child: Child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {}", program, e))?;
    let mut stdout_pipe = child.stdout.take().ok_or("no stdout pipe")?;

    // Drain stdout on a helper thread; chunks arrive over a channel so the main
    // thread can poll for exit against the deadline without a blocking read.
    let (tx, rx) = mpsc::channel::<ReadMsg>();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stdout_pipe.read(&mut chunk) {
                Ok(0) => {
                    let _ = tx.send(ReadMsg::Eof);
                    break;
                }
                Ok(n) => {
                    if tx.send(ReadMsg::Chunk(chunk[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tx.send(ReadMsg::Error);
                    break;
                }
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::new();
    let mut reader_eof = false;

    let status = loop {
        // Drain available chunks.
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ReadMsg::Chunk(c) => {
                    if buf.len() < MAX_OUTPUT_BYTES {
                        let room = MAX_OUTPUT_BYTES - buf.len();
                        buf.extend_from_slice(&c[..c.len().min(room)]);
                    }
                }
                ReadMsg::Eof | ReadMsg::Error => reader_eof = true,
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(CmdOutput {
                        status_success: false,
                        stdout: buf,
                        timed_out: true,
                    });
                }
                // If the reader hit EOF, the child closed stdout; still wait for
                // exit but with a short sleep to avoid a busy spin.
                let nap = if reader_eof {
                    Duration::from_millis(5)
                } else {
                    // Wait briefly for either a chunk or the deadline.
                    match rx.recv_timeout(Duration::from_millis(5)) {
                        Ok(ReadMsg::Chunk(c)) => {
                            if buf.len() < MAX_OUTPUT_BYTES {
                                let room = MAX_OUTPUT_BYTES - buf.len();
                                buf.extend_from_slice(&c[..c.len().min(room)]);
                            }
                        }
                        Ok(ReadMsg::Eof) | Ok(ReadMsg::Error) => reader_eof = true,
                        Err(_) => {}
                    }
                    Duration::ZERO
                };
                if nap > Duration::ZERO {
                    std::thread::sleep(nap);
                }
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait {}: {}", program, e));
            }
        }
    };

    // Reap trailing output after exit.
    while let Ok(msg) = rx.try_recv() {
        if let ReadMsg::Chunk(c) = msg {
            if buf.len() < MAX_OUTPUT_BYTES {
                let room = MAX_OUTPUT_BYTES - buf.len();
                buf.extend_from_slice(&c[..c.len().min(room)]);
            }
        }
    }

    let status = status.expect("loop only exits with Some(status)");
    Ok(CmdOutput {
        status_success: status.success(),
        stdout: buf,
        timed_out: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_fast_command() {
        let out = run("/bin/echo", &["hello"], Duration::from_secs(2)).unwrap();
        assert!(out.status_success);
        assert!(!out.timed_out);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[test]
    fn times_out_hung_command() {
        // `sleep 30` produces no output and never exits within the timeout; the
        // reader-thread pattern must still bound the wait to ~the timeout.
        let start = Instant::now();
        let out = run("/bin/sleep", &["30"], Duration::from_millis(300)).unwrap();
        assert!(out.timed_out, "hung command must be reported as timed out");
        assert!(!out.status_success);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timeout must bound the wait, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn spawn_failure_is_error_not_hang() {
        let r = run("/nonexistent/path/tool", &[], Duration::from_millis(500));
        assert!(r.is_err());
    }

    #[test]
    fn scrubbed_env_and_c_locale() {
        let out = run("/usr/bin/env", &[], Duration::from_secs(2)).unwrap();
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("LC_ALL=C"), "locale pinned: {}", s);
        assert!(!s.contains("HOME=/Users/"), "HOME scrubbed");
    }

    #[test]
    fn large_output_is_capped() {
        // Emit ~1 MiB of 'x'; must be capped at MAX_OUTPUT_BYTES, not grow forever.
        let out = run("/usr/bin/yes", &[], Duration::from_millis(150)).unwrap();
        // yes never exits on its own → timed out, and output bounded.
        assert!(out.timed_out);
        assert!(out.stdout.len() <= MAX_OUTPUT_BYTES);
    }
}

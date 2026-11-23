//! Runs one job's verified [`Plan`]: its own binary, no shell, no stdin, low priority, killed when
//! it outlives its timeout or grows past its memory ceiling.
use otherwise_link::background::{ENV_JOB, ENV_LANG, Plan, Report};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What the helper keeps of a job's stdout (the report is the last line).
pub const MAX_STDOUT: usize = 64 << 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Killed {
    Timeout,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    pub code: Option<i32>,
    pub killed: Option<Killed>,
    pub report: Option<Report>,
    pub elapsed: Duration,
    /// Largest resident size seen, in bytes.
    pub peak_rss: u64,
}

impl Outcome {
    /// A short reason for the menu (`exit 2`, `timeout`, `memory`).
    pub fn error(&self) -> Option<String> {
        if self.ok {
            return None;
        }
        Some(match (self.killed, self.code) {
            (Some(Killed::Timeout), _) => "timeout".into(),
            (Some(Killed::Memory), _) => "memory".into(),
            (None, Some(c)) => format!("exit {c}"),
            (None, None) => "signal".into(),
        })
    }
}

/// Starts and waits. `rss(pid)` reads the child's resident size (injected for tests).
pub fn run(plan: &Plan, lang: &str, rss: &dyn Fn(u32) -> Option<u64>) -> std::io::Result<Outcome> {
    let mut cmd = Command::new(&plan.program);
    cmd.args(&plan.args).env(ENV_JOB, &plan.job.id).env(ENV_LANG, lang).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if let Some(dir) = plan.program.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpriority is async-signal-safe; nothing else runs between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                // its own process group, so a kill reaches whatever it started too
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    let mut out = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        if let Some(o) = out.as_mut() {
            while let Ok(n) = o.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_STDOUT {
                    // keep the tail: the report is the last line
                    let cut = buf.len() - MAX_STDOUT;
                    buf.drain(..cut);
                }
            }
        }
        let _ = tx.send(buf);
    });
    let limit = plan.job.memory_mb() * 1024 * 1024;
    let timeout = plan.job.timeout();
    let mut killed = None;
    let mut peak = 0;
    let mut last_probe = Instant::now() - Duration::from_secs(1);
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if start.elapsed() > timeout {
            killed = Some(Killed::Timeout);
            kill_group(&mut child);
            break child.wait().ok();
        }
        if last_probe.elapsed() >= Duration::from_millis(500) {
            last_probe = Instant::now();
            if let Some(r) = rss(child.id()) {
                peak = peak.max(r);
                if r > limit {
                    killed = Some(Killed::Memory);
                    kill_group(&mut child);
                    break child.wait().ok();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // a leftover grandchild may still hold the pipe: don't wait for it
    let stdout = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let code = status.and_then(|s| s.code());
    let ok = killed.is_none() && status.is_some_and(|s| s.success());
    Ok(Outcome { ok, code, killed, report: Report::from_output(&String::from_utf8_lossy(&stdout)), elapsed: start.elapsed(), peak_rss: peak })
}

fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: the child leads its own process group (setpgid in pre_exec).
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

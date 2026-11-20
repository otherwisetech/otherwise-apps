//! Launching installed apps, revealing them, and asking a running copy to quit before an update.
use crate::catalog::AppEntry;
use crate::platform::Os;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Windows: no console window flashing up for helper processes.
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn quiet(mut cmd: Command) -> std::io::Result<std::process::Child> {
    no_window(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

/// Starts the installed app, detached.
pub fn open_app(os: Os, install: &Path, exe: &Path) -> std::io::Result<()> {
    match os {
        Os::Macos => {
            let mut c = Command::new("open");
            c.arg(install);
            quiet(c).map(|_| ())
        }
        Os::Windows | Os::Linux => {
            let mut c = Command::new(exe);
            c.current_dir(install);
            quiet(c).map(|_| ())
        }
    }
}

/// Finder / Explorer / the file manager, with the install selected where possible.
pub fn reveal(os: Os, path: &Path) -> std::io::Result<()> {
    let c = match os {
        Os::Macos => {
            let mut c = Command::new("open");
            c.arg("-R").arg(path);
            c
        }
        Os::Windows => {
            let mut c = Command::new("explorer");
            c.arg(format!("/select,{}", path.display()));
            c
        }
        Os::Linux => {
            let mut c = Command::new("xdg-open");
            c.arg(path.parent().unwrap_or(path));
            c
        }
    };
    quiet(c).map(|_| ())
}

/// Process ids whose executable lives under `install`.
pub fn running(os: Os, e: &AppEntry, install: &Path) -> Vec<u32> {
    match os {
        Os::Macos => ps_under(install),
        Os::Linux => proc_under(install),
        Os::Windows => tasklist(&format!("{}.exe", e.install_name)),
    }
}

/// `ps -axo pid=,comm=`: on macOS `comm` is the full executable path.
fn ps_under(install: &Path) -> Vec<u32> {
    let Ok(out) = Command::new("ps").args(["-axo", "pid=,comm="]).output() else { return vec![] };
    let me = std::process::id();
    parse_ps(&String::from_utf8_lossy(&out.stdout), install).into_iter().filter(|p| *p != me).collect()
}

pub fn parse_ps(text: &str, install: &Path) -> Vec<u32> {
    let prefix = format!("{}/", install.to_string_lossy().trim_end_matches('/'));
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, cmd) = l.split_once(char::is_whitespace)?;
            cmd.trim_start().starts_with(&prefix).then(|| pid.parse().ok()).flatten()
        })
        .collect()
}

fn proc_under(install: &Path) -> Vec<u32> {
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    rd.flatten()
        .filter_map(|d| d.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != me)
        .filter(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe: PathBuf| exe.starts_with(install)))
        .collect()
}

fn tasklist(image: &str) -> Vec<u32> {
    let mut c = Command::new("tasklist");
    c.args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {image}")]);
    no_window(&mut c);
    let Ok(out) = c.output() else { return vec![] };
    let me = std::process::id();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut cols = l.split("\",\"");
            let name = cols.next()?.trim_start_matches('"');
            let pid = cols.next()?.parse().ok()?;
            name.eq_ignore_ascii_case(image).then_some(pid)
        })
        .filter(|p| *p != me)
        .collect()
}

/// Politely asks the app to quit (macOS: an Apple Event, so it can save; elsewhere SIGTERM /
/// WM_CLOSE) and waits up to `wait` for it to go. Returns whether it is gone.
pub fn quit(os: Os, e: &AppEntry, install: &Path, wait: Duration) -> bool {
    let pids = running(os, e, install);
    if pids.is_empty() {
        return true;
    }
    match os {
        Os::Macos => {
            let script = format!("tell application id \"{}\" to quit", e.bundle_id.replace(['"', '\\'], ""));
            let _ = Command::new("osascript").args(["-e", &script]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        Os::Linux => {
            for p in &pids {
                let _ = Command::new("kill").args(["-TERM", &p.to_string()]).status();
            }
        }
        Os::Windows => {
            for p in &pids {
                let mut c = Command::new("taskkill");
                c.args(["/PID", &p.to_string()]);
                no_window(&mut c);
                let _ = c.status();
            }
        }
    }
    let start = Instant::now();
    while start.elapsed() < wait {
        if running(os, e, install).is_empty() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// Self-update: start the new copy after we have exited.
pub fn relaunch_after_exit(os: Os, install: &Path, exe: &Path) -> std::io::Result<()> {
    match os {
        Os::Macos => {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg("sleep 1; exec open -n \"$0\"").arg(install);
            quiet(c).map(|_| ())
        }
        Os::Linux => {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg("sleep 1; exec \"$0\"").arg(exe).current_dir(install);
            quiet(c).map(|_| ())
        }
        Os::Windows => open_app(os, install, exe),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_parsing() {
        let text = "  101 /Applications/rama.app/Contents/MacOS/rama\n  102 /Applications/rama-old.app/Contents/MacOS/rama\n 7 /sbin/launchd\n";
        assert_eq!(parse_ps(text, Path::new("/Applications/rama.app")), vec![101]);
    }
}

//! Otherwise Helper: one small background process for every Otherwise desktop app. It runs the
//! background jobs installed apps declare (otherwise-link `background`, SPEC §11), shows their
//! notifications and a menu-bar status, and starts at login. No GPUI and no other app's code: it
//! only starts each app's own verified binary.
//!
//! - [`engine`]: installed apps with jobs, running the due one, draining notifications.
//! - [`sched`]: what is due and why the rest waits (pause, battery saver, memory, offline).
//! - [`runner`]: one run with its timeout and memory ceiling.
//! - [`state`]: last runs and shown notifications (this computer's folder, never the data root).
//! - [`menu`]: the menu as data. [`sys`]: memory pressure, network, processes.
//! - `mac` (in the binary): NSStatusItem, UNUserNotificationCenter, SMAppService.
pub mod engine;
pub mod menu;
pub mod runner;
pub mod sched;
pub mod state;
pub mod sys;

use chrono::{Local, Utc};
use engine::{Engine, Step};
use otherwise_link::Root;
use otherwise_link::apps::{AppSpec, Locations, Os};
use otherwise_link::background::Notification;
use otherwise_link::settings::HubSettings;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const BUNDLE_ID: &str = "io.github.otherwisetech.otherwise-helper";
pub const APP_NAME: &str = "Otherwise Helper";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How often installed apps are scanned again.
pub const RESCAN_EVERY: Duration = Duration::from_secs(60);
/// The longest the worker sleeps (conditions change: power, network).
pub const MAX_SLEEP: Duration = Duration::from_secs(30);

/// The launcher's settings folder (its `settings.json` holds the language), as otherwise-apps-core
/// `layout::config_dir`.
pub fn hub_config_dir() -> PathBuf {
    let home = otherwise_link::paths::home();
    let env = |v: &str| std::env::var_os(v).filter(|s| !s.is_empty()).map(PathBuf::from);
    match Os::current() {
        Os::Macos => home.join("Library/Application Support/otherwise-apps"),
        Os::Windows => env("APPDATA").unwrap_or_else(|| home.join("AppData/Roaming")).join("otherwise-apps"),
        Os::Linux => env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("otherwise-apps"),
    }
}

/// The launcher's language, else the system's, else Spanish (as the launcher).
pub fn language() -> &'static str {
    let saved = std::fs::read_to_string(hub_config_dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("language").and_then(|l| l.as_str()).map(str::to_string));
    if let Some(l) = saved.as_deref().and_then(i18n::match_system) {
        return l;
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(l) = std::env::var(var).ok().as_deref().and_then(i18n::match_system) {
            return l;
        }
    }
    if cfg!(target_os = "macos")
        && let Ok(o) = std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output()
        && let Some(l) = i18n::match_system(String::from_utf8_lossy(&o.stdout).trim())
    {
        return l;
    }
    i18n::DEFAULT
}

/// Inode and modification time of a file: tells an update (the bundle replaced) apart.
pub fn exe_identity(p: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let ino = m.len();
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((ino, mtime))
}

/// Was the executable at `path` replaced since we started (`started`)? A missing file is not
/// (yet) a replacement: the old bundle went to the Trash and the new one isn't there yet.
pub fn replaced(path: &Path, started: (u64, i64)) -> bool {
    exe_identity(path).is_some_and(|now| now != started)
}

/// Starts the newly installed helper a moment after this process is gone (it holds the
/// single-instance lock): the bundle through LaunchServices on macOS, else the binary.
pub fn relaunch() {
    let Ok(exe) = std::env::current_exe() else { return };
    #[cfg(unix)]
    {
        let app = exe.ancestors().nth(3).filter(|a| cfg!(target_os = "macos") && a.extension().is_some_and(|x| x == "app"));
        let (script, arg) = match app {
            Some(a) => ("sleep 3; exec /usr/bin/open -g \"$0\"", a.to_path_buf()), // i18n-ok: shell
            None => ("sleep 3; exec \"$0\"", exe.clone()), // i18n-ok: shell
        };
        let _ = std::process::Command::new("/bin/sh").arg("-c").arg(script).arg(arg).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    #[cfg(not(unix))]
    let _ = exe;
}

/// Requests from the menu to the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Pause,
    Resume,
    Rescan,
}

/// What the worker thread and the menu share.
#[derive(Default)]
pub struct Shared {
    /// (generation, items): the menu rebuilds when the generation moves.
    pub menu: Mutex<(u64, Vec<menu::Item>)>,
    /// Notifications to show, taken by the UI thread.
    pub notes: Mutex<Vec<Notification>>,
    pub commands: Mutex<Vec<Command>>,
    pub wake: Condvar,
    pub quit: AtomicBool,
    /// Something is running (for the icon).
    pub busy: AtomicBool,
    pub paused: AtomicBool,
    /// The worker stopped because a newer helper was installed.
    pub updated: AtomicBool,
}

impl Shared {
    pub fn send(&self, c: Command) {
        self.commands.lock().unwrap().push(c);
        self.wake.notify_all();
    }
}

/// The real apps and folders of this computer.
pub fn system_engine() -> Engine {
    Engine::new(Locations::system(), otherwise_link::apps::known_specs(), state::dir())
}

/// The worker loop: scan, run what is due (one at a time), drain notifications, publish the
/// menu. `low_power` is read by the platform shell; `updated()` says our own executable was
/// replaced (Otherwise Apps updated): the loop then ends so the new helper can start.
pub fn work(shared: Arc<Shared>, mut engine: Engine, low_power: &dyn Fn() -> bool, updated: &dyn Fn() -> bool) {
    let mut last_scan = Instant::now();
    let mut lang = language();
    i18n::set_locale(lang);
    loop {
        if shared.quit.load(Ordering::Relaxed) {
            return;
        }
        for c in std::mem::take(&mut *shared.commands.lock().unwrap()) {
            match c {
                Command::Pause => engine.pause(Some(Utc::now() + chrono::Duration::hours(1))),
                Command::Resume => engine.pause(None),
                Command::Rescan => last_scan = Instant::now() - RESCAN_EVERY,
            }
        }
        if last_scan.elapsed() >= RESCAN_EVERY {
            if updated() {
                shared.updated.store(true, Ordering::Relaxed);
                shared.quit.store(true, Ordering::Relaxed);
                return;
            }
            engine.rescan();
            lang = language();
            i18n::set_locale(lang);
            last_scan = Instant::now();
        }
        let root = Root::system();
        let settings = HubSettings::load(&root).background;
        engine.refresh_leases(&root, &settings);
        let c = sched::Conditions { now: Utc::now(), low_power: low_power(), pressure: sys::memory_pressure(), online: sys::online() };
        let open = |r: &otherwise_link::background::Registered| r.installed.executable.as_deref().is_some_and(sys::running);
        publish(&shared, &engine, &settings, &c, &open, lang);
        shared.busy.store(true, Ordering::Relaxed);
        let step = engine.step(&settings, &c, &open, &mut |plan| runner::run(plan, lang, &sys::rss), &Local);
        shared.busy.store(false, Ordering::Relaxed);
        let notes = engine.notifications(&root, &settings, Utc::now());
        if !notes.is_empty() {
            shared.notes.lock().unwrap().extend(notes);
        }
        shared.paused.store(engine.state.paused(Utc::now()), Ordering::Relaxed);
        if !matches!(step, Step::Nothing) {
            // ran something (or dropped a vanished app): look again right away
            continue;
        }
        let c = sched::Conditions { now: Utc::now(), ..c };
        let d = engine.decisions(&settings, &c, &open, &Local);
        publish(&shared, &engine, &settings, &c, &open, lang);
        let sleep = sched::next_wake(&d).and_then(|t| (t - c.now).to_std().ok()).unwrap_or(MAX_SLEEP).clamp(Duration::from_secs(1), MAX_SLEEP);
        let guard = shared.commands.lock().unwrap();
        if guard.is_empty() && !shared.quit.load(Ordering::Relaxed) {
            let _ = shared.wake.wait_timeout(guard, sleep);
        }
    }
}

fn publish(shared: &Shared, engine: &Engine, settings: &otherwise_link::settings::BackgroundSettings, c: &sched::Conditions, open: &dyn Fn(&otherwise_link::background::Registered) -> bool, lang: &str) {
    let d = engine.decisions(settings, c, open, &Local);
    let items = menu::build(&engine.apps, &d, &engine.state, lang, c.now, &Local);
    let mut m = shared.menu.lock().unwrap();
    if m.1 != items {
        m.0 += 1;
        m.1 = items;
    }
}

/// Holds `~/.otherwise/helper/instance.lock` for the life of the process; None if another helper
/// has it.
pub fn single_instance() -> Option<std::fs::File> {
    let dir = state::dir();
    std::fs::create_dir_all(&dir).ok()?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("instance.lock")).ok()?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: a valid open fd.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return None;
        }
    }
    Some(f)
}

/// Spec of the launcher (the helper lives in its bundle).
pub fn hub_spec() -> AppSpec {
    otherwise_link::apps::known(otherwise_link::apps::HUB_ID).map(|k| k.spec()).expect("the launcher is in the table")
}

/// Shows a notification without the macOS framework (Linux `notify-send`, Windows balloon).
pub fn post_plain(title: &str, body: &str) {
    use std::process::{Command as Cmd, Stdio};
    if cfg!(target_os = "linux") {
        let _ = Cmd::new("notify-send").args(["--app-name", APP_NAME, title, body]).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    } else if cfg!(windows) {
        let ps = |s: &str| s.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; $n.ShowBalloonTip(8000, '{}', '{}', 'Info'); Start-Sleep -Seconds 9; $n.Dispose()", // i18n-ok: PowerShell
            ps(title),
            ps(body)
        );
        let _ = Cmd::new("powershell").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn notices_its_executable_being_replaced() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("otherwise-helper");
        std::fs::write(&exe, "old").unwrap();
        let started = super::exe_identity(&exe).unwrap();
        assert!(!super::replaced(&exe, started));
        // the bundle goes to the Trash: not yet a replacement
        std::fs::rename(&exe, t.path().join("trashed")).unwrap();
        assert!(!super::replaced(&exe, started));
        // the new bundle lands at the same path
        std::fs::write(&exe, "new").unwrap();
        assert!(super::replaced(&exe, started));
    }
}

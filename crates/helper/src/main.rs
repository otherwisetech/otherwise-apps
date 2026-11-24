//! `otherwise-helper`: the menu-bar process (no arguments), plus a few commands for Otherwise Apps
//! and for checking it by hand:
//!
//! - `login on|off|status`: start at login (macOS `SMAppService`, the helper's own bundle)
//! - `status`: installed apps with jobs and their last runs, as JSON
//! - `server run|status|init|share|unshare|member|tier|doctor`: the server mode
//! - `once`: run whatever is due now, one job, and exit (no menu, notifications printed as JSON)
#[cfg(target_os = "macos")]
mod mac;

use otherwise_helper::{Shared, engine::Step, state, system_engine};
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    i18n::set_locale(otherwise_helper::language());
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => println!("{} {}", otherwise_helper::APP_NAME, otherwise_helper::VERSION), // i18n-ok: name + version
        Some("login") => std::process::exit(login(args.get(1).map(String::as_str).unwrap_or("status"))),
        Some("status") => status(),
        Some("once") => once(),
        Some("server") => std::process::exit(otherwise_helper::server_mode::main(&args[1..])),
        // launchd / LaunchServices may pass -psn_… or nothing
        _ => run(),
    }
}

fn run() {
    let Some(_lock) = otherwise_helper::single_instance() else {
        eprintln!("{}", i18n::t!("helper.cli.running"));
        return;
    };
    let shared = Arc::new(Shared::default());
    let engine = system_engine();
    let exe = std::env::current_exe().ok();
    let started = exe.as_deref().and_then(otherwise_helper::exe_identity);
    let worker = {
        let shared = shared.clone();
        let exe = exe.clone();
        let updated = move || matches!((&exe, started), (Some(p), Some(s)) if otherwise_helper::replaced(p, s));
        std::thread::Builder::new().name("jobs".into()).spawn(move || otherwise_helper::work(shared, engine, &low_power, &updated))
    };
    #[cfg(target_os = "macos")]
    mac::run(shared.clone());
    #[cfg(not(target_os = "macos"))]
    {
        // Windows / Linux: no tray icon yet; the worker runs and notifications are plain
        let shared2 = shared.clone();
        std::thread::spawn(move || {
            loop {
                for n in std::mem::take(&mut *shared2.notes.lock().unwrap()) {
                    otherwise_helper::post_plain(&n.title, n.body.as_deref().unwrap_or(""));
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
        if let Ok(w) = worker {
            let _ = w.join();
        }
        if shared.updated.load(std::sync::atomic::Ordering::Relaxed) {
            otherwise_helper::relaunch();
        }
        return;
    }
    #[allow(unreachable_code)]
    if let Ok(w) = worker {
        shared.quit.store(true, std::sync::atomic::Ordering::Relaxed);
        shared.wake.notify_all();
        let _ = w.join();
    }
    if shared.updated.load(std::sync::atomic::Ordering::Relaxed) {
        otherwise_helper::relaunch();
    }
}


fn low_power() -> bool {
    #[cfg(target_os = "macos")]
    return mac::low_power();
    #[allow(unreachable_code)]
    false
}

fn login(what: &str) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let r = match what {
            "on" => mac::set_login(true),
            "off" => mac::set_login(false),
            _ => Ok(mac::login_status()),
        };
        let (key, code) = match r {
            Ok(mac::Login::On) => ("helper.cli.login_on", 0),
            Ok(mac::Login::Off) => ("helper.cli.login_off", if what == "on" { 1 } else { 0 }),
            Ok(mac::Login::NeedsApproval) => ("helper.cli.login_approval", 0),
            Ok(mac::Login::Unsupported) => ("helper.cli.login_unsupported", 2),
            Err(e) => {
                eprintln!("{}", i18n::t!("helper.cli.login_failed", error = e));
                return 1;
            }
        };
        let state = match key {
            "helper.cli.login_on" => "on",
            "helper.cli.login_approval" => "approval",
            "helper.cli.login_unsupported" => "unsupported",
            _ => "off",
        };
        // first line machine-readable for Otherwise Apps, then the sentence
        println!("{state}"); // i18n-ok: state token
        println!("{}", i18n::t!(key));
        return code;
    }
    #[allow(unreachable_code)]
    {
        let _ = what;
        println!("unsupported"); // i18n-ok: state token
        println!("{}", i18n::t!("helper.cli.login_unsupported"));
        2
    }
}

fn status() {
    let e = system_engine();
    let apps: Vec<serde_json::Value> = e
        .apps
        .iter()
        .map(|r| {
            let jobs: Vec<serde_json::Value> = r
                .manifest
                .jobs
                .iter()
                .map(|j| {
                    let js = e.state.job(&r.spec.id, &j.id);
                    serde_json::json!({ "id": j.id, "every": j.every, "daily": j.daily, "heavy": j.heavy, "network": j.network,
                        "lastStart": js.last_start, "lastOk": js.last_ok, "failures": js.failures, "status": js.status, "idle": js.idle, "error": js.error })
                })
                .collect();
            serde_json::json!({ "app": r.spec.id, "install": r.install, "version": r.installed.version, "jobs": jobs })
        })
        .collect();
    let out = serde_json::json!({ "version": otherwise_helper::VERSION, "stateDir": state::dir(), "pausedUntil": e.state.paused_until, "apps": apps });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

fn once() {
    let Some(_lock) = otherwise_helper::single_instance() else {
        eprintln!("{}", i18n::t!("helper.cli.running"));
        std::process::exit(3);
    };
    let mut e = system_engine();
    let root = otherwise_link::Root::system();
    let settings = otherwise_link::settings::HubSettings::load(&root).background;
    e.refresh_leases(&root, &settings);
    let lang = otherwise_helper::language();
    let c = otherwise_helper::sched::Conditions { now: chrono::Utc::now(), low_power: low_power(), pressure: otherwise_helper::sys::memory_pressure(), online: otherwise_helper::sys::online() };
    let open = |r: &otherwise_link::background::Registered| r.installed.executable.as_deref().is_some_and(otherwise_helper::sys::running);
    let step = e.step(&settings, &c, &open, &mut |p| otherwise_helper::runner::run(p, lang, &otherwise_helper::sys::rss), &chrono::Local);
    let ran = match &step {
        Step::Ran { app, job, outcome } => serde_json::json!({ "app": app, "job": job, "ok": outcome.ok, "error": outcome.error(), "status": outcome.report.as_ref().and_then(|r| r.status.clone()), "idle": outcome.report.as_ref().is_some_and(|r| r.idle), "seconds": outcome.elapsed.as_secs_f32(), "peakRssMb": outcome.peak_rss / (1024 * 1024) }),
        Step::Failed { app, job } => serde_json::json!({ "app": app, "job": job, "ok": false, "error": "start" }),
        Step::Gone { app } => serde_json::json!({ "gone": app }),
        Step::Nothing => serde_json::Value::Null,
    };
    let notes = e.notifications(&root, &settings, chrono::Utc::now());
    println!("{}", serde_json::json!({ "ran": ran, "notifications": notes }));
}

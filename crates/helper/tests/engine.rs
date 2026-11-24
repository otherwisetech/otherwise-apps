//! The helper against fake installs in temp dirs: real child processes (shell scripts), never
//! /Applications or the user's data root.
#![cfg(unix)]
use chrono::{Duration, Utc};
use otherwise_helper::engine::{Engine, Step};
use otherwise_helper::menu::{self, Action, Item};
use otherwise_helper::runner::{self, Killed};
use otherwise_helper::sched::{Conditions, Pressure, Wait};
use otherwise_link::Root;
use otherwise_link::apps::{Locations, Os, known_specs};
use otherwise_link::background::{self, Job, Label, Manifest, Notification};
use otherwise_link::settings::BackgroundSettings;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const EXPENSE: &str = "io.github.otherwisetech.expense";

struct World {
    _t: tempfile::TempDir,
    apps: PathBuf,
    trash: PathBuf,
    root: Root,
    state: PathBuf,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let apps = t.path().join("Applications");
    let trash = t.path().join("Trash");
    for d in [&apps, &trash] {
        fs::create_dir_all(d).unwrap();
    }
    World { root: Root::at(t.path().join("data")), state: t.path().join("state"), apps, trash, _t: t }
}

impl World {
    fn loc(&self) -> Locations {
        Locations { os: Os::Macos, mac_dirs: vec![self.apps.clone()], roots: vec![], path: vec![] }
    }

    /// This computer ("Mac A"), holding its leases.
    fn engine(&self) -> Engine {
        self.engine_as("Mac A", &self.state)
    }

    fn engine_as(&self, host: &str, state: &std::path::Path) -> Engine {
        let mut e = Engine::new(self.loc(), known_specs(), state.to_path_buf());
        e.me = Some(otherwise_link::lock::Me { host: host.into(), pid: 1 });
        e.refresh_leases(&self.root, &BackgroundSettings::default());
        e
    }

    /// expense.app whose `expense` binary is `script`, declaring `jobs`.
    fn install(&self, script: &str, jobs: Vec<Job>) -> PathBuf {
        let app = self.apps.join("expense.app");
        let c = app.join("Contents");
        fs::create_dir_all(c.join("MacOS")).unwrap();
        fs::create_dir_all(c.join("Resources")).unwrap();
        fs::write(c.join("Info.plist"), format!("<plist><dict><key>CFBundleIdentifier</key><string>{EXPENSE}</string></dict></plist>")).unwrap();
        let exe = c.join("MacOS/expense");
        fs::write(&exe, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(c.join("Resources").join(background::MANIFEST), Manifest { schema: 1, app: "expense".into(), jobs, serve: None }.to_json()).unwrap();
        app
    }

    /// A job script that queues a notification and reports.
    fn notifying_script(&self) -> String {
        let outbox = background::outbox_dir(&self.root, "expense");
        format!(
            "mkdir -p '{o}'\nprintf '{{\"schema\":1,\"id\":\"bill-1\",\"title\":\"Bill due\",\"link\":\"otherwise://expense/payable/1\",\"createdAt\":\"%s\"}}' \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\" > '{o}/bill-1.json'\necho \"job=$OTHERWISE_BACKGROUND_JOB lang=$OTHERWISE_LANG\"\necho '{{\"otherwiseJob\":1,\"status\":\"1 bill due\"}}'",
            o = outbox.display()
        )
    }
}

fn mail() -> Job {
    let mut j = Job::every("mail", Label::en("background.mail", "Check mail").with("es", "Revisar correo"), "15m", &["background", "run", "mail"]);
    j.network = true;
    j.heavy = true;
    j
}

fn normal() -> Conditions {
    Conditions { now: Utc::now(), low_power: false, pressure: Pressure::Normal, online: true }
}

fn never_open(_: &background::Registered) -> bool {
    false
}

fn run_real(p: &background::Plan) -> std::io::Result<runner::Outcome> {
    runner::run(p, "es", &|_| Some(1 << 20))
}

#[test]
fn runs_the_installed_apps_job_and_shows_its_notification_once() {
    let w = world();
    w.install(&w.notifying_script(), vec![mail()]);
    let mut e = w.engine();
    assert_eq!(e.apps.len(), 1);
    let s = BackgroundSettings::default();
    let step = e.step(&s, &normal(), &never_open, &mut run_real, &chrono::Local);
    let Step::Ran { app, job, outcome } = step else { panic!("{step:?}") };
    assert_eq!((app.as_str(), job.as_str(), outcome.ok), ("expense", "mail", true));
    assert_eq!(outcome.report.unwrap().status.as_deref(), Some("1 bill due"));
    let js = e.state.job("expense", "mail");
    assert_eq!((js.failures, js.status.as_deref()), (0, Some("1 bill due")));
    let notes = e.notifications(&w.root, &s, Utc::now());
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].app.as_str(), notes[0].link.as_deref()), ("expense", Some("otherwise://expense/payable/1")));
    // not due again for 15 minutes
    assert_eq!(e.step(&s, &normal(), &never_open, &mut run_real, &chrono::Local), Step::Nothing);
    // the same notification queued again isn't shown twice
    background::notify(&w.root, "expense", &Notification::new("bill-1", "Bill due")).unwrap();
    assert!(e.notifications(&w.root, &s, Utc::now()).is_empty());
    // state survives a restart
    let e2 = w.engine();
    assert!(e2.state.job("expense", "mail").last_ok.is_some());
}

#[test]
fn missing_binary_is_skipped_quietly_and_not_rescheduled() {
    let w = world();
    let app = w.install("exit 0", vec![mail()]);
    let mut e = w.engine();
    fs::remove_file(app.join("Contents/MacOS/expense")).unwrap();
    let mut ran = false;
    let step = e.step(&BackgroundSettings::default(), &normal(), &never_open, &mut |_| {
        ran = true;
        unreachable!()
    }, &chrono::Local);
    assert_eq!(step, Step::Gone { app: "expense".into() });
    assert!(!ran);
    assert!(e.apps.is_empty() && e.gone.contains("expense"));
    // no error loop: nothing more to do, and no failure recorded
    assert_eq!(e.step(&BackgroundSettings::default(), &normal(), &never_open, &mut run_real, &chrono::Local), Step::Nothing);
    assert_eq!(e.state.job("expense", "mail").failures, 0);
}

#[test]
fn a_manifest_without_its_app_gives_no_jobs_and_no_notifications() {
    let w = world();
    // left behind in the data root by an app that was deleted
    let stale = w.root.app_dir("expense").join(background::MANIFEST);
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, Manifest { schema: 1, app: "expense".into(), jobs: vec![mail()], serve: None }.to_json()).unwrap();
    background::notify(&w.root, "expense", &Notification::new("x", "From a deleted app")).unwrap();
    let mut e = w.engine();
    assert!(e.apps.is_empty());
    assert_eq!(e.step(&BackgroundSettings::default(), &normal(), &never_open, &mut run_real, &chrono::Local), Step::Nothing);
    assert!(e.notifications(&w.root, &BackgroundSettings::default(), Utc::now()).is_empty());
    assert!(background::pending(&w.root, "expense"), "an uninstalled app's outbox is left alone");
    assert!(stale.exists());
    let items = menu::build(&e.apps, &[], &e.state, "en", Utc::now(), &chrono::Local);
    assert!(!items.iter().any(|i| matches!(i, Item::Action(Action::OpenApp(_), _))));
}

#[test]
fn app_removed_while_running_then_reinstalled() {
    let w = world();
    let app = w.install(&w.notifying_script(), vec![mail()]);
    let mut e = w.engine();
    let s = BackgroundSettings::default();
    assert!(matches!(e.step(&s, &normal(), &never_open, &mut run_real, &chrono::Local), Step::Ran { .. }));
    // dragged to the Trash with a notification still queued
    background::notify(&w.root, "expense", &Notification::new("late", "Queued before removal")).unwrap();
    fs::rename(&app, w.trash.join("expense.app")).unwrap();
    assert!(e.notifications(&w.root, &s, Utc::now()).is_empty(), "no notifications for an app that is gone");
    assert!(e.gone.contains("expense") && e.apps.is_empty());
    let later = Conditions { now: Utc::now() + Duration::hours(1), ..normal() };
    assert_eq!(e.step(&s, &later, &never_open, &mut run_real, &chrono::Local), Step::Nothing);
    e.rescan();
    assert!(e.apps.is_empty());
    // reinstalled: the next scan finds it and it runs again
    w.install(&w.notifying_script(), vec![mail()]);
    e.rescan();
    e.refresh_leases(&w.root, &s);
    assert_eq!(e.apps.len(), 1);
    assert!(!e.gone.contains("expense"));
    assert!(matches!(e.step(&s, &later, &never_open, &mut run_real, &chrono::Local), Step::Ran { .. }));
    // the queued notification is delivered now that the app is back
    let notes = e.notifications(&w.root, &s, Utc::now());
    assert!(notes.iter().any(|n| n.id == "late"), "{notes:?}");
}

#[test]
fn waits_off_pause_power_memory_network_open() {
    let w = world();
    let mut light = Job::every("bills", Label::en("k", "Bills"), "1h", &["background", "run", "bills"]);
    light.while_open = false;
    w.install("exit 0", vec![mail(), light]);
    let mut e = w.engine();
    let s = BackgroundSettings::default();
    let wait_of = |e: &Engine, s: &BackgroundSettings, c: &Conditions, open: &dyn Fn(&background::Registered) -> bool| -> Vec<(String, Option<Wait>)> {
        e.decisions(s, c, open, &chrono::Local).into_iter().map(|d| (d.job, d.wait)).collect()
    };
    let c = normal();
    assert_eq!(wait_of(&e, &s, &Conditions { low_power: true, ..c }, &never_open), [("mail".into(), Some(Wait::LowPower)), ("bills".into(), Some(Wait::LowPower))]);
    assert_eq!(wait_of(&e, &s, &Conditions { pressure: Pressure::Warn, ..c }, &never_open), [("mail".into(), Some(Wait::Memory)), ("bills".into(), None)]);
    assert_eq!(wait_of(&e, &s, &Conditions { pressure: Pressure::Critical, ..c }, &never_open)[1].1, Some(Wait::Memory));
    assert_eq!(wait_of(&e, &s, &Conditions { online: false, ..c }, &never_open), [("mail".into(), Some(Wait::Offline)), ("bills".into(), None)]);
    assert_eq!(wait_of(&e, &s, &c, &|_| true), [("mail".into(), None), ("bills".into(), Some(Wait::AppOpen))]);
    let mut off = BackgroundSettings::default();
    BackgroundSettings::set(&mut off.jobs_off, "expense", false);
    assert!(wait_of(&e, &off, &c, &never_open).iter().all(|(_, w)| *w == Some(Wait::Off)));
    assert_eq!(e.step(&off, &c, &never_open, &mut run_real, &chrono::Local), Step::Nothing);
    e.pause(Some(c.now + Duration::hours(1)));
    assert!(wait_of(&e, &s, &c, &never_open).iter().all(|(_, w)| *w == Some(Wait::Paused)));
    let items = menu::build(&e.apps, &e.decisions(&s, &c, &never_open, &chrono::Local), &e.state, "en", c.now, &chrono::Local);
    assert!(items.iter().any(|i| matches!(i, Item::Action(Action::Resume, _))));
    e.pause(None);
    assert!(wait_of(&e, &s, &c, &never_open).iter().all(|(_, w)| w.is_none()));
}

#[test]
fn failures_back_off_and_kills_are_reported() {
    let w = world();
    w.install("echo nope; exit 3", vec![mail()]);
    let mut e = w.engine();
    let s = BackgroundSettings::default();
    let c = normal();
    let Step::Ran { outcome, .. } = e.step(&s, &c, &never_open, &mut run_real, &chrono::Local) else { panic!() };
    assert_eq!((outcome.ok, outcome.error().as_deref()), (false, Some("exit 3")));
    assert_eq!(e.state.job("expense", "mail").failures, 1);
    // retried after the backoff (the interval, 15 min), not right away
    assert_eq!(e.step(&s, &Conditions { now: c.now + Duration::minutes(5), ..c }, &never_open, &mut run_real, &chrono::Local), Step::Nothing);
    assert!(matches!(e.step(&s, &Conditions { now: c.now + Duration::minutes(16), ..c }, &never_open, &mut run_real, &chrono::Local), Step::Ran { .. }));
    assert_eq!(e.state.job("expense", "mail").failures, 2);
    let items = menu::build(&e.apps, &e.decisions(&s, &c, &never_open, &chrono::Local), &e.state, "en", c.now, &chrono::Local);
    let failed_at = menu::when(e.state.job("expense", "mail").last_end.unwrap(), c.now, &chrono::Local);
    assert!(items.iter().any(|i| matches!(i, Item::Text(t) if t.contains("Check mail") && t.contains(&failed_at))), "{items:?}");
    // the menu never says anything for apps it doesn't have
    assert!(!items.iter().any(|i| matches!(i, Item::Action(Action::OpenApp(a), _) if a != "expense")));

    // timeout and memory ceiling
    let mut slow = mail();
    slow.timeout_secs = Some(5);
    w.install("sleep 30", vec![slow]);
    let plan = background::plan(&background::scan(&w.loc())[0], "mail", &w.loc()).unwrap();
    let o = runner::run(&plan, "en", &|_| Some(1 << 20)).unwrap();
    assert_eq!(o.killed, Some(Killed::Timeout));
    assert!(o.elapsed.as_secs() < 15);
    let o = runner::run(&plan, "en", &|_| Some(u64::MAX / 2)).unwrap();
    assert_eq!((o.killed, o.error().as_deref()), (Some(Killed::Memory), Some("memory")));
}

#[test]
fn idle_jobs_dont_notify_and_say_so() {
    let w = world();
    w.install(&format!("echo '{}'", background::Report::idle().line()), vec![mail()]);
    let mut e = w.engine();
    let s = BackgroundSettings::default();
    assert!(matches!(e.step(&s, &normal(), &never_open, &mut run_real, &chrono::Local), Step::Ran { .. }));
    assert!(e.state.job("expense", "mail").idle);
    assert!(e.notifications(&w.root, &s, Utc::now()).is_empty());
    i18n::set_locale("es");
    let items = menu::build(&e.apps, &e.decisions(&s, &normal(), &never_open, &chrono::Local), &e.state, "es", Utc::now(), &chrono::Local);
    assert!(items.iter().any(|i| matches!(i, Item::Text(t) if t.contains("Revisar correo") && t.contains("sin configurar"))), "{items:?}");
}

#[test]
fn notifications_respect_quiet_and_per_app_switches() {
    let w = world();
    w.install("exit 0", vec![mail()]);
    let mut e = w.engine();
    let quiet = BackgroundSettings { quiet: true, ..Default::default() };
    background::notify(&w.root, "expense", &Notification::new("a", "A")).unwrap();
    assert!(e.notifications(&w.root, &quiet, Utc::now()).is_empty());
    let mut muted = BackgroundSettings::default();
    BackgroundSettings::set(&mut muted.notifications_off, "expense", false);
    background::notify(&w.root, "expense", &Notification::new("b", "B")).unwrap();
    assert!(e.notifications(&w.root, &muted, Utc::now()).is_empty());
    background::notify(&w.root, "expense", &Notification::new("c", "C")).unwrap();
    assert_eq!(e.notifications(&w.root, &BackgroundSettings::default(), Utc::now()).len(), 1);
}

#[test]
fn two_computers_on_one_synced_root_run_each_job_once() {
    let w = world();
    w.install(&w.notifying_script(), vec![mail()]);
    let s = BackgroundSettings::default();
    let mut a = w.engine();
    // Mac B: same data root (synced), its own device state
    let b_state = w.state.with_file_name("state-b");
    let mut b = w.engine_as("Mac B", &b_state);
    assert!(a.held.contains_key("expense") && !b.held.contains_key("expense"));
    let waits: Vec<_> = b.decisions(&s, &normal(), &never_open, &chrono::Local).into_iter().map(|d| d.wait).collect();
    assert_eq!(waits, [Some(Wait::Elsewhere("Mac A".into()))]);
    assert_eq!(b.step(&s, &normal(), &never_open, &mut |_| unreachable!("Mac B must not run it"), &chrono::Local), Step::Nothing);
    let items = menu::build(&b.apps, &b.decisions(&s, &normal(), &never_open, &chrono::Local), &b.state, "en", Utc::now(), &chrono::Local);
    assert!(items.iter().any(|i| matches!(i, Item::Text(t) if t.contains("Mac A"))), "{items:?}");
    // A runs it; its notification is drained by A only
    assert!(matches!(a.step(&s, &normal(), &never_open, &mut run_real, &chrono::Local), Step::Ran { .. }));
    assert!(b.notifications(&w.root, &s, Utc::now()).is_empty());
    assert!(background::pending(&w.root, "expense"), "left for the lease holder");
    assert_eq!(a.notifications(&w.root, &s, Utc::now()).len(), 1);
    // device state stays per computer
    assert!(a.state.job("expense", "mail").last_ok.is_some() && b.state.job("expense", "mail").last_ok.is_none());
    // A goes away: after the lease lapses, B takes over
    b.clock = Some(otherwise_link::lock::now() + otherwise_link::lock::STALE_AFTER.as_secs() as i64 + 5);
    b.refresh_leases(&w.root, &s);
    assert!(b.held.contains_key("expense"));
    assert!(matches!(b.step(&s, &normal(), &never_open, &mut run_real, &chrono::Local), Step::Ran { .. }));
    // and A, back later, waits for B
    a.clock = b.clock.map(|c| c + 30);
    a.refresh_leases(&w.root, &s);
    assert_eq!(a.waiting.get("expense"), Some(&Wait::Elsewhere("Mac B".into())));
}

#[test]
fn turning_jobs_off_lets_go_of_the_lease() {
    let w = world();
    w.install("exit 0", vec![mail()]);
    let mut a = w.engine();
    assert!(a.held.contains_key("expense"));
    let mut off = BackgroundSettings::default();
    BackgroundSettings::set(&mut off.jobs_off, "expense", false);
    a.refresh_leases(&w.root, &off);
    assert!(a.held.is_empty() && a.waiting.is_empty());
}

#[test]
fn the_real_engine_finds_apps_that_describe_themselves_installed_later() {
    let w = world();
    let mut e = w.engine();
    e.refresh_specs = true;
    assert!(e.apps.is_empty());
    // an app not in otherwise-link's table, with its own otherwise-app.json
    let c = w.apps.join("Surco Riego.app/Contents");
    fs::create_dir_all(c.join("MacOS")).unwrap();
    fs::create_dir_all(c.join("Resources")).unwrap();
    fs::write(c.join("Info.plist"), "<plist><dict><key>CFBundleIdentifier</key><string>com.otherwisetech.surco.riego</string></dict></plist>").unwrap();
    let exe = c.join("MacOS/surco-riego");
    fs::write(&exe, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(c.join("Resources").join(otherwise_link::apps::APP_MANIFEST), r#"{"schema":1,"id":"surco-riego","name":"Surco Riego","bundleId":"com.otherwisetech.surco.riego","binary":"surco-riego"}"#).unwrap();
    fs::write(c.join("Resources").join(background::MANIFEST), Manifest { schema: 1, app: "surco-riego".into(), jobs: vec![mail()], serve: None }.to_json()).unwrap();
    e.rescan();
    assert_eq!(e.apps.iter().map(|r| r.spec.id.as_str()).collect::<Vec<_>>(), ["surco-riego"]);
    // a test engine keeps the list it was given
    let mut fixed = w.engine();
    fixed.rescan();
    assert!(fixed.apps.is_empty());
}

//! Otherwise Helper from the launcher's side: the jobs it runs for us (`otherwise-apps background
//! run <job>`, no window), starting it (on by default once, `open -g` of the helper inside our
//! bundle) and Settings → Otherwise Helper (start at login, quiet mode, per-app switches).
//!
//! The helper lives at `Otherwise Apps.app/Contents/Library/LoginItems/Otherwise Helper.app`
//! and registers itself as a login item (`SMAppService.mainApp`), so System Settings → Login
//! Items shows it by name.
use crate::app::Launcher;
use crate::theme::Tokens;
use crate::ui;
use gpui::*;
use gpui_component::switch::Switch;
use otherwise_core::background as jobs;
use otherwise_link::Root;
use otherwise_link::apps::Locations;
use otherwise_link::background::{self, Report};
use otherwise_link::settings::{BackgroundSettings, HubSettings};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const BUNDLE: &str = "Otherwise Helper.app"; // i18n-ok: bundle name
pub const BINARY: &str = "otherwise-helper"; // i18n-ok: binary name

/// `otherwise-apps background manifest | run <job>`: prints the manifest, or runs a job and
/// prints its report line. Exit code 0 unless the job failed.
pub fn run_cli(args: &[String]) -> i32 {
    let root = Root::system();
    match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
        (Some("manifest"), _) => {
            print!("{}", jobs::manifest().to_json());
            0
        }
        (Some("run"), Some(jobs::SYNC)) => {
            let r = jobs::sync(&root, &|u| otherwise_core::net::get_calendar(u).map_err(|e| e.to_string()), chrono::Utc::now().timestamp(), &i18n::t!("app.cal.ics_name"), &i18n::t!("app.cal.busy"));
            println!("{}", jobs::sync_report(&root, &r).line());
            if r.failed > 0 && r.refreshed == 0 { 1 } else { 0 }
        }
        (Some("run"), Some(jobs::UPCOMING)) => {
            let installed: Vec<String> = otherwise_link::apps::discover(&Locations::system()).into_iter().map(|(s, _)| s.id).collect();
            let now = chrono::Utc::now();
            let u = jobs::upcoming(&root, now, &chrono::Local, &|a| installed.iter().any(|i| i == a), &i18n::t!("app.cal.busy"));
            for n in &u.notifications {
                let _ = background::notify(&root, otherwise_link::apps::HUB_ID, n);
            }
            println!("{}", jobs::upcoming_report(&u, now, &chrono::Local).line());
            0
        }
        _ => {
            println!("{}", Report::status(i18n::t!("app.helper.unknown_job")).line());
            2
        }
    }
}

/// The helper bundle inside ours (macOS), if this build has one.
pub fn helper_app() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    let app = contents.join("Library").join("LoginItems").join(BUNDLE);
    app.join("Contents").join("MacOS").join(BINARY).is_file().then_some(app)
}

fn helper(args: &[&str]) -> Option<String> {
    let exe = helper_app()?.join("Contents").join("MacOS").join(BINARY);
    let out = Command::new(exe).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(|l| l.trim().to_string())
}

/// `on`, `off`, `approval` or `unsupported`; None without the helper.
pub fn login_state() -> Option<String> {
    helper(&["login", "status"])
}

pub fn set_login(on: bool) -> Option<String> {
    helper(&["login", if on { "on" } else { "off" }])
}

/// Starts the helper in the background (no-op if it is running: one instance per user).
pub fn launch() {
    if let Some(app) = helper_app() {
        let _ = Command::new("open").arg("-g").arg(&app).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

/// At launch: the first time, turn start at login on (the default) and remember it; then start
/// the helper unless the user turned it off.
pub fn on_launch() {
    if helper_app().is_none() {
        return;
    }
    std::thread::spawn(|| {
        let s = crate::settings::Settings::load();
        if s.helper.is_none() {
            let on = set_login(true).is_some_and(|st| st == "on" || st == "approval");
            crate::settings::Settings::update(|s| s.helper = Some(if on { "on" } else { "off" }.into()));
        }
        if crate::settings::Settings::load().helper.as_deref() != Some("off") {
            launch();
        }
    });
}

/// What Settings shows (read when the window activates; the switches update it).
#[derive(Debug, Clone, Default)]
pub struct HelperView {
    pub present: bool,
    pub login: Option<String>,
    /// (app id, name) of installed apps that declare background jobs.
    pub apps: Vec<(String, String)>,
}

impl HelperView {
    pub fn load() -> HelperView {
        let apps = background::scan(&Locations::system()).into_iter().map(|r| (r.spec.id, r.spec.name)).collect();
        HelperView { present: helper_app().is_some(), login: login_state(), apps }
    }
}

impl Launcher {
    pub fn reload_helper(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move { HelperView::load() });
        cx.spawn(async move |this, cx| {
            let v = task.await;
            this.update(cx, |this, cx| {
                this.helper = v;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_background(&mut self, f: impl FnOnce(&mut BackgroundSettings), cx: &mut Context<Self>) {
        if let Ok(s) = HubSettings::update(&self.hub.root, |s| f(&mut s.background)) {
            self.hub.settings = s;
        }
        cx.notify();
    }

    fn set_start_at_login(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings = crate::settings::Settings::update(|s| s.helper = Some(if on { "on" } else { "off" }.into()));
        let task = cx.background_executor().spawn(async move {
            let st = set_login(on);
            if on {
                launch();
            }
            st
        });
        cx.spawn(async move |this, cx| {
            let st = task.await;
            this.update(cx, |this, cx| {
                this.helper.login = st;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn render_helper_settings(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let row = |label: String, hint: Option<String>, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .py(px(10.))
                .border_t_1()
                .border_color(t.border)
                .child(ui::section_title(t, label))
                .child(control)
                .children(hint.map(|h| div().text_size(px(11.)).line_height(px(16.)).text_color(t.ink_muted).child(h)))
        };
        let weak = cx.entity().downgrade();
        let bg = &self.hub.settings.background;
        let title = div().pt(px(14.)).pb(px(4.)).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.helper.title"));
        let mut out = div().flex().flex_col().child(title);
        if !self.helper.present {
            return out.child(row(i18n::t!("app.helper.login"), Some(i18n::t!("app.helper.not_bundled")), div().into_any_element()));
        }
        let login_on = matches!(self.helper.login.as_deref(), Some("on" | "approval"));
        let hint = match self.helper.login.as_deref() {
            Some("approval") => i18n::t!("app.helper.login_approval"),
            Some("unsupported") => i18n::t!("app.helper.login_unsupported"),
            _ => i18n::t!("app.helper.login_hint"),
        };
        let login = Switch::new("helper-login").checked(login_on).label(i18n::t!("app.helper.login_label")).on_click({
            let w = weak.clone();
            move |on, _, cx| {
                w.update(cx, |this, cx| this.set_start_at_login(*on, cx)).ok();
            }
        });
        let quiet = Switch::new("helper-quiet").checked(bg.quiet).label(i18n::t!("app.helper.quiet_label")).on_click({
            let w = weak.clone();
            move |on, _, cx| {
                let on = *on;
                w.update(cx, |this, cx| this.set_background(|b| b.quiet = on, cx)).ok();
            }
        });
        let mut list = div().flex().flex_col().gap(px(6.));
        if self.helper.apps.is_empty() {
            list = list.child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.helper.no_apps")));
        }
        for (i, (id, name)) in self.helper.apps.iter().enumerate() {
            let jobs_on = bg.jobs_on(id);
            let (w1, w2, id1, id2) = (weak.clone(), weak.clone(), id.clone(), id.clone());
            let jobs = Switch::new(("helper-jobs", i)).checked(jobs_on).label(i18n::t!("app.helper.app_jobs", app = name)).on_click(move |on, _, cx| {
                let (on, id) = (*on, id1.clone());
                w1.update(cx, |this, cx| this.set_background(|b| BackgroundSettings::set(&mut b.jobs_off, &id, on), cx)).ok();
            });
            let notes = Switch::new(("helper-notes", i)).checked(!bg.notifications_off.iter().any(|a| a == id)).label(i18n::t!("app.helper.app_notifications")).on_click(move |on, _, cx| {
                let (on, id) = (*on, id2.clone());
                w2.update(cx, |this, cx| this.set_background(|b| BackgroundSettings::set(&mut b.notifications_off, &id, on), cx)).ok();
            });
            list = list.child(div().flex().flex_wrap().gap(px(16.)).child(jobs).child(notes));
        }
        out = out
            .child(row(i18n::t!("app.helper.login"), Some(hint), login.into_any_element()))
            .child(row(i18n::t!("app.helper.quiet"), Some(i18n::t!("app.helper.quiet_hint")), quiet.into_any_element()))
            .child(row(i18n::t!("app.helper.apps"), Some(i18n::t!("app.helper.apps_hint")), list.into_any_element()));
        out
    }
}

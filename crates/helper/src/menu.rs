//! The menu-bar menu as data (so it is tested without AppKit): one section per installed app
//! with jobs, then the actions.
use crate::sched::{Decision, Wait};
use crate::state::State;
use chrono::{DateTime, TimeZone, Utc};
use otherwise_link::background::Registered;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Pause,
    Resume,
    OpenHub,
    Settings,
    About,
    Quit,
    /// Opens the app (`otherwise://<id>`).
    OpenApp(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Not clickable.
    Text(String),
    Action(Action, String),
    Separator,
}

/// `14:05` today, else `2026-11-24 14:05`, local time.
pub fn when<Tz: TimeZone>(t: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (l, n) = (t.with_timezone(tz), now.with_timezone(tz));
    if l.date_naive() == n.date_naive() { l.format("%H:%M").to_string() } else { l.format("%Y-%m-%d %H:%M").to_string() }
}

/// One job's line: label, then what happened or what it waits for.
pub fn job_line<Tz: TimeZone>(label: &str, d: &Decision, state: &State, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let js = state.job(&d.app, &d.job);
    let last = if js.failures > 0 {
        i18n::t!("helper.job.failed", job = label, time = js.last_end.map(|t| when(t, now, tz)).unwrap_or_default())
    } else if js.idle {
        i18n::t!("helper.job.idle", job = label)
    } else if let Some(t) = js.last_ok {
        match &js.status {
            Some(s) => i18n::t!("helper.job.status", job = label, time = when(t, now, tz), status = s),
            None => i18n::t!("helper.job.ok", job = label, time = when(t, now, tz)),
        }
    } else {
        i18n::t!("helper.job.never", job = label)
    };
    let wait = match d.wait {
        Some(Wait::Off) => Some(i18n::t!("helper.wait.off")),
        Some(Wait::LowPower) => Some(i18n::t!("helper.wait.low_power")),
        Some(Wait::Memory) => Some(i18n::t!("helper.wait.memory")),
        Some(Wait::Offline) => Some(i18n::t!("helper.wait.offline")),
        Some(Wait::AppOpen) => Some(i18n::t!("helper.wait.app_open")),
        Some(Wait::Paused) | Some(Wait::Until(_)) | None => None,
    };
    match wait {
        Some(w) => i18n::t!("helper.job.waiting", line = last, wait = w),
        None => last,
    }
}

/// The whole menu. Apps that aren't installed have no section: `apps` holds installed ones only.
pub fn build<Tz: TimeZone>(apps: &[Registered], decisions: &[Decision], state: &State, lang: &str, now: DateTime<Utc>, tz: &Tz) -> Vec<Item>
where
    Tz::Offset: std::fmt::Display,
{
    let mut items = vec![];
    if state.paused(now) {
        let until = state.paused_until.map(|t| when(t, now, tz)).unwrap_or_default();
        items.push(Item::Text(i18n::t!("helper.menu.paused_until", time = until)));
        items.push(Item::Separator);
    }
    if apps.is_empty() {
        items.push(Item::Text(i18n::t!("helper.menu.no_apps")));
    }
    for r in apps {
        items.push(Item::Action(Action::OpenApp(r.spec.id.clone()), r.spec.name.clone()));
        for j in &r.manifest.jobs {
            if let Some(d) = decisions.iter().find(|d| d.app == r.spec.id && d.job == j.id) {
                items.push(Item::Text(format!("    {}", job_line(j.label.get(lang), d, state, now, tz)))); // i18n-ok: indentation only
            }
        }
    }
    items.push(Item::Separator);
    if state.paused(now) {
        items.push(Item::Action(Action::Resume, i18n::t!("helper.menu.resume")));
    } else {
        items.push(Item::Action(Action::Pause, i18n::t!("helper.menu.pause")));
    }
    items.push(Item::Action(Action::OpenHub, i18n::t!("helper.menu.open_hub")));
    items.push(Item::Action(Action::Settings, i18n::t!("helper.menu.settings")));
    items.push(Item::Separator);
    items.push(Item::Action(Action::About, i18n::t!("helper.menu.about")));
    items.push(Item::Action(Action::Quit, i18n::t!("helper.menu.quit")));
    items
}

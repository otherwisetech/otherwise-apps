//! The launcher's own background jobs, run by Otherwise Helper as
//! `otherwise-apps background run <job>` (declared in the bundle by `script/bundle-macos.sh`):
//!
//! - `calendar-sync` (hourly, network): refreshes the `.ics` subscriptions (URL overlays) so the
//!   Calendar page is current when it opens, and rewrites the combined `otherwise.ics` when the
//!   user turned it on.
//! - `upcoming` (every 5 minutes): timed events starting in the next [`LEAD_MINUTES`] become a
//!   notification (once per occurrence; busy-only events as "Busy" without details), and the
//!   next event is the job's status line in the helper's menu. Events of apps that aren't
//!   installed are skipped.
use crate::calendar::{self as cal, Settings, Source};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use otherwise_link::background::{Job, Label, Manifest, Notification, Report};
use otherwise_link::{Link, Root};

pub const SYNC: &str = "calendar-sync";
pub const UPCOMING: &str = "upcoming";
pub const LEAD_MINUTES: i64 = 15;
/// A subscription is fetched again only when its copy is older than this.
pub const REFRESH_AFTER_SECS: i64 = 50 * 60;

fn label(key: &str) -> Label {
    let mut l = Label::en(key, &i18n::tr_in("en", key));
    for (code, _) in i18n::available() {
        if code != "en" {
            l = l.with(code, &i18n::tr_in(code, key));
        }
    }
    l
}

/// What the bundle ships as `Contents/Resources/otherwise-background.json`.
pub fn manifest() -> Manifest {
    let mut sync = Job::every(SYNC, label("core.background.sync"), "1h", &["background", "run", SYNC]); // i18n-ok: arguments
    sync.network = true;
    sync.timeout_secs = Some(180);
    sync.memory_mb = Some(300);
    let mut upcoming = Job::every(UPCOMING, label("core.background.upcoming"), "5m", &["background", "run", UPCOMING]); // i18n-ok: arguments
    upcoming.timeout_secs = Some(60);
    upcoming.memory_mb = Some(200);
    Manifest { schema: 1, app: cal::APP.into(), jobs: vec![sync, upcoming], serve: None }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    pub refreshed: usize,
    pub failed: usize,
    pub combined: bool,
}

/// Refreshes stale URL subscriptions (a failed one keeps its last copy) and the combined file.
pub fn sync(root: &Root, fetch: &dyn Fn(&str) -> Result<String, String>, now: i64, ics_name: &str, busy_label: &str) -> Synced {
    let mut s = Settings::load(root);
    let mut out = Synced::default();
    let due: Vec<String> = s.overlays.iter().filter(|o| o.enabled && o.is_url() && o.fetched.is_none_or(|f| now - f >= REFRESH_AFTER_SECS)).map(|o| o.id.clone()).collect();
    for id in due {
        match cal::refresh_overlay(root, &mut s, &id, fetch, now) {
            Ok(()) => out.refreshed += 1,
            Err(_) => out.failed += 1,
        }
    }
    if s.combined {
        out.combined = cal::write_combined(root, ics_name, busy_label).is_ok();
    }
    out
}

/// `sync`'s report line.
pub fn sync_report(root: &Root, r: &Synced) -> Report {
    let subs = Settings::load(root).overlays.iter().filter(|o| o.enabled && o.is_url()).count();
    if subs == 0 && !r.combined {
        return Report::idle();
    }
    if r.failed > 0 {
        return Report::status(i18n::t!("core.background.sync_failed", n = r.failed));
    }
    Report::status(i18n::tn!("core.background.synced", subs, n = subs))
}

/// What `upcoming` found.
#[derive(Debug, Clone, PartialEq)]
pub struct Upcoming {
    pub notifications: Vec<Notification>,
    /// (title, start) of the next event from now on, for the menu.
    pub next: Option<(String, DateTime<Utc>)>,
}

fn notification_id(key: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("event-{h:016x}")
}

/// Events of the next week (timed ones notify within the lead time). `installed(app)` filters
/// apps' events; holidays never notify.
pub fn upcoming<Tz: TimeZone>(root: &Root, now: DateTime<Utc>, tz: &Tz, installed: &dyn Fn(&str) -> bool, busy_label: &str) -> Upcoming
where
    Tz::Offset: std::fmt::Display,
{
    let s = Settings::load(root);
    let today: NaiveDate = now.with_timezone(tz).date_naive();
    let view = cal::build(root, &s, today, today + Duration::days(7), tz, &[]);
    let lead = now + Duration::minutes(LEAD_MINUTES);
    let mut out = Upcoming { notifications: vec![], next: None };
    for item in cal::visible(&view, &s, None) {
        let ok_source = match &item.source {
            Source::App(app) => installed(app),
            Source::Overlay(_) => true,
            _ => false,
        };
        if !ok_source || item.occ.all_day() {
            continue;
        }
        let start = item.occ.start.instant(tz);
        if start <= now {
            continue;
        }
        let occ = if item.occ.event.busy_only { item.occ.redacted(busy_label) } else { item.occ.clone() };
        if out.next.as_ref().is_none_or(|(_, t)| start < *t) {
            out.next = Some((occ.event.title.clone(), start));
        }
        if start <= lead {
            let time = start.with_timezone(tz).format("%H:%M").to_string();
            let mut n = Notification::new(notification_id(&item.occ.key()), occ.event.title.clone()).body(i18n::t!("core.background.starts_at", time = time));
            let link = item.target().and_then(|l| Link::parse(l).ok()).filter(|_| !item.occ.event.busy_only).unwrap_or_else(|| Link::new(cal::APP).segment("calendar").param("date", start.with_timezone(tz).format("%Y-%m-%d").to_string()));
            n = n.link(&link);
            out.notifications.push(n);
        }
    }
    out
}

/// `upcoming`'s report line.
pub fn upcoming_report<Tz: TimeZone>(u: &Upcoming, now: DateTime<Utc>, tz: &Tz) -> Report
where
    Tz::Offset: std::fmt::Display,
{
    match &u.next {
        None => Report::status(i18n::t!("core.background.no_events")),
        Some((title, at)) => {
            let (l, n) = (at.with_timezone(tz), now.with_timezone(tz));
            let when = if l.date_naive() == n.date_naive() { l.format("%H:%M").to_string() } else { l.format("%Y-%m-%d %H:%M").to_string() };
            Report::status(i18n::t!("core.background.next", title = title, when = when))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otherwise_link::events::{self, Event, Kind};

    fn feed(root: &Root, app: &str, evs: &[Event]) {
        events::publish(root, app, evs, None).unwrap();
    }

    #[test]
    fn manifest_declares_both_jobs_translated() {
        let m = manifest();
        let back = Manifest::parse(&m.to_json(), cal::APP).unwrap();
        assert_eq!(back.jobs.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), [SYNC, UPCOMING]);
        assert!(back.jobs[0].network && !back.jobs[1].network);
        assert_ne!(back.jobs[0].label.get("es"), back.jobs[0].label.get("en"));
        assert!(!back.jobs[0].label.get("fr").is_empty() && !back.jobs[0].label.get("pt").is_empty());
    }

    #[test]
    fn upcoming_notifies_soon_events_of_installed_apps_once_per_occurrence() {
        let t = tempfile::tempdir().unwrap();
        let root = Root::at(t.path());
        let tz = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-11-23T15:00:00Z").unwrap().with_timezone(&Utc);
        let at = |m: i64| (now + Duration::minutes(m)).to_rfc3339();
        feed(&root, "vigia", &[Event::new("soat", Kind::Due, "SOAT renewal call", at(10)).link(&Link::new("vigia").segment("item").segment("4")), Event::new("later", Kind::Due, "Later", at(120))]);
        feed(&root, "expense", &[Event::new("pay", Kind::Payment, "Rent transfer", at(5)).busy_only(true)]);
        feed(&root, "nodo", &[Event::new("x", Kind::Due, "From an uninstalled app", at(3))]);
        let installed = |a: &str| a != "nodo";
        let u = upcoming(&root, now, &tz, &installed, "Busy");
        let titles: Vec<&str> = u.notifications.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["Busy", "SOAT renewal call"]);
        assert_eq!(u.notifications[1].link.as_deref(), Some("otherwise://vigia/item/4"));
        assert!(u.notifications[0].link.as_deref().unwrap().starts_with("otherwise://otherwise-apps/calendar"), "busy-only: no link into the app");
        assert_eq!(u.next.as_ref().map(|n| n.0.as_str()), Some("Busy"));
        // same occurrence → same id (shown once by the helper)
        let again = upcoming(&root, now + Duration::minutes(1), &tz, &installed, "Busy");
        assert_eq!(again.notifications[1].id, u.notifications[1].id);
        let r = upcoming_report(&u, now, &tz);
        assert!(r.status.unwrap().contains("10:05"));
        // nothing at all
        let empty = Root::at(t.path().join("none"));
        let u = upcoming(&empty, now, &tz, &installed, "Busy");
        assert!(u.notifications.is_empty() && u.next.is_none());
    }

    #[test]
    fn sync_refreshes_stale_subscriptions_and_keeps_copies_on_failure() {
        let t = tempfile::tempdir().unwrap();
        let root = Root::at(t.path());
        let ics = "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Team\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261124T150000Z\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut s = Settings::load(&root);
        let ok = |_: &str| Ok::<String, String>(ics.to_string());
        let o = cal::add_overlay(&root, &mut s, "https://example.com/team.ics", &ok, 1000).unwrap();
        assert_eq!(sync_report(&Root::at(t.path().join("none")), &Synced::default()), Report::idle());
        // fresh: not fetched again
        let r = sync(&root, &|_| panic!("no fetch"), 1000 + 60, "Otherwise", "Busy");
        assert_eq!(r, Synced::default());
        // stale and failing: copy kept
        let r = sync(&root, &|_| Err("offline".into()), 1000 + REFRESH_AFTER_SECS, "Otherwise", "Busy");
        assert_eq!(r.failed, 1);
        assert!(o.copy_path(&root).exists());
        let r = sync(&root, &ok, 1000 + REFRESH_AFTER_SECS, "Otherwise", "Busy");
        assert_eq!(r.refreshed, 1);
        assert!(sync_report(&root, &r).status.is_some());
        let mut s = Settings::load(&root);
        s.combined = true;
        s.save(&root).unwrap();
        let r = sync(&root, &ok, 1000 + REFRESH_AFTER_SECS + 1, "Otherwise", "Busy");
        assert!(r.combined && cal::combined_path(&root).exists());
    }
}

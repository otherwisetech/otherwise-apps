//! The Calendar page's data, without UI: every app's events feed (otherwise-link `events`),
//! computed public holidays, read-only `.ics` overlays (a file or a URL, kept as a copy so they
//! work offline, refreshed only when the user asks) and, optionally, Vcity; plus the combined
//! `.ics` file other calendar apps subscribe to.
//!
//! Everything lives under `<data root>/otherwise-apps/calendar/`:
//!
//! ```text
//! settings.json      holidays, hidden apps, overlays, Vcity orgs (no secrets)
//! overlays/<id>.ics  the last copy of each overlay
//! otherwise.ics      every app's events in one calendar (busy-only events without details)
//! ```
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use otherwise_link::Root;
use otherwise_link::events::{self, Event, Occurrence};
use otherwise_link::holidays::Country;
use otherwise_link::ics;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP: &str = otherwise_link::apps::HUB_ID;
pub const DIR: &str = "calendar";
pub const SETTINGS: &str = "settings.json";
pub const COMBINED: &str = "otherwise.ics";
/// Pseudo app id of overlay events.
pub const OVERLAY_APP: &str = "ics";
/// Pseudo app id of Vcity events.
pub const VCITY_APP: &str = "vcity";
pub const MAX_OVERLAY: u64 = 16 << 20;

pub fn dir(root: &Root) -> PathBuf {
    root.app_dir(APP).join(DIR)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub id: String,
    pub name: String,
    /// A file path or an http(s) / webcal URL.
    pub source: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Unix seconds of the last good read.
    #[serde(default)]
    pub fetched: Option<i64>,
}

impl Overlay {
    pub fn is_url(&self) -> bool {
        is_url(&self.source)
    }

    pub fn copy_path(&self, root: &Root) -> PathBuf {
        dir(root).join("overlays").join(format!("{}.ics", self.id))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcityOrg {
    pub id: String,
    pub slug: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn default_holidays() -> Vec<String> {
    vec!["co".into(), "us".into()]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub schema: u32,
    pub show_holidays: bool,
    /// Country ids of `otherwise_link::holidays`.
    pub holidays: Vec<String>,
    /// App ids (or `holidays`, `ics`, `vcity`) the user hid.
    pub hidden: Vec<String>,
    pub overlays: Vec<Overlay>,
    /// Signed in to Vcity (the tokens are in the keychain, never here).
    pub vcity: bool,
    pub vcity_base: Option<String>,
    pub vcity_orgs: Vec<VcityOrg>,
    /// Write `otherwise.ics` whenever the calendar is read.
    pub combined: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { schema: 1, show_holidays: true, holidays: default_holidays(), hidden: Vec::new(), overlays: Vec::new(), vcity: false, vcity_base: None, vcity_orgs: Vec::new(), combined: false }
    }
}

impl Settings {
    pub fn load(root: &Root) -> Settings {
        std::fs::read_to_string(dir(root).join(SETTINGS)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, root: &Root) -> std::io::Result<()> {
        otherwise_link::fsutil::write_atomic(&dir(root).join(SETTINGS), serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }

    pub fn countries(&self) -> Vec<Country> {
        self.holidays.iter().filter_map(|c| Country::from_id(c)).collect()
    }

    pub fn shows(&self, app: &str) -> bool {
        !self.hidden.iter().any(|h| h == app)
    }

    pub fn toggle(&mut self, app: &str) {
        if self.shows(app) {
            self.hidden.push(app.to_string());
        } else {
            self.hidden.retain(|h| h != app);
        }
    }
}

pub fn is_url(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("webcal://")
}

/// Where an occurrence came from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    // holidays sort first on their day
    Holiday(Country),
    App(String),
    Overlay(String),
    Vcity,
}

impl Source {
    /// The id filters and colors key on.
    pub fn key(&self) -> String {
        match self {
            Source::App(a) => a.clone(),
            Source::Holiday(_) => events::HOLIDAYS_APP.to_string(),
            Source::Overlay(_) => OVERLAY_APP.to_string(),
            Source::Vcity => VCITY_APP.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub occ: Occurrence,
    pub source: Source,
}

impl Item {
    /// What opening it does: an `otherwise://` link or a web URL; holidays have none.
    pub fn target(&self) -> Option<&str> {
        self.occ.event.link.as_deref().filter(|l| l.starts_with("otherwise") || l.starts_with("https://") || l.starts_with("http://"))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// Everything in the range, hidden sources included (the UI filters, so toggling is instant).
    pub items: Vec<Item>,
    /// Apps that have an events file, sorted.
    pub apps: Vec<String>,
    /// (source, reason) for files and overlays that couldn't be read.
    pub errors: Vec<(String, String)>,
}

/// The overlay events read from their copies.
/// (overlay id, its events) and (overlay name, read error).
pub type OverlayEvents = (Vec<(String, Vec<Event>)>, Vec<(String, String)>);

pub fn overlay_events<Tz: TimeZone>(root: &Root, s: &Settings, tz: &Tz) -> OverlayEvents {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for o in s.overlays.iter().filter(|o| o.enabled) {
        match std::fs::read_to_string(o.copy_path(root)) {
            Ok(text) => {
                let mut evs = ics::parse(&text, tz).events;
                for e in &mut evs {
                    e.app = OVERLAY_APP.into();
                    e.category = Some(o.name.clone());
                    // only web links open from an overlay; file:// or javascript: never
                    e.link = e.link.take().filter(|l| l.starts_with("https://") || l.starts_with("http://"));
                }
                out.push((o.id.clone(), evs));
            }
            Err(e) => errors.push((o.name.clone(), e.to_string())),
        }
    }
    (out, errors)
}

/// Everything between `from` and `to` (local days of `tz`, inclusive), sorted. `extra`: events
/// fetched from Vcity (in memory only).
pub fn build<Tz: TimeZone>(root: &Root, s: &Settings, from: NaiveDate, to: NaiveDate, tz: &Tz, extra: &[Event]) -> View {
    let m = events::read_in(root, from, to, tz);
    let mut v = View { from: Some(from), to: Some(to), apps: m.updated.keys().filter(|a| a.as_str() != APP).cloned().collect(), errors: m.errors, ..Default::default() };
    v.items.extend(m.occurrences.into_iter().map(|o| Item { source: Source::App(o.event.app.clone()), occ: o }));
    if s.show_holidays {
        let hol = events::holiday_events(&s.countries(), from, to);
        for e in hol {
            let c = e.category.as_deref().and_then(Country::from_id).unwrap_or(Country::Co);
            v.items.extend(events::expand(&e, from, to, tz).into_iter().map(|o| Item { occ: o, source: Source::Holiday(c) }));
        }
    }
    let (ovs, errs) = overlay_events(root, s, tz);
    v.errors.extend(errs);
    for (id, evs) in ovs {
        v.items.extend(evs.iter().flat_map(|e| events::expand(e, from, to, tz)).map(|o| Item { occ: o, source: Source::Overlay(id.clone()) }));
    }
    v.items.extend(extra.iter().flat_map(|e| events::expand(e, from, to, tz)).map(|o| Item { occ: o, source: Source::Vcity }));
    let mut occ: Vec<(Occurrence, Source)> = v.items.drain(..).map(|i| (i.occ, i.source)).collect();
    occ.sort_by(|(a, sa), (b, sb)| {
        a.first_day
            .cmp(&b.first_day)
            .then(b.all_day().cmp(&a.all_day()))
            .then_with(|| a.start.instant(tz).cmp(&b.start.instant(tz)))
            .then_with(|| sa.cmp(sb))
            .then_with(|| a.event.id.cmp(&b.event.id))
    });
    v.items = occ.into_iter().map(|(occ, source)| Item { occ, source }).collect();
    v
}

/// The items visible with the user's filters: sources not hidden and, with `entity`, only the
/// ones about that entity.
pub fn visible<'a>(v: &'a View, s: &Settings, entity: Option<uuid::Uuid>) -> Vec<&'a Item> {
    v.items.iter().filter(|i| s.shows(&i.source.key())).filter(|i| entity.is_none_or(|id| i.occ.event.entities.contains(&id))).collect()
}

/// Items covering day `d`.
pub fn on_day<'a>(items: &[&'a Item], d: NaiveDate) -> Vec<&'a Item> {
    items.iter().copied().filter(|i| i.occ.covers(d)).collect()
}

/// Every app's events (rules kept) as one calendar; busy-only ones as `busy_label`.
/// `DTSTAMP` is the newest feed's `updatedAt`, so unchanged feeds give identical bytes.
pub fn combined(root: &Root, name: &str, busy_label: &str) -> String {
    let f = events::feeds(root);
    let stamp = f.feeds.iter().filter_map(|x| DateTime::parse_from_rfc3339(&x.updated_at).ok()).map(|t| t.with_timezone(&Utc)).max().unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    let evs: Vec<Event> = f.feeds.into_iter().flat_map(|x| x.events).collect();
    ics::write(&evs, &ics::Options { name: name.into(), product: "Otherwise Apps".into(), busy_label: busy_label.into(), stamp, refresh_hours: 1 })
}

pub fn combined_path(root: &Root) -> PathBuf {
    dir(root).join(COMBINED)
}

/// Writes `otherwise.ics` when its content changed. Returns its path.
pub fn write_combined(root: &Root, name: &str, busy_label: &str) -> std::io::Result<PathBuf> {
    let p = combined_path(root);
    let text = combined(root, name, busy_label);
    if std::fs::read_to_string(&p).ok().as_deref() != Some(text.as_str()) {
        otherwise_link::fsutil::write_atomic(&p, text.as_bytes())?;
    }
    Ok(p)
}

/// `file:///…` for a path (percent-encoded), for "Copy ICS link".
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) || (b == b':' && cfg!(windows)) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum OverlayError {
    NotCalendar,
    Read(String),
}

/// Adds an overlay from `source` (a path, or a URL read with `fetch`). The text is checked and
/// copied under `overlays/`.
pub fn add_overlay(root: &Root, s: &mut Settings, source: &str, fetch: &dyn Fn(&str) -> Result<String, String>, now: i64) -> Result<Overlay, OverlayError> {
    let source = source.trim().to_string();
    let text = read_source(&source, fetch)?;
    let name = ics::parse(&text, &Utc).name.unwrap_or_else(|| fallback_name(&source));
    let id = format!("o{now}{}", s.overlays.len());
    let o = Overlay { id, name, source, enabled: true, fetched: Some(now) };
    store_copy(root, &o, &text)?;
    s.overlays.push(o.clone());
    s.save(root).map_err(|e| OverlayError::Read(e.to_string()))?;
    Ok(o)
}

/// Reads an overlay's source again and replaces its copy (the old copy stays on failure).
pub fn refresh_overlay(root: &Root, s: &mut Settings, id: &str, fetch: &dyn Fn(&str) -> Result<String, String>, now: i64) -> Result<(), OverlayError> {
    let Some(o) = s.overlays.iter_mut().find(|o| o.id == id) else { return Err(OverlayError::Read("missing".into())) };
    let text = read_source(&o.source, fetch)?;
    o.fetched = Some(now);
    let o = o.clone();
    store_copy(root, &o, &text)?;
    s.save(root).map_err(|e| OverlayError::Read(e.to_string()))
}

pub fn remove_overlay(root: &Root, s: &mut Settings, id: &str) -> std::io::Result<()> {
    if let Some(o) = s.overlays.iter().find(|o| o.id == id) {
        let _ = std::fs::remove_file(o.copy_path(root));
    }
    s.overlays.retain(|o| o.id != id);
    s.save(root)
}

fn read_source(source: &str, fetch: &dyn Fn(&str) -> Result<String, String>) -> Result<String, OverlayError> {
    let text = if is_url(source) {
        fetch(&ics::fetch_url(source)).map_err(OverlayError::Read)?
    } else {
        let p = Path::new(source);
        match std::fs::metadata(p) {
            Ok(m) if m.len() > MAX_OVERLAY => return Err(OverlayError::Read("too large".into())),
            Ok(_) => {}
            Err(e) => return Err(OverlayError::Read(e.to_string())),
        }
        std::fs::read_to_string(p).map_err(|e| OverlayError::Read(e.to_string()))?
    };
    if !ics::looks_like_ics(&text) {
        return Err(OverlayError::NotCalendar);
    }
    Ok(text)
}

fn store_copy(root: &Root, o: &Overlay, text: &str) -> Result<(), OverlayError> {
    otherwise_link::fsutil::write_atomic(&o.copy_path(root), text.as_bytes()).map_err(|e| OverlayError::Read(e.to_string()))
}

fn fallback_name(source: &str) -> String {
    let t = source.trim_end_matches('/');
    let last = t.rsplit(['/', '\\']).next().unwrap_or(t);
    last.trim_end_matches(".ics").to_string()
}

/// A plain-text summary of `items` for the AI panel (busy-only ones redacted).
pub fn agent_lines(items: &[&Item], busy_label: &str, limit: usize) -> Vec<String> {
    items
        .iter()
        .take(limit)
        .map(|i| {
            let o = i.occ.redacted(busy_label);
            let when = if o.all_day() {
                if o.last_day > o.first_day { format!("{}..{}", o.first_day, o.last_day) } else { o.first_day.to_string() }
            } else {
                o.start.to_string_value()
            };
            let link = o.event.link.as_deref().map(|l| format!(" <{l}>")).unwrap_or_default();
            format!("{when} · {} · {} · {}{link}", i.source.key(), o.event.kind.key(), o.event.title)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use otherwise_link::events::Kind;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn merges_apps_holidays_overlays_and_extra() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::at(dir.path());
        events::publish(&root, "vigia", &[Event::day("a", Kind::Due, "SOAT", d("2026-12-08")).link(&otherwise_link::Link::new("vigia").segment("item").segment("1")).entity(uuid::Uuid::nil())], None).unwrap();
        events::publish(&root, "expense", &[Event::day("p", Kind::Payment, "Rent 4.500.000", d("2026-12-01")).rrule("FREQ=MONTHLY").busy_only(true)], None).unwrap();
        let mut s = Settings::default();
        let ics_file = dir.path().join("team.ics");
        std::fs::write(&ics_file, "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Equipo\r\nBEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Offsite\r\nURL:file:///etc/passwd\r\nDTSTART;VALUE=DATE:20261210\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").unwrap();
        let no_net = |_: &str| -> Result<String, String> { Err("offline".into()) };
        let o = add_overlay(&root, &mut s, ics_file.to_str().unwrap(), &no_net, 100).unwrap();
        assert_eq!(o.name, "Equipo");
        assert_eq!(add_overlay(&root, &mut s, "https://example.com/a.ics", &no_net, 101), Err(OverlayError::Read("offline".into())));
        std::fs::write(dir.path().join("not.ics"), "<html>").unwrap();
        assert_eq!(add_overlay(&root, &mut s, dir.path().join("not.ics").to_str().unwrap(), &no_net, 102), Err(OverlayError::NotCalendar));
        let vcity = vec![{
            let mut e = Event::new("b1", Kind::Booking, "Demo", "2026-12-09T15:00:00Z");
            e.app = VCITY_APP.into();
            e
        }];
        let v = build(&root, &s, d("2026-12-01"), d("2026-12-31"), &Utc, &vcity);
        let keys: Vec<(String, String)> = v.items.iter().map(|i| (i.occ.first_day.to_string(), i.source.key())).collect();
        assert_eq!(keys, [("2026-12-01".into(), "expense".into()), ("2026-12-08".into(), "holidays".into()), ("2026-12-08".into(), "vigia".into()), ("2026-12-09".into(), "vcity".into()), ("2026-12-10".into(), "ics".into()), ("2026-12-25".into(), "holidays".into()), ("2026-12-25".into(), "holidays".into())]);
        assert_eq!(v.apps, ["expense", "vigia"]);
        let ov = v.items.iter().find(|i| i.source.key() == OVERLAY_APP).unwrap();
        assert_eq!(ov.target(), None, "file:// links from overlays never open");
        assert_eq!(v.items.iter().find(|i| i.source.key() == "vigia").unwrap().target(), Some("otherwise://vigia/item/1"));
        // filters
        s.toggle("holidays");
        assert_eq!(visible(&v, &s, None).len(), 4);
        assert_eq!(visible(&v, &s, Some(uuid::Uuid::nil())).len(), 1);
        s.toggle("holidays");
        assert_eq!(on_day(&visible(&v, &s, None), d("2026-12-08")).len(), 2);
        // the agent never sees busy-only details
        let lines = agent_lines(&visible(&v, &s, None), "busy", 50);
        assert!(lines[0].contains("busy") && !lines.join("\n").contains("4.500.000"), "{lines:?}");
        // overlays persist, refresh keeps the copy on failure, remove deletes it
        let back = Settings::load(&root);
        assert_eq!(back.overlays.len(), 1);
        let mut s2 = back.clone();
        std::fs::remove_file(&ics_file).unwrap();
        assert!(refresh_overlay(&root, &mut s2, &o.id, &no_net, 200).is_err());
        assert!(o.copy_path(&root).exists());
        remove_overlay(&root, &mut s2, &o.id).unwrap();
        assert!(!o.copy_path(&root).exists() && Settings::load(&root).overlays.is_empty());
    }

    #[test]
    fn combined_file_is_stable_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::at(dir.path());
        events::publish(&root, "expense", &[Event::day("p", Kind::Payment, "Rent 4.500.000", d("2026-12-01")).rrule("FREQ=MONTHLY").busy_only(true)], None).unwrap();
        events::publish(&root, "vigia", &[Event::day("a", Kind::Due, "SOAT", d("2026-12-08"))], None).unwrap();
        let p = write_combined(&root, "Otherwise", "Ocupado").unwrap();
        let a = std::fs::read_to_string(&p).unwrap();
        assert!(a.contains("SUMMARY:SOAT") && a.contains("SUMMARY:Ocupado") && !a.contains("4.500.000") && a.contains("RRULE:FREQ=MONTHLY"));
        let m1 = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_combined(&root, "Otherwise", "Ocupado").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), m1, "unchanged → not rewritten");
        let cal = ics::parse(&a, &Utc);
        assert_eq!(cal.events.len(), 2);
        assert_eq!(file_url(Path::new("/Users/a b/Otherwise Ñ/otherwise.ics")), "file:///Users/a%20b/Otherwise%20%C3%91/otherwise.ics");
    }

    #[test]
    fn settings_defaults_and_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::at(dir.path());
        let s = Settings::load(&root);
        assert_eq!(s.countries(), [Country::Co, Country::Us]);
        assert!(s.show_holidays && !s.vcity && !s.combined);
        let mut s = s;
        s.vcity_orgs.push(VcityOrg { id: "o1".into(), slug: "acme".into(), name: "Acme".into(), enabled: true });
        s.save(&root).unwrap();
        assert_eq!(Settings::load(&root), s);
        assert!(!std::fs::read_to_string(dir.path().join("otherwise-apps/calendar/settings.json")).unwrap().contains("token"));
    }
}

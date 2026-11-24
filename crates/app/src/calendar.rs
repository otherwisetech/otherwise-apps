//! The Calendar page (⌘5): every Otherwise app's events (otherwise-link `events`), holidays,
//! read-only .ics overlays and, optionally, Vcity, as a month / week / agenda view. Clicking an
//! event opens it in its app. Data and files: `otherwise_core::calendar`; Vcity:
//! `otherwise_core::vcity` with the tokens in the OS keychain (`secrets`).
use crate::app::{Launcher, Modal};
use crate::hub::Page;
use crate::theme::Tokens;
use crate::ui;
use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::switch::Switch;
use gpui_component::{IconName, Sizable as _};
use otherwise_core::calendar::{self as cal, Item, Settings, Source, View};
use otherwise_core::vcity;
use otherwise_link::Root;
use otherwise_link::events::{Event, Kind, Moment, Status};
use otherwise_link::holidays::Country;
use otherwise_link::link::Link;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Keep `otherwise.ics` current this often while the app runs.
const COMBINED_EVERY: std::time::Duration = std::time::Duration::from_secs(300);
const AGENDA_DAYS: i64 = 60;
const MONTH_CELL_ITEMS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Month,
    Week,
    Agenda,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VcityState {
    Idle,
    /// Waiting for the user to approve in the browser.
    Signing { user_code: String, approve_url: String },
    Loading,
    Error(String),
}

pub struct CalState {
    pub settings: Settings,
    pub mode: Mode,
    pub cursor: NaiveDate,
    pub view: View,
    pub loading: bool,
    pub entity: Option<Uuid>,
    pub entity_names: BTreeMap<Uuid, String>,
    pub vcity_events: Vec<Event>,
    pub vcity: VcityState,
    /// Bumped to cancel a sign-in poll.
    pub vcity_attempt: u64,
    pub overlay_input: Entity<InputState>,
    pub scroll: ScrollHandle,
    timer: bool,
}

impl CalState {
    pub fn new(root: &Root, window: &mut Window, cx: &mut Context<Launcher>) -> CalState {
        CalState {
            settings: Settings::load(root),
            mode: Mode::Month,
            cursor: Local::now().date_naive(),
            view: View::default(),
            loading: false,
            entity: None,
            entity_names: BTreeMap::new(),
            vcity_events: Vec::new(),
            vcity: VcityState::Idle,
            vcity_attempt: 0,
            overlay_input: cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t!("app.cal.overlay_placeholder"))),
            scroll: ScrollHandle::new(),
            timer: false,
        }
    }

    /// The local days the current mode shows.
    pub fn range(&self) -> (NaiveDate, NaiveDate) {
        range(self.mode, self.cursor)
    }
}

fn monday(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

pub fn range(mode: Mode, cursor: NaiveDate) -> (NaiveDate, NaiveDate) {
    match mode {
        Mode::Month => {
            let first = cursor.with_day(1).unwrap_or(cursor);
            let start = monday(first);
            (start, start + Duration::days(41))
        }
        Mode::Week => {
            let s = monday(cursor);
            (s, s + Duration::days(6))
        }
        Mode::Agenda => (cursor, cursor + Duration::days(AGENDA_DAYS - 1)),
    }
}

fn step(mode: Mode, cursor: NaiveDate, dir: i32) -> NaiveDate {
    match mode {
        Mode::Month => {
            let total = cursor.year() * 12 + cursor.month0() as i32 + dir;
            NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(cursor)
        }
        Mode::Week => cursor + Duration::days(7 * dir as i64),
        Mode::Agenda => cursor + Duration::days(AGENDA_DAYS * dir as i64),
    }
}

pub fn month_name(m: u32) -> String {
    match m {
        1 => i18n::t!("app.cal.month_1"),
        2 => i18n::t!("app.cal.month_2"),
        3 => i18n::t!("app.cal.month_3"),
        4 => i18n::t!("app.cal.month_4"),
        5 => i18n::t!("app.cal.month_5"),
        6 => i18n::t!("app.cal.month_6"),
        7 => i18n::t!("app.cal.month_7"),
        8 => i18n::t!("app.cal.month_8"),
        9 => i18n::t!("app.cal.month_9"),
        10 => i18n::t!("app.cal.month_10"),
        11 => i18n::t!("app.cal.month_11"),
        _ => i18n::t!("app.cal.month_12"),
    }
}

fn weekday_short(i: u32) -> String {
    match i {
        0 => i18n::t!("app.cal.wd_mon"),
        1 => i18n::t!("app.cal.wd_tue"),
        2 => i18n::t!("app.cal.wd_wed"),
        3 => i18n::t!("app.cal.wd_thu"),
        4 => i18n::t!("app.cal.wd_fri"),
        5 => i18n::t!("app.cal.wd_sat"),
        _ => i18n::t!("app.cal.wd_sun"),
    }
}

pub fn kind_label(k: Kind) -> String {
    match k {
        Kind::Due => i18n::t!("app.cal.kind_due"),
        Kind::Renewal => i18n::t!("app.cal.kind_renewal"),
        Kind::Reminder => i18n::t!("app.cal.kind_reminder"),
        Kind::Payment => i18n::t!("app.cal.kind_payment"),
        Kind::Payroll => i18n::t!("app.cal.kind_payroll"),
        Kind::Tax => i18n::t!("app.cal.kind_tax"),
        Kind::Close => i18n::t!("app.cal.kind_close"),
        Kind::Birthday => i18n::t!("app.cal.kind_birthday"),
        Kind::Anniversary => i18n::t!("app.cal.kind_anniversary"),
        Kind::Meeting => i18n::t!("app.cal.kind_meeting"),
        Kind::Booking => i18n::t!("app.cal.kind_booking"),
        Kind::Task => i18n::t!("app.cal.kind_task"),
        Kind::Audit => i18n::t!("app.cal.kind_audit"),
        Kind::Maintenance => i18n::t!("app.cal.kind_maintenance"),
        Kind::Warranty => i18n::t!("app.cal.kind_warranty"),
        Kind::Holiday => i18n::t!("app.cal.kind_holiday"),
        Kind::Other => i18n::t!("app.cal.kind_other"),
    }
}

pub fn country_label(c: Country) -> String {
    match c {
        Country::Co => i18n::t!("app.cal.country_co"),
        Country::Us => i18n::t!("app.cal.country_us"),
        Country::UsWy => i18n::t!("app.cal.country_us_wy"),
        Country::Es => i18n::t!("app.cal.country_es"),
        Country::Mx => i18n::t!("app.cal.country_mx"),
        Country::Br => i18n::t!("app.cal.country_br"),
        Country::Ca => i18n::t!("app.cal.country_ca"),
    }
}

/// The filter chip's name for a source key.
fn source_label(key: &str) -> String {
    match key {
        otherwise_link::events::HOLIDAYS_APP => i18n::t!("app.cal.source_holidays"),
        cal::OVERLAY_APP => i18n::t!("app.cal.source_overlays"),
        cal::VCITY_APP => "Vcity".to_string(), // i18n-ok: product name
        app => otherwise_link::apps::display_name(app),
    }
}

fn hex(h: u32) -> Hsla {
    rgb(h).into()
}

/// One color per app (the same in month, week and agenda); an event's own hint wins.
pub fn color(t: &Tokens, key: &str, hint: Option<&str>) -> Hsla {
    if let Some(h) = hint {
        let named = match h {
            "red" => Some(0xD0503F),
            "orange" => Some(0xE07B24),
            "yellow" => Some(0xC9A227),
            "green" => Some(0x2E8B57),
            "teal" => Some(0x1F8A8A),
            "blue" => Some(0x3B6FD1),
            "purple" => Some(0x8A56C9),
            "pink" => Some(0xC9508A),
            "gray" => Some(0x8A867E),
            _ => h.strip_prefix('#').filter(|x| x.len() == 6).and_then(|x| u32::from_str_radix(x, 16).ok()),
        };
        if let Some(c) = named {
            return hex(c);
        }
    }
    match key {
        "vigia" => hex(0x1F8A8A),
        "expense" => hex(0xE07B24),
        "genealogy" => hex(0x2E8B57),
        "brujula" => hex(0x8A56C9),
        "helio" => hex(0xC9A227),
        "nodo" => hex(0x3B6FD1),
        cal::VCITY_APP => hex(0x5B5BD6),
        cal::OVERLAY_APP => hex(0x8A867E),
        otherwise_link::events::HOLIDAYS_APP => t.danger,
        other => {
            let palette = [0xC9508A, 0x4F7CAC, 0x7A9A3A, 0xB5651D, 0x6D5BA8];
            let n = other.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
            hex(palette[n % palette.len()])
        }
    }
}

fn time_label(i: &Item) -> Option<String> {
    match i.occ.start {
        Moment::At(t) => Some(t.with_timezone(&Local).format("%H:%M").to_string()),
        Moment::Day(_) => None,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn vcity_base(s: &Settings) -> String {
    s.vcity_base.clone().unwrap_or_else(|| vcity::DEFAULT_BASE.to_string())
}

fn vcity_error(e: &vcity::Error) -> String {
    match e {
        vcity::Error::SignedOut => i18n::t!("app.cal.vcity_signed_out"),
        vcity::Error::UnsafeBase => i18n::t!("app.cal.vcity_unsafe"),
        vcity::Error::Status(c) => i18n::t!("app.cal.vcity_status", code = c),
        vcity::Error::Transport(d) => i18n::t!("app.cal.vcity_network", detail = d),
        vcity::Error::Invalid(w) => i18n::t!("app.cal.vcity_invalid", detail = w),
    }
}

impl Launcher {
    // ── loading ─────────────────────────────────────────────────────────────────────────────

    pub fn reload_calendar(&mut self, cx: &mut Context<Self>) {
        self.hub.root = Root::system();
        let root = self.hub.root.clone();
        self.cal.settings = Settings::load(&root);
        let settings = self.cal.settings.clone();
        let (from, to) = self.cal.range();
        let extra = self.cal.vcity_events.clone();
        let registry = self.hub.settings.entities;
        let busy = i18n::t!("app.cal.busy");
        let name = i18n::t!("app.cal.ics_name");
        self.cal.loading = true;
        let task = cx.background_executor().spawn(async move {
            let view = cal::build(&root, &settings, from, to, &Local, &extra);
            // names for the entity filter, only when the user turned the registry on
            let mut names = BTreeMap::new();
            if registry && let Ok(reg) = otherwise_link::registry::Registry::open_shared(&root) {
                let ids: std::collections::BTreeSet<Uuid> = view.items.iter().flat_map(|i| i.occ.event.entities.iter().copied()).collect();
                for id in ids.into_iter().take(200) {
                    if let Ok(Some(e)) = reg.get(id) {
                        names.insert(id, e.name);
                    }
                }
            }
            if settings.combined {
                let _ = cal::write_combined(&root, &name, &busy);
            }
            (view, names)
        });
        cx.spawn(async move |this, cx| {
            let (view, names) = task.await;
            this.update(cx, |this, cx| {
                this.cal.view = view;
                this.cal.entity_names = names;
                this.cal.loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.start_calendar_timer(cx);
    }

    /// Keeps `otherwise.ics` current while the app runs (apps rewrite their feeds any time).
    fn start_calendar_timer(&mut self, cx: &mut Context<Self>) {
        if self.cal.timer {
            return;
        }
        self.cal.timer = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(COMBINED_EVERY).await;
                let job = this.update(cx, |this, _| (this.cal.settings.combined, this.hub.root.clone()));
                let Ok((on, root)) = job else { break };
                if on {
                    let (busy, name) = (i18n::t!("app.cal.busy"), i18n::t!("app.cal.ics_name"));
                    cx.background_executor().spawn(async move { cal::write_combined(&root, &name, &busy) }).await.ok();
                }
            }
        })
        .detach();
    }

    fn save_cal_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.cal.settings.save(&self.hub.root) {
            self.show_toast(i18n::t!("app.cal.save_error", detail = e), cx);
        }
    }

    fn set_cal_mode(&mut self, m: Mode, cx: &mut Context<Self>) {
        self.cal.mode = m;
        self.reload_calendar(cx);
        self.reload_vcity(cx);
    }

    fn move_cal(&mut self, dir: i32, cx: &mut Context<Self>) {
        self.cal.cursor = if dir == 0 { Local::now().date_naive() } else { step(self.cal.mode, self.cal.cursor, dir) };
        self.reload_calendar(cx);
        self.reload_vcity(cx);
    }

    fn toggle_source(&mut self, key: &str, cx: &mut Context<Self>) {
        self.cal.settings.toggle(key);
        self.save_cal_settings(cx);
        cx.notify();
    }

    /// Opens an event in its app (or a web source in the browser).
    pub fn open_item(&mut self, target: &str, cx: &mut Context<Self>) {
        if target.starts_with("https://") || target.starts_with("http://") {
            cx.open_url(target);
            return;
        }
        match Link::parse(target) {
            Ok(l) => self.open_link(&l, cx),
            Err(_) => self.show_toast(i18n::t!("app.cal.bad_link"), cx),
        }
    }

    // ── combined ICS ────────────────────────────────────────────────────────────────────────

    /// "Copy ICS link": turns the combined file on, writes it and copies its file:// URL.
    fn copy_ics_link(&mut self, cx: &mut Context<Self>) {
        self.cal.settings.combined = true;
        self.save_cal_settings(cx);
        let root = self.hub.root.clone();
        let (busy, name) = (i18n::t!("app.cal.busy"), i18n::t!("app.cal.ics_name"));
        let task = cx.background_executor().spawn(async move { cal::write_combined(&root, &name, &busy) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| match res {
                Ok(p) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(cal::file_url(&p)));
                    this.modal = Some(Modal::Info { title: i18n::t!("app.cal.ics_copied_title"), body: i18n::t!("app.cal.ics_copied_body", path = p.display()) });
                    cx.notify();
                }
                Err(e) => this.show_toast(i18n::t!("app.cal.save_error", detail = e), cx),
            })
            .ok();
        })
        .detach();
    }

    fn export_ics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = otherwise_link::paths::home().join("Downloads");
        let dir = if dir.is_dir() { dir } else { otherwise_link::paths::home() };
        let rx = cx.prompt_for_new_path(&dir, Some("otherwise.ics")); // i18n-ok: file name
        let root = self.hub.root.clone();
        let (busy, name) = (i18n::t!("app.cal.busy"), i18n::t!("app.cal.ics_name"));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(p))) = rx.await {
                let text = cal::combined(&root, &name, &busy);
                let res = otherwise_link::fsutil::write_atomic(&p, text.as_bytes());
                this.update(cx, |this, cx| match res {
                    Ok(()) => this.show_toast(i18n::t!("app.cal.exported", path = p.display()), cx),
                    Err(e) => this.show_toast(i18n::t!("app.cal.save_error", detail = e), cx),
                })
                .ok();
            }
        })
        .detach();
    }

    fn set_combined(&mut self, on: bool, cx: &mut Context<Self>) {
        self.cal.settings.combined = on;
        self.save_cal_settings(cx);
        if on {
            self.reload_calendar(cx);
        }
        cx.notify();
    }

    // ── overlays ────────────────────────────────────────────────────────────────────────────

    fn add_overlay(&mut self, source: String, cx: &mut Context<Self>) {
        let source = source.trim().to_string();
        if source.is_empty() {
            return;
        }
        let root = self.hub.root.clone();
        let mut s = self.cal.settings.clone();
        let task = cx.background_executor().spawn(async move {
            let res = cal::add_overlay(&root, &mut s, &source, &|u| otherwise_core::net::get_calendar(u).map_err(|e| e.to_string()), chrono::Utc::now().timestamp());
            (res, s)
        });
        cx.spawn(async move |this, cx| {
            let (res, s) = task.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(o) => {
                        this.cal.settings = s;
                        this.show_toast(i18n::t!("app.cal.overlay_added", name = o.name), cx);
                        this.reload_calendar(cx);
                    }
                    Err(cal::OverlayError::NotCalendar) => this.show_toast(i18n::t!("app.cal.overlay_not_ics"), cx),
                    Err(cal::OverlayError::Read(e)) => this.show_toast(i18n::t!("app.cal.overlay_error", detail = e), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn pick_overlay_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(i18n::t!("app.cal.overlay_pick").into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.add_overlay(p.display().to_string(), cx)).ok();
            }
        })
        .detach();
    }

    fn refresh_overlay(&mut self, id: String, cx: &mut Context<Self>) {
        let root = self.hub.root.clone();
        let mut s = self.cal.settings.clone();
        let task = cx.background_executor().spawn(async move {
            let res = cal::refresh_overlay(&root, &mut s, &id, &|u| otherwise_core::net::get_calendar(u).map_err(|e| e.to_string()), chrono::Utc::now().timestamp());
            (res, s)
        });
        cx.spawn(async move |this, cx| {
            let (res, s) = task.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.cal.settings = s;
                        this.show_toast(i18n::t!("app.cal.overlay_refreshed"), cx);
                        this.reload_calendar(cx);
                    }
                    Err(cal::OverlayError::NotCalendar) => this.show_toast(i18n::t!("app.cal.overlay_not_ics"), cx),
                    Err(cal::OverlayError::Read(e)) => this.show_toast(i18n::t!("app.cal.overlay_error", detail = e), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_overlay(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut s = self.cal.settings.clone();
        if cal::remove_overlay(&self.hub.root, &mut s, id).is_ok() {
            self.cal.settings = s;
        }
        self.reload_calendar(cx);
    }

    fn toggle_overlay(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(o) = self.cal.settings.overlays.iter_mut().find(|o| o.id == id) {
            o.enabled = on;
        }
        self.save_cal_settings(cx);
        self.reload_calendar(cx);
    }

    fn toggle_country(&mut self, c: Country, cx: &mut Context<Self>) {
        let id = c.id().to_string();
        if self.cal.settings.holidays.contains(&id) {
            self.cal.settings.holidays.retain(|x| *x != id);
        } else {
            self.cal.settings.holidays.push(id);
        }
        self.save_cal_settings(cx);
        self.reload_calendar(cx);
    }

    fn set_show_holidays(&mut self, on: bool, cx: &mut Context<Self>) {
        self.cal.settings.show_holidays = on;
        self.save_cal_settings(cx);
        self.reload_calendar(cx);
    }

    // ── Vcity ───────────────────────────────────────────────────────────────────────────────

    /// "Conectar Vcity": the device-code sign-in of the `vcity` CLI, in the browser.
    pub fn connect_vcity(&mut self, cx: &mut Context<Self>) {
        let base = vcity_base(&self.cal.settings);
        self.cal.vcity_attempt += 1;
        let attempt = self.cal.vcity_attempt;
        self.cal.vcity = VcityState::Loading;
        cx.notify();
        let task = cx.background_executor().spawn({
            let base = base.clone();
            async move { vcity::start_device(&base) }
        });
        cx.spawn(async move |this, cx| {
            let dc = match task.await {
                Ok(dc) => dc,
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.cal.vcity = VcityState::Error(vcity_error(&e));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let url = dc.approve_url.clone();
            let still = this
                .update(cx, |this, cx| {
                    this.cal.vcity = VcityState::Signing { user_code: dc.user_code.clone(), approve_url: url.clone() };
                    cx.open_url(&url);
                    cx.notify();
                })
                .is_ok();
            if !still {
                return;
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(dc.expires_in);
            let mut interval = dc.interval;
            let outcome: Result<vcity::Tokens, String> = loop {
                cx.background_executor().timer(std::time::Duration::from_secs(interval)).await;
                let current = this.update(cx, |this, _| this.cal.vcity_attempt).unwrap_or(0);
                if current != attempt {
                    return;
                }
                if std::time::Instant::now() > deadline {
                    break Err(i18n::t!("app.cal.vcity_expired"));
                }
                let (b, code) = (base.clone(), dc.device_code.clone());
                match cx.background_executor().spawn(async move { vcity::poll(&b, &code, now_ms()) }).await {
                    Ok(vcity::Poll::Pending { interval: i }) => interval = i,
                    Ok(vcity::Poll::Approved(t)) => break Ok(t),
                    Ok(vcity::Poll::Denied) => break Err(i18n::t!("app.cal.vcity_denied")),
                    Ok(vcity::Poll::Expired) => break Err(i18n::t!("app.cal.vcity_expired")),
                    // network blips while the user approves: keep polling until the deadline
                    Err(vcity::Error::Transport(_)) => {}
                    Err(e) => break Err(vcity_error(&e)),
                }
            };
            let tokens = match outcome {
                Ok(t) => t,
                Err(msg) => {
                    this.update(cx, |this, cx| {
                        this.cal.vcity = VcityState::Error(msg);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let b = base.clone();
            let saved = cx
                .background_executor()
                .spawn(async move {
                    crate::secrets::set(vcity::KEYCHAIN_SERVICE, &vcity::account(&b), &tokens.to_secret())?;
                    let mut s = vcity::Session::new(&b, tokens).map_err(|e| vcity_error(&e))?;
                    let orgs = s.orgs(now_ms()).map_err(|e| vcity_error(&e))?;
                    if s.changed {
                        crate::secrets::set(vcity::KEYCHAIN_SERVICE, &vcity::account(&b), &s.tokens.to_secret())?;
                    }
                    Ok::<_, String>(orgs)
                })
                .await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(orgs) => {
                        this.cal.settings.vcity = true;
                        this.cal.settings.vcity_orgs = orgs.into_iter().map(|o| cal::VcityOrg { id: o.id, slug: o.slug, name: o.name, enabled: true }).collect();
                        this.save_cal_settings(cx);
                        this.cal.vcity = VcityState::Idle;
                        this.show_toast(i18n::t!("app.cal.vcity_connected"), cx);
                        this.reload_vcity(cx);
                    }
                    Err(e) => this.cal.vcity = VcityState::Error(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn cancel_vcity(&mut self, cx: &mut Context<Self>) {
        self.cal.vcity_attempt += 1;
        self.cal.vcity = VcityState::Idle;
        cx.notify();
    }

    pub fn disconnect_vcity(&mut self, cx: &mut Context<Self>) {
        let account = vcity::account(&vcity_base(&self.cal.settings));
        cx.background_executor().spawn(async move { crate::secrets::set(vcity::KEYCHAIN_SERVICE, &account, "") }).detach();
        self.cal.settings.vcity = false;
        self.cal.settings.vcity_orgs.clear();
        self.cal.vcity_events.clear();
        self.cal.vcity = VcityState::Idle;
        self.save_cal_settings(cx);
        self.reload_calendar(cx);
    }

    pub fn toggle_vcity_org(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(o) = self.cal.settings.vcity_orgs.iter_mut().find(|o| o.id == id) {
            o.enabled = on;
        }
        self.save_cal_settings(cx);
        self.reload_vcity(cx);
    }

    /// Reads the visible range from Vcity (only when signed in), then redraws.
    pub fn reload_vcity(&mut self, cx: &mut Context<Self>) {
        if !self.cal.settings.vcity || matches!(self.cal.vcity, VcityState::Signing { .. }) {
            return;
        }
        let base = vcity_base(&self.cal.settings);
        let orgs: Vec<vcity::Org> = self.cal.settings.vcity_orgs.iter().filter(|o| o.enabled).map(|o| vcity::Org { id: o.id.clone(), slug: o.slug.clone(), name: o.name.clone() }).collect();
        let (from, to) = self.cal.range();
        self.cal.vcity = VcityState::Loading;
        let task = cx.background_executor().spawn(async move {
            let account = vcity::account(&base);
            let secret = crate::secrets::get(vcity::KEYCHAIN_SERVICE, &account).ok_or_else(|| i18n::t!("app.cal.vcity_signed_out"))?;
            let tokens = vcity::Tokens::from_secret(&secret).ok_or_else(|| i18n::t!("app.cal.vcity_signed_out"))?;
            let mut s = vcity::Session::new(&base, tokens).map_err(|e| vcity_error(&e))?;
            let res = s.calendar(&orgs, from, to, &Local, now_ms());
            if s.changed {
                let _ = crate::secrets::set(vcity::KEYCHAIN_SERVICE, &account, &s.tokens.to_secret());
            }
            res.map_err(|e| vcity_error(&e))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(evs) => {
                        this.cal.vcity_events = evs;
                        this.cal.vcity = VcityState::Idle;
                        this.reload_calendar(cx);
                    }
                    Err(e) => this.cal.vcity = VcityState::Error(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── AI panel ────────────────────────────────────────────────────────────────────────────

    /// The visible range and its events for the agent (busy-only ones without details).
    pub fn calendar_context(&self) -> serde_json::Value {
        let (from, to) = self.cal.range();
        let items = cal::visible(&self.cal.view, &self.cal.settings, self.cal.entity);
        let mode = match self.cal.mode {
            Mode::Month => "month",
            Mode::Week => "week",
            Mode::Agenda => "agenda",
        };
        serde_json::json!({
            "page": "Calendar (every Otherwise app's dates: due dates, payments, birthdays, audits; plus holidays and overlays)",
            "mode": mode,
            "today": Local::now().date_naive().to_string(),
            "visible_range": {"from": from.to_string(), "to": to.to_string()},
            "events": cal::agent_lines(&items, "busy", 120),
            "more_with_tool": "calendar_events(from, to) on the otherwise hub",
        })
    }

    // ── rendering ───────────────────────────────────────────────────────────────────────────

    fn title_label(&self) -> String {
        let (from, to) = self.cal.range();
        match self.cal.mode {
            Mode::Month => i18n::t!("app.cal.title_month", month = month_name(self.cal.cursor.month()), year = self.cal.cursor.year()),
            _ => i18n::t!("app.cal.title_range", from_day = from.day(), from_month = month_name(from.month()), to_day = to.day(), to_month = month_name(to.month()), year = to.year()),
        }
    }

    fn pill(&self, t: &Tokens, i: &Item, compact: bool, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let e = &i.occ.event;
        let key = i.source.key();
        let c = color(t, &key, e.color.as_deref());
        let done = e.status == Status::Done;
        let tentative = e.status == Status::Tentative;
        let target = i.target().map(str::to_string);
        let hover = t.raised;
        let title = if e.title.is_empty() { kind_label(e.kind) } else { e.title.clone() };
        let time = time_label(i);
        let tip = format!("{} · {} · {}", source_label(&key), kind_label(e.kind), title);
        div()
            .id(SharedString::from(format!("pill-{}-{n}", i.occ.key()))) // i18n-ok: element id
            .flex()
            .items_center()
            .gap(px(5.))
            .min_w_0()
            .px(px(5.))
            .py(px(if compact { 1. } else { 4. }))
            .rounded(px(5.))
            .bg(c.opacity(if tentative { 0.06 } else { 0.13 }))
            .when(target.is_some(), |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
            .when_some(target, |d, l| d.on_click(cx.listener(move |this, _, _, cx| this.open_item(&l, cx))))
            .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(div().flex_none().w(px(3.)).h(px(if compact { 12. } else { 16. })).rounded(px(2.)).bg(c))
            .children(time.map(|x| div().flex_none().text_size(px(10.)).text_color(t.ink_muted).child(x)))
            .child(div().flex_1().min_w_0().truncate().text_size(px(if compact { 11. } else { 12. })).when(done, |d| d.line_through().text_color(t.ink_muted)).child(title))
            .when(e.busy_only, |d| d.child(ui::icon(IconName::EyeOff, 10., t.ink_muted)))
            .into_any_element()
    }

    fn render_month(&self, t: &Tokens, items: &[&Item], cx: &mut Context<Self>) -> AnyElement {
        let (from, _) = self.cal.range();
        let today = Local::now().date_naive();
        let month = self.cal.cursor.month();
        let mut grid = div().flex().flex_col().flex_1().border_1().border_color(t.border).rounded(px(10.)).overflow_hidden();
        let mut head = div().flex().bg(t.raised).border_b_1().border_color(t.border);
        for i in 0..7 {
            head = head.child(div().flex_1().px(px(8.)).py(px(6.)).text_size(px(11.)).text_color(t.ink_muted).child(weekday_short(i)));
        }
        grid = grid.child(head);
        let mut n = 0;
        for w in 0..6 {
            let mut row = div().flex().flex_1().min_h(px(96.)).when(w < 5, |d| d.border_b_1().border_color(t.border));
            for k in 0..7 {
                let d = from + Duration::days(w * 7 + k);
                let day_items = cal::on_day(items, d);
                let other = d.month() != month;
                let is_today = d == today;
                let extra = day_items.len().saturating_sub(MONTH_CELL_ITEMS);
                let mut cell = div()
                    .id(SharedString::from(format!("cell-{d}"))) // i18n-ok: element id
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .p(px(4.))
                    .when(k < 6, |c| c.border_r_1().border_color(t.border))
                    .when(other, |c| c.bg(t.canvas.opacity(0.6)))
                    .child(
                        div()
                            .id(SharedString::from(format!("day-{d}"))) // i18n-ok: element id
                            .flex()
                            .justify_center()
                            .items_center()
                            .size(px(22.))
                            .rounded_full()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .when(is_today, |x| x.bg(t.accent).text_color(t.on_fill()))
                            .when(!is_today && other, |x| x.text_color(t.ink_muted))
                            .child(d.day().to_string())
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.cal.open_week")).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cal.cursor = d;
                                this.set_cal_mode(Mode::Week, cx);
                            })),
                    );
                for i in day_items.iter().take(MONTH_CELL_ITEMS) {
                    n += 1;
                    cell = cell.child(self.pill(t, i, true, n, cx));
                }
                if extra > 0 {
                    cell = cell.child(
                        div()
                            .id(SharedString::from(format!("more-{d}"))) // i18n-ok: element id
                            .px(px(5.))
                            .text_size(px(10.))
                            .text_color(t.ink_muted)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(i18n::t!("app.cal.more", n = extra))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cal.cursor = d;
                                this.set_cal_mode(Mode::Week, cx);
                            })),
                    );
                }
                row = row.child(cell);
            }
            grid = grid.child(row);
        }
        grid.into_any_element()
    }

    fn render_week(&self, t: &Tokens, items: &[&Item], cx: &mut Context<Self>) -> AnyElement {
        let (from, _) = self.cal.range();
        let today = Local::now().date_naive();
        let mut cols = div().flex().flex_1().border_1().border_color(t.border).rounded(px(10.)).overflow_hidden().min_h(px(360.));
        let mut n = 0;
        for k in 0..7 {
            let d = from + Duration::days(k);
            let day_items = cal::on_day(items, d);
            let mut col = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(4.))
                .p(px(6.))
                .when(k < 6, |c| c.border_r_1().border_color(t.border))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .pb(px(4.))
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(11.)).text_color(t.ink_muted).child(weekday_short(k as u32)))
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).when(d == today, |x| x.text_color(t.accent)).child(d.day().to_string())),
                );
            if day_items.is_empty() {
                col = col.child(div().text_size(px(11.)).text_color(t.ink_muted.opacity(0.6)).child("·")); // i18n-ok: symbol
            }
            for i in day_items {
                n += 1;
                col = col.child(self.pill(t, i, false, n, cx));
            }
            cols = cols.child(col);
        }
        cols.into_any_element()
    }

    fn render_agenda(&self, t: &Tokens, items: &[&Item], cx: &mut Context<Self>) -> AnyElement {
        let (from, to) = self.cal.range();
        let today = Local::now().date_naive();
        let mut list = div().flex().flex_col().gap(px(14.));
        let mut any = false;
        let mut n = 0;
        let mut d = from;
        while d <= to {
            let day_items = cal::on_day(items, d);
            if !day_items.is_empty() {
                any = true;
                let label = if d == today { i18n::t!("app.today.today") } else { crate::hub::date_label(d) };
                let mut sec = div().flex().flex_col().gap(px(4.)).child(ui::section_title(t, format!("{label} · {}", day_items.len())));
                for i in day_items {
                    n += 1;
                    let e = &i.occ.event;
                    let key = i.source.key();
                    let c = color(t, &key, e.color.as_deref());
                    let target = i.target().map(str::to_string);
                    let hover = t.raised;
                    let done = e.status == Status::Done;
                    sec = sec.child(
                        div()
                            .id(SharedString::from(format!("ag-{}-{n}", i.occ.key()))) // i18n-ok: element id
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(8.))
                            .rounded(px(9.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.surface)
                            .when(target.is_some(), |x| x.cursor_pointer().hover(move |s| s.bg(hover)))
                            .when_some(target, |x, l| x.on_click(cx.listener(move |this, _, _, cx| this.open_item(&l, cx))))
                            .child(div().flex_none().w(px(4.)).h(px(28.)).rounded(px(2.)).bg(c))
                            .child(div().flex_none().w(px(44.)).text_size(px(12.)).text_color(t.ink_muted).child(time_label(i).unwrap_or_else(|| i18n::t!("app.cal.all_day"))))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).truncate().when(done, |x| x.line_through().text_color(t.ink_muted)).child(if e.title.is_empty() { kind_label(e.kind) } else { e.title.clone() }))
                                    .children(e.body.clone().filter(|b| !b.is_empty()).map(|b| div().text_size(px(11.)).text_color(t.ink_muted).truncate().child(b))),
                            )
                            .when(e.busy_only, |x| x.child(ui::icon(IconName::EyeOff, 12., t.ink_muted)))
                            .child(ui::chip(kind_label(e.kind), t.ink_muted, t.border.opacity(0.5)))
                            .child(ui::chip(e.category.clone().filter(|_| matches!(i.source, Source::Overlay(_) | Source::Vcity)).unwrap_or_else(|| source_label(&key)), c, c.opacity(0.12))),
                    );
                }
                list = list.child(sec);
            }
            d += Duration::days(1);
        }
        if !any {
            list = list.child(div().py(px(40.)).text_center().text_size(px(13.)).text_color(t.ink_muted).child(i18n::t!("app.cal.empty")));
        }
        list.into_any_element()
    }

    fn render_filters(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mut keys: Vec<String> = self.cal.view.apps.clone();
        let mut present: Vec<String> = self.cal.view.items.iter().map(|i| i.source.key()).collect();
        present.sort();
        present.dedup();
        for k in present {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
        for (n, k) in keys.into_iter().enumerate() {
            let on = self.cal.settings.shows(&k);
            let c = color(t, &k, None);
            let key = k.clone();
            row = row.child(
                div()
                    .id(("cal-src", n))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(9.))
                    .py(px(4.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(if on { c.opacity(0.5) } else { t.border })
                    .bg(if on { c.opacity(0.10) } else { t.surface })
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(if on { t.ink } else { t.ink_muted })
                    .child(div().size(px(8.)).rounded_full().bg(if on { c } else { t.border_strong }))
                    .child(source_label(&k))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_source(&key, cx))),
            );
        }
        // entity filter: only when the registry is on and the range has events about entities
        if !self.cal.entity_names.is_empty() {
            let current = self.cal.entity.and_then(|id| self.cal.entity_names.get(&id).cloned()).unwrap_or_else(|| i18n::t!("app.cal.all_entities"));
            let names: Vec<(Uuid, String)> = self.cal.entity_names.iter().map(|(k, v)| (*k, v.clone())).collect();
            let selected = self.cal.entity;
            let weak = cx.entity().downgrade();
            row = row.child(Button::new("cal-entity").ghost().with_size(px(26.)).label(current).icon(IconName::Search).dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                let w = weak.clone();
                menu = menu.item(PopupMenuItem::new(i18n::t!("app.cal.all_entities")).checked(selected.is_none()).on_click(move |_, _, cx| {
                    w.update(cx, |this, cx| {
                        this.cal.entity = None;
                        cx.notify();
                    })
                    .ok();
                }));
                for (id, name) in &names {
                    let w = weak.clone();
                    let id = *id;
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(selected == Some(id)).on_click(move |_, _, cx| {
                        w.update(cx, |this, cx| {
                            this.cal.entity = Some(id);
                            cx.notify();
                        })
                        .ok();
                    }));
                }
                menu
            }));
        }
        row.into_any_element()
    }

    pub fn render_calendar(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.cal.mode;
        let mut modes = ui::segmented(t);
        for (m, label, id) in [(Mode::Month, i18n::t!("app.cal.month"), "cal-month"), (Mode::Week, i18n::t!("app.cal.week"), "cal-week"), (Mode::Agenda, i18n::t!("app.cal.agenda"), "cal-agenda")] {
            modes = modes.child(ui::segment(t, id, m == mode).child(label).on_click(cx.listener(move |this, _, _, cx| this.set_cal_mode(m, cx))));
        }
        let weak = cx.entity().downgrade();
        let ics = Button::new("cal-ics").ghost().with_size(px(32.)).label(i18n::t!("app.cal.ics")).icon(IconName::Calendar).dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
            menu.item(PopupMenuItem::new(i18n::t!("app.cal.copy_ics")).on_click(move |_, _, cx| {
                w1.update(cx, |this, cx| this.copy_ics_link(cx)).ok();
            }))
            .item(PopupMenuItem::new(i18n::t!("app.cal.export_ics")).on_click(move |_, window, cx| {
                w2.update(cx, |this, cx| this.export_ics(window, cx)).ok();
            }))
            .separator()
            .item(PopupMenuItem::new(i18n::t!("app.cal.sources")).on_click(move |_, _, cx| {
                w3.update(cx, |this, cx| {
                    this.modal = Some(Modal::CalendarSources);
                    cx.notify();
                })
                .ok();
            }))
        });
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().truncate().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(self.title_label()))
            .when(self.cal.loading || self.cal.vcity == VcityState::Loading, |d| d.child(div().text_size(px(11.)).text_color(t.ink_muted).child(i18n::t!("app.cal.loading"))))
            .child(ui::icon_button(t, "cal-prev", true).child(ui::icon(IconName::ChevronLeft, 15., t.ink)).on_click(cx.listener(|this, _, _, cx| this.move_cal(-1, cx))))
            .child(ui::outline_button(t, "cal-today").child(i18n::t!("app.cal.today")).on_click(cx.listener(|this, _, _, cx| this.move_cal(0, cx))))
            .child(ui::icon_button(t, "cal-next", true).child(ui::icon(IconName::ChevronRight, 15., t.ink)).on_click(cx.listener(|this, _, _, cx| this.move_cal(1, cx))))
            .child(modes)
            .child(ics)
            .child(ui::icon_button(t, "cal-sources", true).child(ui::icon(IconName::Settings, 15., t.ink)).tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.cal.sources")).build(window, cx)).on_click(cx.listener(|this, _, _, cx| {
                this.modal = Some(Modal::CalendarSources);
                cx.notify();
            })))
            .child(ui::icon_button(t, "cal-refresh", true).child(ui::glyph(ui::REFRESH, 15., t.ink)).on_click(cx.listener(|this, _, _, cx| {
                this.reload_calendar(cx);
                this.reload_vcity(cx);
            })));
        let items = cal::visible(&self.cal.view, &self.cal.settings, self.cal.entity);
        let body = match mode {
            Mode::Month => self.render_month(t, &items, cx),
            Mode::Week => self.render_week(t, &items, cx),
            Mode::Agenda => self.render_agenda(t, &items, cx),
        };
        let mut notes: Vec<String> = self.cal.view.errors.iter().map(|(f, e)| format!("{f}: {e}")).collect();
        if let VcityState::Error(e) = &self.cal.vcity {
            notes.push(format!("Vcity: {e}")); // i18n-ok: product name prefix
        }
        let empty_feeds = self.cal.view.apps.is_empty() && !self.cal.loading;
        div()
            .id("cal-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.cal.scroll)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(20.))
                    .min_h_full()
                    .child(header)
                    .child(self.render_filters(t, cx))
                    .when(!notes.is_empty(), |d| d.child(div().text_size(px(11.)).text_color(t.warn).child(i18n::t!("app.cal.errors", detail = notes.join("; ")))))
                    .when(empty_feeds, |d| d.child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.cal.no_feeds"))))
                    .child(body),
            )
            .into_any_element()
    }

    /// Calendar → Sources: holidays, overlays, the combined file and Vcity.
    pub fn render_calendar_sources(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let weak = cx.entity().downgrade();
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
        // holidays
        let show = Switch::new("cal-holidays").checked(self.cal.settings.show_holidays).label(i18n::t!("app.cal.holidays_label")).on_click({
            let w = weak.clone();
            move |on, _, cx| {
                w.update(cx, |this, cx| this.set_show_holidays(*on, cx)).ok();
            }
        });
        let mut countries = div().flex().flex_wrap().gap(px(6.));
        for c in Country::ALL {
            let on = self.cal.settings.holidays.iter().any(|x| x == c.id());
            countries = countries.child(ui::segment(t, SharedString::from(format!("country-{}", c.id())), on).border_1().border_color(t.border).child(country_label(c)).on_click(cx.listener(move |this, _, _, cx| this.toggle_country(c, cx)))); // i18n-ok: element id
        }
        let holidays = div().flex().flex_col().gap(px(8.)).child(show).child(countries);
        // overlays
        let mut overlays = div().flex().flex_col().gap(px(6.));
        for (n, o) in self.cal.settings.overlays.iter().enumerate() {
            let (id1, id2, id3) = (o.id.clone(), o.id.clone(), o.id.clone());
            let w = weak.clone();
            let fetched = o.fetched.and_then(|s| chrono::DateTime::from_timestamp(s, 0)).map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
            overlays = overlays.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Switch::new(("ov-on", n)).checked(o.enabled).on_click(move |on, _, cx| {
                        let id = id1.clone();
                        w.update(cx, |this, cx| this.toggle_overlay(&id, *on, cx)).ok();
                    }))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(div().text_size(px(13.)).truncate().child(o.name.clone())).child(div().text_size(px(11.)).text_color(t.ink_muted).truncate().child(i18n::t!("app.cal.overlay_meta", source = o.source.clone(), when = fetched))))
                    .child(ui::outline_button(t, ("ov-refresh", n)).child(i18n::t!("app.cal.overlay_refresh")).on_click(cx.listener(move |this, _, _, cx| this.refresh_overlay(id2.clone(), cx))))
                    .child(ui::icon_button(t, ("ov-remove", n), true).child(ui::glyph(ui::TRASH, 14., t.danger)).on_click(cx.listener(move |this, _, _, cx| this.remove_overlay(&id3, cx)))),
            );
        }
        let input = self.cal.overlay_input.clone();
        overlays = overlays.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().h(px(32.)).px(px(8.)).flex().items_center().rounded(px(9.)).border_1().border_color(t.border).bg(t.canvas).text_size(px(13.)).child(Input::new(&self.cal.overlay_input).appearance(false)))
                .child(ui::outline_button(t, "ov-add").child(i18n::t!("app.cal.overlay_add")).on_click(cx.listener(move |this, _, window, cx| {
                    let v = input.read(cx).value().to_string();
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                    this.add_overlay(v, cx);
                })))
                .child(ui::outline_button(t, "ov-file").child(i18n::t!("app.cal.overlay_file")).on_click(cx.listener(|this, _, window, cx| this.pick_overlay_file(window, cx)))),
        );
        // combined file
        let path = cal::combined_path(&self.hub.root);
        let combined = Switch::new("cal-combined").checked(self.cal.settings.combined).label(i18n::t!("app.cal.combined_label")).on_click({
            let w = weak.clone();
            move |on, _, cx| {
                w.update(cx, |this, cx| this.set_combined(*on, cx)).ok();
            }
        });
        let combined_box = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(combined)
            .child(div().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(t.canvas).border_1().border_color(t.border).text_size(px(11.)).font_family("Menlo").child(path.display().to_string())) // i18n-ok: font name
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(ui::outline_button(t, "ics-copy").child(i18n::t!("app.cal.copy_ics")).on_click(cx.listener(|this, _, _, cx| this.copy_ics_link(cx))))
                    .child(ui::outline_button(t, "ics-export").child(i18n::t!("app.cal.export_ics")).on_click(cx.listener(|this, _, window, cx| this.export_ics(window, cx)))),
            );
        div()
            .flex()
            .flex_col()
            .child(div().pb(px(8.)).text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.cal.sources")))
            .child(row(i18n::t!("app.cal.holidays"), Some(i18n::t!("app.cal.holidays_hint")), holidays.into_any_element()))
            .child(row(i18n::t!("app.cal.overlays"), Some(i18n::t!("app.cal.overlays_hint")), overlays.into_any_element()))
            .child(row(i18n::t!("app.cal.combined"), Some(i18n::t!("app.cal.combined_hint")), combined_box.into_any_element()))
            .child(row(i18n::t!("app.cal.vcity"), Some(i18n::t!("app.cal.vcity_hint")), self.render_vcity_control(t, cx).into_any_element()))
    }

    /// Settings → Hub and Calendar → Sources: connect / disconnect Vcity, pick orgs.
    pub fn render_vcity_control(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let mut d = div().flex().flex_col().gap(px(8.));
        match &self.cal.vcity {
            VcityState::Signing { user_code, approve_url } => {
                let url = approve_url.clone();
                return d
                    .child(div().text_size(px(12.)).child(i18n::t!("app.cal.vcity_approve")))
                    .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).font_family("Menlo").child(user_code.clone())) // i18n-ok: font name
                    .child(div().text_size(px(11.)).text_color(t.ink_muted).child(i18n::t!("app.cal.vcity_phishing")))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(ui::outline_button(t, "vcity-open").child(i18n::t!("app.cal.vcity_open")).on_click(move |_, _, cx| cx.open_url(&url)))
                            .child(ui::outline_button(t, "vcity-cancel").child(i18n::t!("app.action.cancel")).on_click(cx.listener(|this, _, _, cx| this.cancel_vcity(cx)))),
                    );
            }
            VcityState::Error(e) => d = d.child(div().text_size(px(11.)).text_color(t.danger).child(e.clone())),
            _ => {}
        }
        if !self.cal.settings.vcity {
            return d.child(ui::filled_button(t, "vcity-connect", t.accent).child(i18n::t!("app.cal.vcity_connect")).on_click(cx.listener(|this, _, _, cx| this.connect_vcity(cx))));
        }
        let weak = cx.entity().downgrade();
        for (n, o) in self.cal.settings.vcity_orgs.iter().enumerate() {
            let id = o.id.clone();
            let w = weak.clone();
            d = d.child(Switch::new(("vcity-org", n)).checked(o.enabled).label(o.name.clone()).on_click(move |on, _, cx| {
                let id = id.clone();
                w.update(cx, |this, cx| this.toggle_vcity_org(&id, *on, cx)).ok();
            }));
        }
        if self.cal.settings.vcity_orgs.is_empty() {
            d = d.child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.cal.vcity_no_orgs")));
        }
        d.child(ui::outline_button(t, "vcity-disconnect").child(i18n::t!("app.cal.vcity_disconnect")).on_click(cx.listener(|this, _, _, cx| this.disconnect_vcity(cx))))
    }

    pub fn show_calendar(&mut self, cx: &mut Context<Self>) {
        self.set_page(Page::Calendar, cx);
    }
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's glob brings its own `test` attribute
    use super::{Mode, range, step};
    use chrono::NaiveDate;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn ranges_and_steps() {
        // November 2026 starts on a Sunday: the grid starts Monday Oct 26 and has 6 weeks
        assert_eq!(range(Mode::Month, d("2026-11-21")), (d("2026-10-26"), d("2026-12-06")));
        assert_eq!(range(Mode::Week, d("2026-11-21")), (d("2026-11-16"), d("2026-11-22")));
        assert_eq!(range(Mode::Agenda, d("2026-11-21")).1, d("2027-01-19"));
        assert_eq!(step(Mode::Month, d("2026-12-31"), 1), d("2027-01-01"));
        assert_eq!(step(Mode::Month, d("2026-01-15"), -1), d("2025-12-01"));
        assert_eq!(step(Mode::Week, d("2026-11-21"), -1), d("2026-11-14"));
    }
}

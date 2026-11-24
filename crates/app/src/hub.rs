//! The hub: the Today page (every app's feed), the Entities page (the shared registry), the Hub
//! part of Settings, and `otherwise://` links. Formats and rules live in otherwise-link.
use crate::app::{Launcher, Modal, Pending};
use crate::theme::Tokens;
use crate::ui;
use chrono::{Datelike, Local, NaiveDate, Utc};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::IconName;
use gpui_component::input::{Input, InputState};
use gpui_component::switch::Switch;
use otherwise_link::Root;
use otherwise_link::apps::{self, AppSpec, Capability, Installed, Locations};
use otherwise_link::entity::{Entity as Card, Kind};
use otherwise_link::link::Link;
use otherwise_link::registry::Registry;
use otherwise_link::settings::HubSettings;
use otherwise_link::today::{self, Event, EventKind, Merged, Severity};
use otherwise_mcp_hub::hub::ChildStatus;
use std::collections::BTreeMap;
use uuid::Uuid;

/// The app id the launcher records registry changes under.
const ME: &str = apps::HUB_ID;
const LIST_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Apps,
    Today,
    Entities,
    Calendar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Day,
    App,
}

pub struct Row {
    pub card: Card,
    /// (app id, that app's ref)
    pub refs: Vec<(String, String)>,
}

pub struct Edit {
    pub id: Uuid,
    pub name: Entity<InputState>,
    pub aliases: Entity<InputState>,
    pub tags: Entity<InputState>,
    pub notes: Entity<InputState>,
}

pub struct HubState {
    pub root: Root,
    pub settings: HubSettings,
    pub today: Merged,
    pub group: Group,
    pub installed: Vec<(AppSpec, Installed)>,
    pub rows: Vec<Row>,
    pub counts: BTreeMap<Kind, usize>,
    pub duplicates: Vec<(Card, Card)>,
    pub show_duplicates: bool,
    pub kind: Option<Kind>,
    pub search: Entity<InputState>,
    pub selected: Option<Uuid>,
    pub edit: Option<Edit>,
    pub error: Option<String>,
    /// What our own Today file last said, to skip identical rewrites.
    pub published: Option<Vec<Event>>,
    pub scroll: ScrollHandle,
    /// What the MCP hub found when it last started the apps' servers (Settings → Hub).
    pub mcp_status: Vec<ChildStatus>,
    /// Bumped per probe, so a slow old probe can't overwrite a newer one.
    pub mcp_probe: u64,
}

/// What Settings → Hub says under an app's switch, from the hub's last probe.
#[derive(Debug, Clone, PartialEq)]
pub enum McpNote {
    /// It refused write mode (its own settings don't allow agent changes): offered read-only.
    ReadOnly,
    /// It didn't start at all (both ways): its error.
    Unavailable(String),
}

pub fn mcp_note(status: &[ChildStatus], app: &str) -> Option<McpNote> {
    let s = status.iter().find(|s| s.app == app)?;
    match (&s.tools, &s.read_only) {
        (Err(e), _) => Some(McpNote::Unavailable(e.clone())),
        (Ok(_), Some(_)) => Some(McpNote::ReadOnly),
        (Ok(_), None) => None,
    }
}

impl HubState {
    pub fn new(window: &mut Window, cx: &mut Context<Launcher>) -> HubState {
        let root = Root::system();
        HubState {
            settings: HubSettings::load(&root),
            today: today::read(&root, Utc::now()),
            root,
            group: Group::Day,
            installed: vec![],
            rows: vec![],
            counts: BTreeMap::new(),
            duplicates: vec![],
            show_duplicates: false,
            kind: None,
            search: cx.new(|cx| InputState::new(window, cx)),
            selected: None,
            edit: None,
            error: None,
            published: None,
            scroll: ScrollHandle::new(),
            mcp_status: vec![],
            mcp_probe: 0,
        }
    }

    pub fn needs_attention(&self) -> usize {
        let today = Local::now().date_naive();
        self.today.events.iter().filter(|e| e.when_parsed().is_some_and(|w| w.day(&Local) <= today) || e.kind == EventKind::Alert).count()
    }
}

pub fn kind_label(k: Kind) -> String {
    match k {
        Kind::Person => i18n::t!("app.kind.person"),
        Kind::Company => i18n::t!("app.kind.company"),
        Kind::Vehicle => i18n::t!("app.kind.vehicle"),
        Kind::Property => i18n::t!("app.kind.property"),
        Kind::Device => i18n::t!("app.kind.device"),
        Kind::Site => i18n::t!("app.kind.site"),
        Kind::Digital => i18n::t!("app.kind.digital"),
    }
}

fn event_kind_label(k: EventKind) -> String {
    match k {
        EventKind::Due => i18n::t!("app.today.kind_due"),
        EventKind::Alert => i18n::t!("app.today.kind_alert"),
        EventKind::Review => i18n::t!("app.today.kind_review"),
        EventKind::Info => i18n::t!("app.today.kind_info"),
    }
}

fn weekday(d: NaiveDate) -> String {
    match d.weekday().num_days_from_monday() {
        0 => i18n::t!("app.weekday.mon"),
        1 => i18n::t!("app.weekday.tue"),
        2 => i18n::t!("app.weekday.wed"),
        3 => i18n::t!("app.weekday.thu"),
        4 => i18n::t!("app.weekday.fri"),
        5 => i18n::t!("app.weekday.sat"),
        _ => i18n::t!("app.weekday.sun"),
    }
}

pub fn date_label(d: NaiveDate) -> String {
    i18n::t!("app.today.date", weekday = weekday(d), day = d.day(), month = d.month(), year = d.year())
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// (label, value) lines for a card's kind section.
fn details(c: &Card) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    let mut put = |label: String, value: Option<String>| {
        if let Some(x) = value.filter(|x| !x.is_empty()) {
            v.push((label, x));
        }
    };
    let list = |l: &[String]| Some(l.join(", "));
    let tax = |l: &[otherwise_link::tax::TaxId]| Some(l.iter().map(|t| format!("{} {} {}", t.country, t.scheme, t.value)).collect::<Vec<_>>().join(", "));
    let coords = |lat: Option<f64>, lon: Option<f64>| lat.zip(lon).map(|(a, b)| format!("{a:.5}, {b:.5}"));
    if let Some(p) = &c.person {
        put(i18n::t!("app.field.given_name"), p.given_name.clone());
        put(i18n::t!("app.field.family_name"), p.family_name.clone());
        put(i18n::t!("app.field.birth_date"), p.birth_date.clone());
        put(i18n::t!("app.field.death_date"), p.death_date.clone());
        put(i18n::t!("app.field.tax_ids"), tax(&p.tax_ids));
        put(i18n::t!("app.field.emails"), list(&p.emails));
        put(i18n::t!("app.field.phones"), list(&p.phones));
        put(i18n::t!("app.field.country"), p.country.clone());
    }
    if let Some(p) = &c.company {
        put(i18n::t!("app.field.legal_name"), p.legal_name.clone());
        put(i18n::t!("app.field.tax_ids"), tax(&p.tax_ids));
        put(i18n::t!("app.field.country"), p.country.clone());
        put(i18n::t!("app.field.website"), p.website.clone());
        put(i18n::t!("app.field.emails"), list(&p.emails));
        put(i18n::t!("app.field.phones"), list(&p.phones));
        put(i18n::t!("app.field.address"), p.address.clone());
    }
    if let Some(p) = &c.vehicle {
        put(i18n::t!("app.field.plate"), p.plate.clone());
        put(i18n::t!("app.field.country"), p.country.clone());
        put(i18n::t!("app.field.vin"), p.vin.clone());
        put(i18n::t!("app.field.make"), p.make.clone());
        put(i18n::t!("app.field.model"), p.model.clone());
        put(i18n::t!("app.field.year"), p.year.map(|y| y.to_string()));
        put(i18n::t!("app.field.color"), p.color.clone());
    }
    if let Some(p) = &c.property {
        put(i18n::t!("app.field.address"), p.address.clone());
        put(i18n::t!("app.field.country"), p.country.clone());
        put(i18n::t!("app.field.cadastral_id"), p.cadastral_id.clone());
        put(i18n::t!("app.field.coordinates"), coords(p.lat, p.lon));
    }
    if let Some(p) = &c.device {
        put(i18n::t!("app.field.macs"), list(&p.macs));
        put(i18n::t!("app.field.hostnames"), list(&p.hostnames));
        put(i18n::t!("app.field.ips"), list(&p.ips));
        put(i18n::t!("app.field.tailscale_id"), p.tailscale_id.clone());
        put(i18n::t!("app.field.serial"), p.serial.clone());
        put(i18n::t!("app.field.vendor"), p.vendor.clone());
        put(i18n::t!("app.field.model"), p.model.clone());
        put(i18n::t!("app.field.os"), p.os.clone());
    }
    if let Some(p) = &c.site {
        put(i18n::t!("app.field.coordinates"), coords(p.lat, p.lon));
        put(i18n::t!("app.field.radius"), p.radius_m.map(|r| format!("{r:.0} m"))); // i18n-ok: unit symbol
        put(i18n::t!("app.field.address"), p.address.clone());
        put(i18n::t!("app.field.country"), p.country.clone());
    }
    if let Some(p) = &c.digital {
        put(i18n::t!("app.field.domain"), p.domain.clone());
        put(i18n::t!("app.field.service"), p.service.clone());
        put(i18n::t!("app.field.account"), p.account.clone());
        put(i18n::t!("app.field.url"), p.url.clone());
    }
    v
}

/// The command that adds the hub to Claude Code: this very executable with `mcp`.
pub fn mcp_command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "otherwise-apps".into()); // i18n-ok: binary name
    format!("claude mcp add otherwise-hub -- \"{exe}\" mcp") // i18n-ok: shell command
}

impl Launcher {
    // ── navigation and links ────────────────────────────────────────────────────────────────

    pub fn set_page(&mut self, p: Page, cx: &mut Context<Self>) {
        self.page = p;
        match p {
            Page::Today => self.reload_today(cx),
            Page::Entities => self.reload_entities(cx),
            Page::Calendar => {
                self.reload_calendar(cx);
                self.reload_vcity(cx);
            }
            Page::Apps => {}
        }
        cx.notify();
    }

    /// Reacts to an `otherwise://` link: our own paths switch pages, other apps' go to them.
    pub fn handle_link(&mut self, link: Link, window: &mut Window, cx: &mut Context<Self>) {
        window.activate_window();
        if link.app == ME {
            match link.at(0) {
                Some("today") => self.set_page(Page::Today, cx),
                Some("calendar") => {
                    if let Some(d) = link.get("date").and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) {
                        self.cal.cursor = d;
                    }
                    self.set_page(Page::Calendar, cx);
                }
                Some("assistant") => self.show_panel(window, cx),
                Some("entities") => {
                    self.hub.selected = link.at(1).and_then(|s| Uuid::parse_str(s).ok());
                    self.set_page(Page::Entities, cx);
                }
                Some("hub" | "settings") => {
                    self.open_settings(cx);
                }
                _ => self.set_page(Page::Apps, cx),
            }
            return;
        }
        self.open_link(&link, cx);
    }

    /// Opens a link in its app; never through `otherwise://` again (that would be us).
    pub fn open_link(&mut self, link: &Link, cx: &mut Context<Self>) {
        if link.app == ME {
            let l = link.clone();
            self.page = match l.at(0) {
                Some("entities") => Page::Entities,
                Some("today") => Page::Today,
                Some("calendar") => Page::Calendar,
                _ => Page::Apps,
            };
            cx.notify();
            return;
        }
        let name = apps::display_name(&link.app);
        let installed = self.hub.installed.iter().any(|(s, _)| s.id == link.app) || self.cards.iter().any(|c| c.entry.id == link.app && c.installed.is_some());
        if installed && otherwise_link::open::open_app_link(link).is_ok() {
            return;
        }
        // installed but without its link handler yet: at least bring the app up
        if self.cards.iter().any(|c| c.entry.id == link.app && c.installed.is_some()) {
            let id = link.app.clone();
            self.open_app(&id, cx);
            return;
        }
        if let Some((_, i)) = self.hub.installed.iter().find(|(s, _)| s.id == link.app)
            && let (Some(p), Some(exe)) = (i.path.clone(), i.executable.clone())
        {
            let _ = otherwise_core::process::open_app(self.target.os, &p, &exe);
            return;
        }
        self.show_toast(i18n::t!("app.link.not_installed", app = name), cx);
        self.page = Page::Apps;
        cx.notify();
    }

    /// Drains links that arrived from the OS (argv, Apple events) every 300 ms.
    pub fn watch_links(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
                let links = crate::take_inbound();
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        for l in links {
                            this.handle_link(l, window, cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    // ── discovery and our own Today feed ────────────────────────────────────────────────────

    /// Which Otherwise apps are installed (also closed-source ones the catalog doesn't list).
    pub fn rediscover(&mut self, cx: &mut Context<Self>) {
        let mut loc = Locations::system();
        loc.roots.insert(0, self.layout().root);
        let task = cx.background_executor().spawn(async move { apps::discover(&loc) });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            this.update(cx, |this, cx| {
                this.hub.installed = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pending updates as our own Today events.
    pub fn publish_own_today(&mut self) {
        let events: Vec<Event> = self
            .cards
            .iter()
            .filter_map(|c| match &c.status {
                otherwise_core::pipeline::Status::Update { to, .. } => Some(
                    Event::new(format!("update-{}-{to}", c.entry.id), EventKind::Info, i18n::t!("app.today.update_event", app = c.entry.name, version = to))
                        .severity(Severity::Low)
                        .link(&Link::new(ME).segment("app").segment(c.entry.id.clone())),
                ),
                _ => None,
            })
            .collect();
        if self.hub.published.as_ref() == Some(&events) {
            return;
        }
        if today::publish(&self.hub.root, ME, &events).is_ok() {
            self.hub.published = Some(events);
        }
    }

    // ── Today ───────────────────────────────────────────────────────────────────────────────

    pub fn reload_today(&mut self, cx: &mut Context<Self>) {
        // the data root may have moved (another Otherwise Hub window, or Settings)
        self.hub.root = Root::system();
        let root = self.hub.root.clone();
        let task = cx.background_executor().spawn(async move { today::read(&root, Utc::now()) });
        cx.spawn(async move |this, cx| {
            let m = task.await;
            this.update(cx, |this, cx| {
                this.hub.today = m;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn dismiss(&mut self, e: &Event, cx: &mut Context<Self>) {
        if today::dismiss(&self.hub.root, e, Utc::now()).is_ok() {
            let key = e.key();
            self.hub.today.events.retain(|x| x.key() != key);
        }
        cx.notify();
    }

    fn undismiss_all(&mut self, cx: &mut Context<Self>) {
        let _ = today::undismiss_all(&self.hub.root);
        self.reload_today(cx);
    }

    fn render_event(&self, t: &Tokens, e: &Event, cx: &mut Context<Self>) -> AnyElement {
        let bar = match e.severity {
            Severity::Critical => t.danger,
            Severity::High => t.warn,
            Severity::Normal => t.accent,
            Severity::Low => t.border_strong,
        };
        let time = match e.when_parsed() {
            Some(today::When::At(at)) => Some(at.with_timezone(&Local).format("%H:%M").to_string()),
            _ => None,
        };
        let link = e.link.as_deref().and_then(|l| Link::parse(l).ok());
        let has_link = link.is_some();
        let hover = t.raised;
        let ev = e.clone();
        let question = i18n::t!("app.assistant.ask_about", app = apps::display_name(&e.app), title = e.title.clone(), body = e.body.clone().unwrap_or_default());
        div()
            .id(SharedString::from(format!("ev-{}", e.key()))) // i18n-ok: element id
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .when(has_link, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
            .when_some(link, |d, l| d.on_click(cx.listener(move |this, _, _, cx| this.open_link(&l, cx))))
            .child(div().flex_none().w(px(4.)).h(px(32.)).rounded(px(2.)).bg(bar))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).truncate().child(e.title.clone()))
                    .children(e.body.clone().map(|b| div().text_size(px(12.)).text_color(t.ink_muted).truncate().child(b))),
            )
            .child(ui::chip(event_kind_label(e.kind), t.ink_muted, t.border.opacity(0.5)))
            .child(ui::chip(apps::display_name(&e.app), t.accent, t.accent_soft))
            .children(time.map(|x| div().flex_none().text_size(px(12.)).text_color(t.ink_muted).child(x)))
            .child(
                ui::outline_button(t, SharedString::from(format!("ask-{}", e.key()))) // i18n-ok: element id
                    .h(px(26.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .child(ui::icon(IconName::Bot, 13., t.accent))
                    .child(i18n::t!("app.today.ask"))
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.today.ask_tooltip")).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.ask_about(question.clone(), window, cx);
                    })),
            )
            .child(
                ui::icon_button(t, SharedString::from(format!("dismiss-{}", e.key())), true) // i18n-ok: element id
                    .child(ui::icon(IconName::Close, 13., t.ink_muted))
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.today.dismiss")).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.dismiss(&ev, cx);
                    })),
            )
            .into_any_element()
    }

    pub fn render_today(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let events = &self.hub.today.events;
        let group = self.hub.group;
        let mut switch = ui::segmented(t);
        for (g, label, id) in [(Group::Day, i18n::t!("app.today.by_day"), "by-day"), (Group::App, i18n::t!("app.today.by_app"), "by-app")] {
            switch = switch.child(ui::segment(t, id, g == group).child(label).on_click(cx.listener(move |this, _, _, cx| {
                this.hub.group = g;
                cx.notify();
            })));
        }
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.today.title")))
            .child(switch)
            .child(ui::outline_button(t, "today-restore").child(i18n::t!("app.today.restore")).on_click(cx.listener(|this, _, _, cx| this.undismiss_all(cx))))
            .child(ui::icon_button(t, "today-refresh", true).child(ui::glyph(ui::REFRESH, 15., t.ink)).on_click(cx.listener(|this, _, _, cx| this.reload_today(cx))));
        let mut body = div().flex().flex_col().gap(px(18.));
        if events.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .py(px(60.))
                    .child(ui::icon(IconName::CircleCheck, 28., t.ok))
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.today.empty_title")))
                    .child(div().text_size(px(13.)).text_color(t.ink_muted).text_center().max_w(px(460.)).child(i18n::t!("app.today.empty_body"))),
            );
        }
        let today_d = Local::now().date_naive();
        let section = |title: String, items: Vec<&Event>, this: &Self, cx: &mut Context<Self>| {
            div().flex().flex_col().gap(px(6.)).child(ui::section_title(t, format!("{title} · {}", items.len()))).children(items.into_iter().map(|e| this.render_event(t, e, cx)))
        };
        match group {
            Group::Day => {
                let mut overdue: Vec<&Event> = Vec::new();
                let mut rest: Vec<(String, Vec<&Event>)> = Vec::new();
                for (d, items) in today::by_day(events, &Local) {
                    match d {
                        Some(d) if d < today_d => overdue.extend(items),
                        Some(d) if d == today_d => rest.push((i18n::t!("app.today.today"), items)),
                        Some(d) if Some(d) == today_d.succ_opt() => rest.push((i18n::t!("app.today.tomorrow"), items)),
                        Some(d) => rest.push((date_label(d), items)),
                        None => rest.push((i18n::t!("app.today.anytime"), items)),
                    }
                }
                if !overdue.is_empty() {
                    body = body.child(section(i18n::t!("app.today.overdue"), overdue, self, cx));
                }
                for (title, items) in rest {
                    body = body.child(section(title, items, self, cx));
                }
            }
            Group::App => {
                for (app, items) in today::by_app(events) {
                    body = body.child(section(apps::display_name(&app), items, self, cx));
                }
            }
        }
        let errors: Vec<String> = self.hub.today.errors.iter().map(|(f, e)| format!("{f}: {e}")).collect();
        div()
            .id("today-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .p(px(24.))
                    .max_w(px(900.))
                    .child(header)
                    .when(!errors.is_empty(), |d| d.child(div().text_size(px(11.)).text_color(t.warn).child(i18n::t!("app.today.errors", files = errors.join("; ")))))
                    .child(body),
            )
            .into_any_element()
    }

    // ── Entities ────────────────────────────────────────────────────────────────────────────

    pub fn set_entities_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        match HubSettings::update(&self.hub.root, |s| s.entities = on) {
            Ok(s) => self.hub.settings = s,
            Err(e) => self.show_toast(i18n::t!("app.entities.error", detail = e), cx),
        }
        self.reload_entities(cx);
        cx.notify();
    }

    pub fn reload_entities(&mut self, cx: &mut Context<Self>) {
        self.hub.settings = HubSettings::load(&self.hub.root);
        if !self.hub.settings.entities {
            self.hub.rows.clear();
            self.hub.duplicates.clear();
            self.hub.counts.clear();
            return;
        }
        let root = self.hub.root.clone();
        let q = self.hub.search.read(cx).value().to_string();
        let kind = self.hub.kind;
        let task = cx.background_executor().spawn(async move {
            let reg = Registry::open_shared(&root)?;
            let cards = reg.search(&q, kind, LIST_LIMIT)?;
            let rows: Vec<Row> = cards.into_iter().map(|c| Row { refs: reg.referencing_apps(c.id).unwrap_or_default(), card: c }).collect();
            Ok::<_, otherwise_link::registry::Error>((rows, reg.counts()?, reg.duplicates(50)?))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok((rows, counts, dups)) => {
                        this.hub.rows = rows;
                        this.hub.counts = counts;
                        this.hub.duplicates = dups;
                        this.hub.error = None;
                    }
                    Err(e) => this.hub.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs a registry write in the background, then reloads.
    fn registry_write(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Registry) -> Result<(), otherwise_link::registry::Error> + Send + 'static) {
        let root = self.hub.root.clone();
        let task = cx.background_executor().spawn(async move { Registry::open_shared(&root).and_then(|mut r| f(&mut r)).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| {
                if let Err(e) = res {
                    this.show_toast(i18n::t!("app.entities.error", detail = e), cx);
                }
                this.reload_entities(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn ask_merge(&mut self, keep: &Card, drop: &Card, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Confirm {
            title: i18n::t!("app.entities.merge_title"),
            body: i18n::t!("app.entities.merge_body", keep = keep.name, drop = drop.name),
            confirm: i18n::t!("app.entities.merge_ok"),
            danger: false,
            action: Pending::Merge { keep: keep.id, drop: drop.id },
        });
        cx.notify();
    }

    pub fn ask_delete_entity(&mut self, c: &Card, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Confirm {
            title: i18n::t!("app.entities.delete_title", name = c.name),
            body: i18n::t!("app.entities.delete_body"),
            confirm: i18n::t!("app.entities.delete_ok"),
            danger: true,
            action: Pending::DeleteEntity { id: c.id },
        });
        cx.notify();
    }

    pub fn merge_entities(&mut self, keep: Uuid, drop: Uuid, cx: &mut Context<Self>) {
        self.hub.selected = Some(keep);
        self.registry_write(cx, move |r| r.merge(ME, keep, drop).map(|_| ()));
    }

    pub fn delete_entity(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.hub.selected = None;
        self.hub.edit = None;
        self.registry_write(cx, move |r| r.delete(ME, id).map(|_| ()));
    }

    fn start_edit(&mut self, c: &Card, window: &mut Window, cx: &mut Context<Self>) {
        let make = |v: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(v, window, cx);
                s
            })
        };
        self.hub.edit = Some(Edit {
            id: c.id,
            name: make(c.name.clone(), window, cx),
            aliases: make(c.aliases.join(", "), window, cx),
            tags: make(c.tags.join(", "), window, cx),
            notes: make(c.notes.clone().unwrap_or_default(), window, cx),
        });
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.hub.edit.take() else { return };
        let Some(row) = self.hub.rows.iter().find(|r| r.card.id == ed.id) else { return };
        let mut card = row.card.clone();
        card.name = ed.name.read(cx).value().trim().to_string();
        card.aliases = split_list(&ed.aliases.read(cx).value());
        card.tags = split_list(&ed.tags.read(cx).value());
        let notes = ed.notes.read(cx).value().trim().to_string();
        card.notes = (!notes.is_empty()).then_some(notes);
        if card.name.is_empty() {
            self.show_toast(i18n::t!("app.entities.name_required"), cx);
            self.hub.edit = Some(ed);
            return;
        }
        self.registry_write(cx, move |r| r.replace(ME, card).map(|_| ()));
    }

    fn render_optin(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .size_full()
            .p(px(32.))
            .child(ui::icon(IconName::User, 30., t.accent))
            .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.entities.optin_title")))
            .child(div().max_w(px(520.)).text_center().text_size(px(13.)).line_height(px(19.)).text_color(t.ink_muted).child(i18n::t!("app.entities.optin_body", path = self.hub.root.entities_db().display())))
            .child(ui::filled_button(t, "entities-on", t.accent).mt(px(6.)).child(i18n::t!("app.entities.turn_on")).on_click(cx.listener(|this, _, _, cx| this.set_entities_enabled(true, cx))))
            .into_any_element()
    }

    fn app_chips(t: &Tokens, refs: &[(String, String)]) -> Div {
        let mut apps_: Vec<&str> = refs.iter().map(|(a, _)| a.as_str()).collect();
        apps_.dedup();
        div().flex().flex_wrap().gap(px(4.)).children(apps_.into_iter().map(|a| ui::chip(apps::display_name(a), t.accent, t.accent_soft)))
    }

    fn render_entity_row(&self, t: &Tokens, r: &Row, cx: &mut Context<Self>) -> AnyElement {
        let id = r.card.id;
        let on = self.hub.selected == Some(id);
        let hover = t.raised;
        let keys = r.card.natural_keys().into_iter().map(|k| k.split_once(':').map(|(_, v)| v.to_string()).unwrap_or(k)).take(2).collect::<Vec<_>>().join(" · ");
        div()
            .id(SharedString::from(format!("ent-{id}"))) // i18n-ok: element id
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(9.))
            .cursor_pointer()
            .when(on, |d| d.bg(t.accent_soft))
            .when(!on, |d| d.hover(move |s| s.bg(hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.hub.selected = Some(id);
                this.hub.edit = None;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).truncate().child(r.card.name.clone()))
                    .child(div().text_size(px(11.)).text_color(t.ink_muted).truncate().child(if keys.is_empty() { kind_label(r.card.kind) } else { format!("{} · {keys}", kind_label(r.card.kind)) })),
            )
            .child(Self::app_chips(t, &r.refs))
            .into_any_element()
    }

    fn render_detail(&self, t: &Tokens, r: &Row, cx: &mut Context<Self>) -> AnyElement {
        let c = r.card.clone();
        let line = |label: String, value: String| div().flex().gap(px(8.)).text_size(px(12.)).child(div().w(px(110.)).flex_none().text_color(t.ink_muted).child(label)).child(div().flex_1().min_w_0().child(value));
        let mut panel = div().flex().flex_col().gap(px(10.)).w(px(380.)).flex_none().h_full().p(px(18.)).border_l_1().border_color(t.border).bg(t.surface).id("entity-detail").overflow_y_scroll();
        if let Some(ed) = self.hub.edit.as_ref().filter(|e| e.id == c.id) {
            let field = |label: String, input: &Entity<InputState>| {
                div().flex().flex_col().gap(px(4.)).child(ui::section_title(t, label)).child(div().h(px(32.)).px(px(8.)).flex().items_center().rounded(px(9.)).border_1().border_color(t.border).bg(t.canvas).text_size(px(13.)).child(Input::new(input).appearance(false)))
            };
            return panel
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.entities.edit_title")))
                .child(field(i18n::t!("app.entities.name"), &ed.name))
                .child(field(i18n::t!("app.entities.aliases"), &ed.aliases))
                .child(field(i18n::t!("app.entities.tags"), &ed.tags))
                .child(field(i18n::t!("app.entities.notes"), &ed.notes))
                .child(div().text_size(px(11.)).text_color(t.ink_muted).child(i18n::t!("app.entities.edit_hint")))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(ui::outline_button(t, "edit-cancel").child(i18n::t!("app.action.cancel")).on_click(cx.listener(|this, _, _, cx| {
                            this.hub.edit = None;
                            cx.notify();
                        })))
                        .child(ui::filled_button(t, "edit-save", t.accent).child(i18n::t!("app.entities.save")).on_click(cx.listener(|this, _, _, cx| this.save_edit(cx)))),
                )
                .into_any_element();
        }
        panel = panel
            .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(c.name.clone()))
            .child(div().flex().gap(px(6.)).child(ui::chip(kind_label(c.kind), t.ink_muted, t.border.opacity(0.5))))
            .when(!c.aliases.is_empty(), |d| d.child(line(i18n::t!("app.entities.aliases"), c.aliases.join(", "))))
            .children(details(&c).into_iter().map(|(l, v)| line(l, v)))
            .when(!c.tags.is_empty(), |d| d.child(line(i18n::t!("app.entities.tags"), c.tags.join(", "))))
            .children(c.notes.clone().map(|n| line(i18n::t!("app.entities.notes"), n)))
            .child(line(i18n::t!("app.entities.id"), c.id.to_string()));
        panel = panel.child(div().h(px(1.)).bg(t.border)).child(ui::section_title(t, i18n::t!("app.entities.references")));
        if r.refs.is_empty() {
            panel = panel.child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.entities.no_references")));
        }
        for (i, (app, rf)) in r.refs.iter().enumerate() {
            let link = Link::new(app).segment("entity").segment(c.id.to_string());
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ui::chip(apps::display_name(app), t.accent, t.accent_soft))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(t.ink_muted).child(rf.clone()))
                    .child(ui::outline_button(t, ("open-ref", i)).child(i18n::t!("app.entities.open_in", app = apps::display_name(app))).on_click(cx.listener(move |this, _, _, cx| this.open_link(&link, cx)))),
            );
        }
        let (c1, c2) = (c.clone(), c.clone());
        panel
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .mt(px(8.))
                    .child(ui::outline_button(t, "entity-edit").child(i18n::t!("app.entities.edit")).on_click(cx.listener(move |this, _, window, cx| this.start_edit(&c1, window, cx))))
                    .child(ui::outline_button(t, "entity-delete").text_color(t.danger).child(i18n::t!("app.entities.delete")).on_click(cx.listener(move |this, _, _, cx| this.ask_delete_entity(&c2, cx)))),
            )
            .into_any_element()
    }

    fn render_duplicates(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let mut d = div().flex().flex_col().gap(px(8.));
        for (i, (a, b)) in self.hub.duplicates.iter().enumerate() {
            let (ka, da) = (a.clone(), b.clone());
            let (kb, db) = (b.clone(), a.clone());
            d = d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.surface)
                    .child(div().flex_1().min_w_0().text_size(px(13.)).child(i18n::t!("app.entities.pair", a = a.name, b = b.name, kind = kind_label(a.kind))))
                    .child(ui::outline_button(t, ("keep-a", i)).child(i18n::t!("app.entities.keep", name = a.name)).on_click(cx.listener(move |this, _, _, cx| this.ask_merge(&ka, &da, cx))))
                    .child(ui::outline_button(t, ("keep-b", i)).child(i18n::t!("app.entities.keep", name = b.name)).on_click(cx.listener(move |this, _, _, cx| this.ask_merge(&kb, &db, cx)))),
            );
        }
        d
    }

    pub fn render_entities(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        if !self.hub.settings.entities {
            return self.render_optin(t, cx);
        }
        let total: usize = self.hub.counts.values().sum();
        let mut kinds = ui::segmented(t).child(ui::segment(t, "kind-all", self.hub.kind.is_none()).child(i18n::t!("app.entities.all", n = total)).on_click(cx.listener(|this, _, _, cx| {
            this.hub.kind = None;
            this.reload_entities(cx);
        })));
        for k in Kind::ALL {
            let n = self.hub.counts.get(&k).copied().unwrap_or(0);
            if n == 0 && self.hub.kind != Some(k) {
                continue;
            }
            kinds = kinds.child(ui::segment(t, SharedString::from(k.key()), self.hub.kind == Some(k)).child(format!("{} {n}", kind_label(k))).on_click(cx.listener(move |this, _, _, cx| {
                this.hub.kind = Some(k);
                this.reload_entities(cx);
            })));
        }
        let dups = self.hub.duplicates.len();
        let header = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.entities.title")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .w(px(260.))
                            .h(px(32.))
                            .px(px(10.))
                            .rounded(px(9.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.canvas)
                            .child(ui::icon(IconName::Search, 14., t.ink_muted))
                            .child(div().flex_1().min_w_0().text_size(px(13.)).child(Input::new(&self.hub.search).appearance(false))),
                    ),
            )
            .child(kinds)
            .when(dups > 0, |d| {
                d.child(
                    div()
                        .id("dups-banner")
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(t.warn_soft)
                        .cursor_pointer()
                        .child(ui::icon(IconName::TriangleAlert, 14., t.warn))
                        .child(div().flex_1().text_size(px(12.)).child(i18n::tn!("app.entities.duplicates", dups)))
                        .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(if self.hub.show_duplicates { i18n::t!("app.entities.hide") } else { i18n::t!("app.entities.review") }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hub.show_duplicates = !this.hub.show_duplicates;
                            cx.notify();
                        })),
                )
            })
            .when(self.hub.show_duplicates && dups > 0, |d| d.child(self.render_duplicates(t, cx)))
            .children(self.hub.error.clone().map(|e| div().text_size(px(12.)).text_color(t.danger).child(i18n::t!("app.entities.error", detail = e))));
        let mut list = div().flex().flex_col().gap(px(2.));
        if self.hub.rows.is_empty() {
            list = list.child(div().py(px(40.)).text_center().text_size(px(13.)).text_color(t.ink_muted).child(if total == 0 { i18n::t!("app.entities.empty") } else { i18n::t!("app.entities.no_match") }));
        }
        for r in &self.hub.rows {
            list = list.child(self.render_entity_row(t, r, cx));
        }
        let detail = self.hub.selected.and_then(|id| self.hub.rows.iter().find(|r| r.card.id == id)).map(|r| self.render_detail(t, r, cx));
        div()
            .flex()
            .size_full()
            .child(div().id("entities-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().track_scroll(&self.hub.scroll).child(div().flex().flex_col().gap(px(14.)).p(px(24.)).child(header).child(list)))
            .children(detail)
            .into_any_element()
    }

    // ── Settings → Hub ──────────────────────────────────────────────────────────────────────

    pub fn set_mcp_writes(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Ok(s) = HubSettings::update(&self.hub.root, |s| s.mcp_writes = on) {
            self.hub.settings = s;
        }
        self.probe_mcp(cx);
        cx.notify();
    }

    /// Opens Settings and re-checks the apps' MCP servers (which run read-only, which fail).
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Settings);
        self.probe_mcp(cx);
        cx.notify();
    }

    /// Starts the hub as an agent would (hub.json as it is now) and keeps what each app did.
    /// The hub only ever drops an app's write flag, never adds one.
    pub fn probe_mcp(&mut self, cx: &mut Context<Self>) {
        self.hub.mcp_probe += 1;
        let probe = self.hub.mcp_probe;
        let task = cx.background_executor().spawn(async move { otherwise_mcp_hub::Hub::start(otherwise_mcp_hub::Config::system()).status() });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            this.update(cx, |this, cx| {
                if this.hub.mcp_probe == probe {
                    this.hub.mcp_status = status;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn set_mcp_app(&mut self, app: String, on: bool, cx: &mut Context<Self>) {
        if let Ok(s) = HubSettings::update(&self.hub.root, |s| {
            s.mcp_disabled.retain(|a| *a != app);
            if !on {
                s.mcp_disabled.push(app.clone());
            }
        }) {
            self.hub.settings = s;
        }
        self.probe_mcp(cx);
        cx.notify();
    }

    pub fn render_hub_settings(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
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
        let entities = Switch::new("entities-switch").checked(self.hub.settings.entities).label(i18n::t!("app.hub.entities_label")).on_click({
            let w = weak.clone();
            move |on, _, cx| {
                w.update(cx, |this, cx| this.set_entities_enabled(*on, cx)).ok();
            }
        });
        let cmd = mcp_command();
        let copy = cmd.clone();
        let command = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(t.canvas).border_1().border_color(t.border).text_size(px(11.)).font_family("Menlo").child(cmd)) // i18n-ok: font name
            .child(ui::outline_button(t, "mcp-copy").child(i18n::t!("app.hub.copy")).on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                this.show_toast(i18n::t!("app.hub.copied"), cx);
            })));
        let writes = Switch::new("mcp-writes").checked(self.hub.settings.mcp_writes).label(i18n::t!("app.hub.writes_label")).on_click({
            let w = weak.clone();
            move |on, _, cx| {
                w.update(cx, |this, cx| this.set_mcp_writes(*on, cx)).ok();
            }
        });
        let mcp_apps: Vec<&(AppSpec, Installed)> = self.hub.installed.iter().filter(|(s, i)| s.has(Capability::Mcp) && i.cli.is_some()).collect();
        let mut list = div().flex().flex_col().gap(px(6.));
        if mcp_apps.is_empty() {
            list = list.child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.hub.no_mcp_apps")));
        }
        for (i, (s, _)) in mcp_apps.into_iter().enumerate() {
            let id = s.id.clone();
            let w = weak.clone();
            list = list.child(Switch::new(("mcp-app", i)).checked(self.hub.settings.mcp_enabled(&s.id)).label(i18n::t!("app.hub.app_tools", app = s.name, prefix = s.id)).on_click(move |on, _, cx| {
                let id = id.clone();
                w.update(cx, |this, cx| this.set_mcp_app(id, *on, cx)).ok();
            }));
            let note = if self.hub.settings.mcp_enabled(&s.id) { mcp_note(&self.hub.mcp_status, &s.id) } else { None };
            match note {
                Some(McpNote::ReadOnly) => {
                    list = list.child(
                        div()
                            .pl(px(44.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(t.ink).child(i18n::t!("app.hub.read_only")))
                            .child(div().text_size(px(11.)).line_height(px(16.)).text_color(t.ink_muted).child(i18n::t!("app.hub.read_only_hint", app = s.name))),
                    );
                }
                Some(McpNote::Unavailable(e)) => {
                    list = list.child(div().pl(px(44.)).text_size(px(11.)).line_height(px(16.)).text_color(t.danger).child(i18n::t!("app.hub.unavailable", detail = e)));
                }
                None => {}
            }
        }
        div()
            .flex()
            .flex_col()
            .child(div().pt(px(14.)).pb(px(4.)).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.hub.title")))
            .child(row(i18n::t!("app.hub.entities"), Some(i18n::t!("app.hub.entities_hint", path = self.hub.root.entities_db().display())), entities.into_any_element()))
            .child(row(i18n::t!("app.hub.mcp"), Some(i18n::t!("app.hub.mcp_hint")), command.into_any_element()))
            .child(row(i18n::t!("app.hub.writes"), Some(i18n::t!("app.hub.writes_hint")), writes.into_any_element()))
            .child(row(i18n::t!("app.hub.apps"), None, list.into_any_element()))
            .child(row(i18n::t!("app.cal.vcity"), Some(i18n::t!("app.cal.vcity_hint")), self.render_vcity_control(t, cx).into_any_element()))
    }
}

#[cfg(test)]
mod tests {
    use super::{ChildStatus, McpNote, mcp_note};

    fn st(app: &str, tools: Result<usize, String>, read_only: Option<&str>) -> ChildStatus {
        ChildStatus { app: app.into(), name: app.into(), program: String::new(), tools, read_only: read_only.map(Into::into) }
    }

    #[test]
    fn mcp_notes_follow_the_hub_probe() {
        let status = [st("expense", Ok(53), Some("exited with code 4")), st("vigia", Ok(9), None), st("nodo", Err("with writes: boom; read-only: boom".into()), None)];
        assert_eq!(mcp_note(&status, "expense"), Some(McpNote::ReadOnly));
        assert_eq!(mcp_note(&status, "vigia"), None, "write mode accepted: nothing to say");
        assert_eq!(mcp_note(&status, "nodo"), Some(McpNote::Unavailable("with writes: boom; read-only: boom".into())));
        assert_eq!(mcp_note(&status, "rama"), None, "not probed yet");
    }
}

//! The launcher window: catalog state, checks, jobs and the top-level layout. Cards are drawn in
//! `cards.rs`, modals in `dialogs.rs`.
use crate::hub::{HubState, Page};
use crate::jobs::{self, Job, JobError, Kind, Spec};
use crate::settings::Settings;
use crate::theme::{self, SANS, Tokens, tokens};
use crate::ui::{self, glyph};
use crate::util;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{IconName, Sizable as _};
use otherwise_core::catalog::{self, AppEntry, Manifest};
use otherwise_core::detect::{self, Installed};
use otherwise_core::layout::{self, Layout};
use otherwise_core::net;
use otherwise_core::pipeline::{self, Status};
use otherwise_core::platform::{Os, Target};
use otherwise_core::process;
use otherwise_core::sources::{self, Source};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

pub(crate) const CONTEXT: &str = "Launcher";
const TOAST_FOR: Duration = Duration::from_secs(5);
/// How often the "is a check due?" timer wakes up (the interval itself is in settings).
const TICK: Duration = Duration::from_secs(60);

actions!(otherwise_apps, [Refresh, ShowAbout, ShowSettings, FocusSearch, CloseModal, ShowApps, ShowToday, ShowEntities]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-r", Refresh, Some(CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(CONTEXT)),
        KeyBinding::new("secondary-,", ShowSettings, Some(CONTEXT)),
        KeyBinding::new("escape", CloseModal, Some(CONTEXT)),
        KeyBinding::new("secondary-1", ShowApps, Some(CONTEXT)),
        KeyBinding::new("secondary-2", ShowToday, Some(CONTEXT)),
        KeyBinding::new("secondary-3", ShowEntities, Some(CONTEXT)),
    ]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Name,
    Updated,
    Installed,
}

impl Sort {
    fn from_setting(s: Option<&str>) -> Sort {
        match s {
            Some("updated") => Sort::Updated,
            Some("installed") => Sort::Installed,
            _ => Sort::Name,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Updated => "updated",
            Sort::Installed => "installed",
        }
    }

    pub fn label(self) -> String {
        match self {
            Sort::Name => i18n::t!("app.sort.name"),
            Sort::Updated => i18n::t!("app.sort.updated"),
            Sort::Installed => i18n::t!("app.sort.installed"),
        }
    }
}

pub struct Card {
    pub entry: AppEntry,
    pub installed: Option<Installed>,
    pub status: Status,
    pub job: Option<Job>,
    pub error: Option<String>,
}

/// Something waiting for the user's yes.
#[derive(Debug, Clone)]
pub enum Pending {
    Run { id: String, kind: Kind, quit_first: bool },
    Uninstall { id: String, quit_first: bool },
    Merge { keep: uuid::Uuid, drop: uuid::Uuid },
    DeleteEntity { id: uuid::Uuid },
}

pub enum Modal {
    Confirm { title: String, body: String, confirm: String, danger: bool, action: Pending },
    Info { title: String, body: String },
    Restart { install: PathBuf, executable: PathBuf, version: String },
    Settings,
    About,
    /// Settings → Otherwise data folder → Change…: what would move to `to`.
    DataRoot { from: PathBuf, to: PathBuf, list: String, offer: crate::data_root::Offer, cloud: Option<String> },
}

pub struct Launcher {
    pub cards: Vec<Card>,
    pub sources: Vec<Source>,
    pub checking: bool,
    pub last_errors: Vec<String>,
    pub settings: Settings,
    pub search: Entity<InputState>,
    pub root_input: Entity<InputState>,
    pub modal: Option<Modal>,
    pub queue: VecDeque<String>,
    toast: Option<(String, Instant)>,
    pub logo: Arc<Image>,
    pub icons: HashMap<String, Arc<Image>>,
    pub target: Target,
    pub list_scroll: ScrollHandle,
    placeholder_locale: &'static str,
    focus: FocusHandle,
    ticking: bool,
    pub page: Page,
    pub hub: HubState,
    _subs: Vec<Subscription>,
}

/// Icons of the apps we know, so the grid never waits on the network for them.
fn bundled_icon(id: &str) -> Option<&'static [u8]> {
    Some(match id {
        "genealogy" => include_bytes!("../assets/apps/genealogy-rs.png"),
        "diskmap" => include_bytes!("../assets/apps/diskmap.png"),
        "rama" => include_bytes!("../assets/apps/rama.png"),
        "mosaico" => include_bytes!("../assets/apps/mosaico.png"),
        "expense" => include_bytes!("../assets/apps/expense.png"),
        catalog::SELF_ID => crate::ICON_PNG,
        _ => return None,
    })
}

fn saved_catalog_path() -> PathBuf {
    layout::cache_dir().join("catalog.json")
}

/// The catalog saved after the last good check, else the bundled one.
fn initial_catalog() -> (Manifest, Source) {
    std::fs::read_to_string(saved_catalog_path()).ok().and_then(|s| catalog::parse_manifest(&s).ok()).map(|m| (m, Source::Saved)).unwrap_or_else(|| (catalog::bundled(), Source::Bundled))
}

impl Launcher {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        theme::apply(settings.theme.as_deref(), Some(window), cx);
        let search = cx.new(|cx| InputState::new(window, cx));
        let root_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            if let Some(r) = &settings.install_root {
                s.set_value(r.clone(), window, cx);
            }
            s
        });
        let hub = HubState::new(window, cx);
        let subs = vec![
            cx.subscribe_in(&hub.search, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.reload_entities(cx);
                }
            }),
            cx.subscribe_in(&search, window, |_, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&root_input, window, |this, input, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let v = input.read(cx).value().trim().to_string();
                    this.settings = Settings::update(|s| s.install_root = (!v.is_empty()).then_some(v));
                    this.detect_all();
                    cx.notify();
                }
            }),
            // something may have been installed or removed by hand while we were in the background
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.detect_all();
                    this.rediscover(cx);
                    this.reload_today(cx);
                    if this.page == Page::Entities {
                        this.reload_entities(cx);
                    }
                    cx.notify();
                }
            }),
            window.observe_window_appearance(|window, cx| {
                let pref = Settings::load().theme;
                theme::apply(pref.as_deref(), Some(window), cx);
            }),
        ];
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let (manifest, source) = initial_catalog();
        let mut icons = HashMap::new();
        for a in &manifest.apps {
            if let Some(b) = bundled_icon(&a.id) {
                icons.insert(a.id.clone(), Arc::new(Image::from_bytes(ImageFormat::Png, b.to_vec())));
            }
        }
        let mut this = Launcher {
            cards: Vec::new(),
            sources: vec![source],
            checking: false,
            last_errors: Vec::new(),
            settings,
            search,
            root_input,
            modal: None,
            queue: VecDeque::new(),
            toast: None,
            logo: Arc::new(Image::from_bytes(ImageFormat::Png, crate::ICON_PNG.to_vec())),
            icons,
            target: Target::current(),
            list_scroll: ScrollHandle::new(),
            placeholder_locale: "",
            focus,
            ticking: false,
            page: Page::Apps,
            hub,
            _subs: subs,
        };
        this.set_catalog(manifest);
        this.refresh(true, cx);
        this.schedule_checks(cx);
        this.rediscover(cx);
        this.watch_links(window, cx);
        crate::star::refresh(this.settings.check_hours() != 0, cx);
        this
    }

    pub fn layout(&self) -> Layout {
        Layout::system(self.settings.install_root())
    }

    // ── catalog and detection ────────────────────────────────────────────────────────────────

    /// Replaces the catalog, keeping running jobs and errors of apps that are still listed.
    fn set_catalog(&mut self, m: Manifest) {
        let mut old: HashMap<String, Card> = self.cards.drain(..).map(|c| (c.entry.id.clone(), c)).collect();
        // an app with a job in flight stays even if a source dropped it
        let mut cards: Vec<Card> = m
            .apps
            .into_iter()
            .map(|entry| {
                let prev = old.remove(&entry.id);
                let (job, error) = prev.map(|c| (c.job, c.error)).unwrap_or((None, None));
                Card { entry, installed: None, status: Status::NotInstalled, job, error }
            })
            .collect();
        cards.extend(old.into_values().filter(|c| c.job.is_some()));
        self.cards = cards;
        self.detect_all();
    }

    pub fn detect_all(&mut self) {
        let layout = self.layout();
        for c in &mut self.cards {
            if c.job.is_some() {
                continue;
            }
            c.installed = detect::detect(&c.entry, &layout);
            c.status = pipeline::status(&c.entry, c.installed.as_ref(), self.target);
        }
        crate::notify::badge(self.update_count());
        self.publish_own_today();
    }

    pub fn update_count(&self) -> usize {
        self.cards.iter().filter(|c| matches!(c.status, Status::Update { .. })).count()
    }

    /// Website + GitHub in the background. `auto`: a scheduled check (may notify).
    pub fn refresh(&mut self, auto: bool, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.notify();
        let fallback = initial_catalog();
        let task = cx.background_executor().spawn(async move { sources::load(net::fetch_manifest, net::fetch_latest_release, fallback) });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            this.update(cx, |this, cx| this.loaded(loaded, auto, cx)).ok();
        })
        .detach();
    }

    fn loaded(&mut self, l: sources::Loaded, auto: bool, cx: &mut Context<Self>) {
        self.checking = false;
        self.last_errors = l.errors;
        self.sources = l.sources;
        if sources::is_live(&self.sources) {
            let path = saved_catalog_path();
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(path, catalog::to_json(&l.manifest));
            let now = util::now();
            self.settings = Settings::update(|s| s.last_check = Some(now.to_string()));
        }
        self.fetch_icons(&l.manifest, cx);
        self.set_catalog(l.manifest);
        self.announce_updates(auto);
        cx.notify();
    }

    /// Downloads the icons of apps we don't bundle (cached on disk).
    fn fetch_icons(&mut self, m: &Manifest, cx: &mut Context<Self>) {
        let dir = layout::cache_dir().join("icons");
        for a in &m.apps {
            if self.icons.contains_key(&a.id) {
                continue;
            }
            let Some(url) = a.icon.clone() else { continue };
            let file = dir.join(format!("{}.png", a.id));
            let id = a.id.clone();
            let task = cx.background_executor().spawn(async move {
                if let Ok(b) = std::fs::read(&file) {
                    return Some(b);
                }
                let b = net::get_bytes(&url, 2 << 20).ok()?;
                if !b.starts_with(b"\x89PNG") {
                    return None;
                }
                let _ = std::fs::create_dir_all(file.parent()?);
                let _ = std::fs::write(&file, &b);
                Some(b)
            });
            cx.spawn(async move |this, cx| {
                if let Some(b) = task.await {
                    this.update(cx, |this, cx| {
                        this.icons.insert(id, Arc::new(Image::from_bytes(ImageFormat::Png, b)));
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
    }

    /// One notification per new release, never twice for the same one.
    fn announce_updates(&mut self, auto: bool) {
        let fresh: Vec<(String, String)> = self
            .cards
            .iter()
            .filter_map(|c| match &c.status {
                Status::Update { to, .. } => Some((format!("{}@{}", c.entry.id, to), c.entry.name.clone())),
                _ => None,
            })
            .filter(|(key, _)| !self.settings.notified.contains(key))
            .collect();
        if fresh.is_empty() {
            return;
        }
        // the window is right there on a manual check; a notification would be noise
        if auto && self.settings.notify() {
            let names: Vec<&str> = fresh.iter().map(|(_, n)| n.as_str()).collect();
            crate::notify::post(&i18n::tn!("app.notify.title", fresh.len()), &i18n::t!("app.notify.body", apps = names.join(", ")));
        }
        let keys: Vec<String> = fresh.into_iter().map(|(k, _)| k).collect();
        self.settings = Settings::update(|s| {
            s.notified.extend(keys);
            let n = s.notified.len();
            if n > 64 {
                s.notified.drain(..n - 64);
            }
        });
    }

    /// Wakes up every minute: refreshes "checked N minutes ago" and runs a due check.
    fn schedule_checks(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this
                    .update(cx, |this, cx| {
                        let hours = this.settings.check_hours();
                        let due = hours > 0 && this.settings.last_check().is_none_or(|t| util::now() - t >= hours as i64 * 3600);
                        if due && !this.any_job() {
                            this.refresh(true, cx);
                        }
                        this.reload_today(cx);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    // ── jobs ─────────────────────────────────────────────────────────────────────────────────

    pub fn any_job(&self) -> bool {
        self.cards.iter().any(|c| c.job.is_some())
    }

    fn card(&self, id: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.entry.id == id)
    }

    fn card_mut(&mut self, id: &str) -> Option<&mut Card> {
        self.cards.iter_mut().find(|c| c.entry.id == id)
    }

    /// Install / update / reinstall, asking first if the app is open.
    pub fn begin(&mut self, id: &str, kind: Kind, cx: &mut Context<Self>) {
        let Some(c) = self.card(id) else { return };
        if c.job.is_some() {
            return;
        }
        let running = match (&c.installed, c.entry.is_self()) {
            (Some(i), false) => !process::running(self.target.os, &c.entry, &i.path).is_empty(),
            _ => false,
        };
        if running {
            let name = c.entry.name.clone();
            self.modal = Some(Modal::Confirm {
                title: i18n::t!("app.confirm.quit_title", app = name),
                body: i18n::t!("app.confirm.quit_body", app = name),
                confirm: i18n::t!("app.confirm.quit_ok"),
                danger: false,
                action: Pending::Run { id: id.to_string(), kind, quit_first: true },
            });
            cx.notify();
            return;
        }
        self.start(id, kind, false, cx);
    }

    fn start(&mut self, id: &str, kind: Kind, quit_first: bool, cx: &mut Context<Self>) {
        let layout = self.layout();
        let target = self.target;
        let Some(c) = self.card_mut(id) else { return };
        c.error = None;
        let job = Job::new(kind);
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        c.job = Some(job);
        let spec = Spec { entry: c.entry.clone(), layout, target, existing: c.installed.as_ref().map(|i| i.path.clone()), quit_first, overlay: c.entry.is_self() && target.os == Os::Windows };
        let id = id.to_string();
        let task = cx.background_executor().spawn(async move { jobs::install(&spec, &progress, &cancel).map(|o| (o, spec)) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| this.finished_install(&id, kind, res, cx)).ok();
        })
        .detach();
        self.tick(cx);
        cx.notify();
    }

    fn finished_install(&mut self, id: &str, kind: Kind, res: Result<(otherwise_core::install::Outcome, Spec), JobError>, cx: &mut Context<Self>) {
        let layout = self.layout();
        let target = self.target;
        let Some(c) = self.card_mut(id) else { return };
        c.job = None;
        match res {
            Ok((out, spec)) => {
                c.installed = detect::detect(&c.entry, &layout);
                c.status = pipeline::status(&c.entry, c.installed.as_ref(), target);
                let version = c.installed.as_ref().and_then(|i| i.version.clone()).unwrap_or_default();
                let name = c.entry.name.clone();
                let msg = match kind {
                    Kind::Update => i18n::t!("app.toast.updated", app = name, version = version),
                    _ => i18n::t!("app.toast.installed", app = name, version = version),
                };
                self.show_toast(msg, cx);
                if let Some(old) = out.leftover {
                    self.modal = Some(Modal::Info { title: i18n::t!("app.info.leftover_title"), body: i18n::t!("app.info.leftover_body", path = old.display()) });
                } else if spec.entry.is_self() {
                    self.modal = Some(Modal::Restart { install: out.path, executable: out.executable, version });
                }
            }
            Err(JobError::Cancelled) => {}
            Err(JobError::Failed(msg)) => c.error = Some(msg),
        }
        crate::notify::badge(self.update_count());
        self.start_next(cx);
        cx.notify();
    }

    pub fn cancel(&mut self, id: &str) {
        if let Some(j) = self.card(id).and_then(|c| c.job.as_ref()) {
            j.cancel.store(true, Ordering::Relaxed);
        }
        self.queue.retain(|q| q != id);
    }

    /// Queues every app with an update; Otherwise Apps itself goes last (it may restart).
    pub fn update_all(&mut self, cx: &mut Context<Self>) {
        let mut ids: Vec<String> = self.cards.iter().filter(|c| matches!(c.status, Status::Update { .. }) && c.job.is_none()).map(|c| c.entry.id.clone()).collect();
        ids.sort_by_key(|id| id == catalog::SELF_ID);
        for id in ids {
            if !self.queue.contains(&id) {
                self.queue.push_back(id);
            }
        }
        self.start_next(cx);
    }

    fn start_next(&mut self, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        // self-update waits for everything else, since it may ask to restart
        if self.queue.front().is_some_and(|id| id == catalog::SELF_ID) && self.any_job() {
            return;
        }
        while let Some(id) = self.queue.pop_front() {
            if self.card(&id).is_some_and(|c| matches!(c.status, Status::Update { .. }) && c.job.is_none()) {
                self.begin(&id, Kind::Update, cx);
                return;
            }
        }
    }

    pub fn ask_uninstall(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(c) = self.card(id) else { return };
        let Some(i) = &c.installed else { return };
        let layout = self.layout();
        let running = !process::running(self.target.os, &c.entry, &i.path).is_empty();
        let name = c.entry.name.clone();
        let mut body = i18n::t!("app.confirm.uninstall_body", app = name, path = i.path.display(), data = layout.data_dir(&c.entry).display());
        if running {
            body.push_str("\n\n");
            body.push_str(&i18n::t!("app.confirm.uninstall_running", app = name));
        }
        self.modal = Some(Modal::Confirm {
            title: i18n::t!("app.confirm.uninstall_title", app = name),
            body,
            confirm: i18n::t!("app.confirm.uninstall_ok"),
            danger: true,
            action: Pending::Uninstall { id: id.to_string(), quit_first: running },
        });
        cx.notify();
    }

    fn uninstall(&mut self, id: &str, quit_first: bool, cx: &mut Context<Self>) {
        let layout = self.layout();
        let target = self.target;
        let Some(c) = self.card_mut(id) else { return };
        let job = Job::new(Kind::Uninstall);
        let progress = job.progress.clone();
        c.job = Some(job);
        c.error = None;
        let spec = Spec { entry: c.entry.clone(), layout, target, existing: c.installed.as_ref().map(|i| i.path.clone()), quit_first, overlay: false };
        let id = id.to_string();
        let task = cx.background_executor().spawn(async move { jobs::uninstall(&spec, &progress) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| {
                let layout = this.layout();
                let target = this.target;
                let Some(c) = this.card_mut(&id) else { return };
                c.job = None;
                c.installed = detect::detect(&c.entry, &layout);
                c.status = pipeline::status(&c.entry, c.installed.as_ref(), target);
                match res {
                    Ok(removed) => {
                        let name = c.entry.name.clone();
                        this.modal = Some(Modal::Info { title: i18n::t!("app.info.removed_title", app = name), body: i18n::t!("app.info.removed_body", app = name, data = removed.data_dir.display()) });
                    }
                    Err(JobError::Failed(m)) => c.error = Some(m),
                    Err(JobError::Cancelled) => {}
                }
                crate::notify::badge(this.update_count());
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.tick(cx);
        cx.notify();
    }

    /// Repaints while jobs report progress.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(120)).await;
                let busy = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        let busy = this.any_job();
                        if !busy {
                            this.ticking = false;
                        }
                        busy
                    })
                    .unwrap_or(false);
                if !busy {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Confirm { action, .. }) = self.modal.take() else { return };
        match action {
            Pending::Run { id, kind, quit_first } => self.start(&id, kind, quit_first, cx),
            Pending::Uninstall { id, quit_first } => self.uninstall(&id, quit_first, cx),
            Pending::Merge { keep, drop } => self.merge_entities(keep, drop, cx),
            Pending::DeleteEntity { id } => self.delete_entity(id, cx),
        }
        cx.notify();
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        let was_confirm = matches!(self.modal, Some(Modal::Confirm { .. }));
        self.modal = None;
        if was_confirm {
            self.start_next(cx);
        }
        cx.notify();
    }

    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if let Some(Modal::Restart { install, executable, .. }) = self.modal.take() {
            if process::relaunch_after_exit(self.target.os, &install, &executable).is_ok() {
                cx.quit();
            }
        }
    }

    // ── small actions ────────────────────────────────────────────────────────────────────────

    pub fn open_app(&mut self, id: &str, cx: &mut Context<Self>) {
        let layout = self.layout();
        let Some(c) = self.card(id) else { return };
        let Some(i) = &c.installed else { return };
        let exe = layout.executable(&c.entry, &i.path);
        if let Err(e) = process::open_app(self.target.os, &i.path, &exe) {
            let msg = i18n::t!("app.error.open", app = c.entry.name, detail = e);
            self.show_toast(msg, cx);
        }
    }

    pub fn reveal(&mut self, id: &str) {
        if let Some(i) = self.card(id).and_then(|c| c.installed.as_ref()) {
            let _ = process::reveal(self.target.os, &i.path);
        }
    }

    pub fn show_toast(&mut self, text: String, cx: &mut Context<Self>) {
        let at = Instant::now();
        self.toast = Some((text, at));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_FOR).await;
            this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|(_, t)| *t == at) {
                    this.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn set_view(&mut self, list: bool, cx: &mut Context<Self>) {
        self.settings = Settings::update(|s| s.view = list.then(|| "list".to_string()));
        cx.notify();
    }

    pub fn is_list(&self) -> bool {
        self.settings.view.as_deref() == Some("list")
    }

    pub fn sort(&self) -> Sort {
        Sort::from_setting(self.settings.sort.as_deref())
    }

    fn set_sort(&mut self, s: Sort, cx: &mut Context<Self>) {
        self.settings = Settings::update(|x| x.sort = Some(s.key().to_string()));
        cx.notify();
    }

    pub fn set_language(&mut self, code: &'static str, cx: &mut Context<Self>) {
        if i18n::set_locale(code) {
            self.settings = Settings::update(|s| s.language = Some(code.to_string()));
            gpui_component::set_locale(code);
            crate::set_app_menus(cx);
            cx.refresh_windows();
        }
    }

    pub fn set_theme(&mut self, pref: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = Settings::update(|s| s.theme = pref.map(str::to_string));
        theme::apply(pref, Some(window), cx);
        cx.notify();
    }

    pub fn set_check_hours(&mut self, h: u64, cx: &mut Context<Self>) {
        self.settings = Settings::update(|s| s.check_hours = Some(h.to_string()));
        crate::star::refresh(h != 0, cx);
        cx.notify();
    }

    pub fn set_notify(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings = Settings::update(|s| s.notify = (!on).then(|| "off".to_string()));
        cx.notify();
    }

    pub fn reset_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.root_input.update(cx, |s, cx| s.set_value("", window, cx));
        self.settings = Settings::update(|s| s.install_root = None);
        self.detect_all();
        cx.notify();
    }

    /// Cards matching the search, in the chosen order.
    pub fn visible(&self, cx: &App) -> Vec<usize> {
        let q = self.search.read(cx).value().trim().to_lowercase();
        let lang = i18n::locale();
        let mut v: Vec<usize> = (0..self.cards.len())
            .filter(|&i| {
                let e = &self.cards[i].entry;
                q.is_empty() || [e.name.as_str(), e.id.as_str(), catalog::pick(&e.tagline, lang), catalog::pick(&e.description, lang)].iter().any(|s| s.to_lowercase().contains(&q))
            })
            .collect();
        let name = |i: &usize| self.cards[*i].entry.name.to_lowercase();
        let published = |i: &usize| self.cards[*i].entry.release.as_ref().and_then(|r| r.published_at.as_deref()).and_then(util::parse_iso).unwrap_or(i64::MIN);
        let rank = |i: &usize| match self.cards[*i].status {
            Status::Update { .. } => 0,
            Status::UpToDate { .. } => 1,
            Status::NotInstalled => 2,
            _ => 3,
        };
        match self.sort() {
            Sort::Name => v.sort_by_key(name),
            Sort::Updated => v.sort_by(|a, b| published(b).cmp(&published(a)).then_with(|| name(a).cmp(&name(b)))),
            Sort::Installed => v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| name(a).cmp(&name(b)))),
        }
        v
    }

    fn sync_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let loc = i18n::locale();
        if self.placeholder_locale != loc {
            self.placeholder_locale = loc;
            let text = i18n::t!("app.search.placeholder");
            self.search.update(cx, |s, cx| s.set_placeholder(text, window, cx));
            let text = i18n::t!("app.entities.search");
            self.hub.search.update(cx, |s, cx| s.set_placeholder(text, window, cx));
            let text = Layout::default_root(self.target.os).display().to_string();
            self.root_input.update(cx, |s, cx| s.set_placeholder(text, window, cx));
        }
    }

    // ── rendering ────────────────────────────────────────────────────────────────────────────

    fn render_topbar(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let installed = self.cards.iter().filter(|c| c.status.is_installed()).count();
        let updates = self.update_count();
        let summary = if updates > 0 { i18n::tn!("app.topbar.updates", updates, installed = installed) } else { i18n::tn!("app.topbar.installed", installed) };
        let weak = cx.entity().downgrade();
        let current = self.sort();
        let sort = Button::new("sort").ghost().with_size(px(32.)).label(current.label()).icon(gpui_component::Icon::empty().path(ui::SORT)).tooltip(i18n::t!("app.sort.tooltip")).dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            menu = menu.label(i18n::t!("app.sort.tooltip"));
            for s in [Sort::Name, Sort::Updated, Sort::Installed] {
                let w = weak.clone();
                menu = menu.item(PopupMenuItem::new(s.label()).checked(s == current).on_click(move |_, _, cx| {
                    w.update(cx, |this, cx| this.set_sort(s, cx)).ok();
                }));
            }
            menu
        });
        let list = self.is_list();
        let toggle = ui::segmented(t)
            .child(ui::segment(t, "view-grid", !list).child(glyph(ui::GRID, 14., if list { t.ink_muted } else { t.ink })).tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.view.grid")).build(window, cx)).on_click(cx.listener(|this, _, _, cx| this.set_view(false, cx))))
            .child(ui::segment(t, "view-list", list).child(glyph(ui::LIST, 14., if list { t.ink } else { t.ink_muted })).tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.view.list")).build(window, cx)).on_click(cx.listener(|this, _, _, cx| this.set_view(true, cx))));
        let checking = self.checking;
        let refresh = ui::icon_button(t, "refresh", !checking)
            .child(glyph(ui::REFRESH, 15., if checking { t.ink_muted.opacity(0.5) } else { t.ink }))
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.topbar.refresh")).build(window, cx))
            .when(!checking, |d| d.on_click(cx.listener(|this, _, _, cx| this.refresh(false, cx))));
        let settings = ui::icon_button(t, "settings", true).child(ui::icon(IconName::Settings, 16., t.ink)).tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.settings.title")).build(window, cx)).on_click(cx.listener(|this, _, _, cx| {
            this.modal = Some(Modal::Settings);
            cx.notify();
        }));
        let page = self.page;
        let attention = self.hub.needs_attention();
        let mut pages = ui::segmented(t);
        for (p, label, id) in [(Page::Apps, i18n::t!("app.page.apps"), "page-apps"), (Page::Today, i18n::t!("app.page.today"), "page-today"), (Page::Entities, i18n::t!("app.page.entities"), "page-entities")] {
            let label = if p == Page::Today && attention > 0 { format!("{label} {attention}") } else { label };
            pages = pages.child(ui::segment(t, id, p == page).child(label).on_click(cx.listener(move |this, _, _, cx| this.set_page(p, cx))));
        }
        let apps_page = page == Page::Apps;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(56.))
            .px(px(16.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.surface)
            .child(div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(9.)).bg(t.brand).child(ui::logo_glyph(18., gpui::white())))
            .child(div().flex().flex_col().min_w_0().flex_1().child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(crate::APP_NAME)).child(div().text_size(px(11.)).text_color(if updates > 0 { t.update } else { t.ink_muted }).truncate().child(summary)))
            .child(pages)
            .when(apps_page, |d| d.child(self.render_search(t)).child(sort).child(toggle).child(refresh))
            .when(apps_page && updates > 0, |d| {
                d.child(ui::filled_button(t, "update-all", t.accent).child(glyph(ui::UPDATE, 15., t.on_fill())).child(i18n::t!("app.topbar.update_all", n = updates)).on_click(cx.listener(|this, _, _, cx| this.update_all(cx))))
            })
            .child(settings)
    }

    fn render_search(&self, t: &Tokens) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .w(px(260.))
            .flex_shrink_1()
            .min_w(px(120.))
            .h(px(32.))
            .pl(px(10.))
            .pr(px(4.))
            .rounded(px(9.))
            .border_1()
            .border_color(t.border)
            .bg(t.canvas)
            .child(ui::icon(IconName::Search, 14., t.ink_muted))
            .child(div().flex_1().min_w_0().text_size(px(13.)).child(Input::new(&self.search).appearance(false).small()))
    }

    /// Shown above the grid when Otherwise Apps itself has an update.
    fn render_self_banner(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let c = self.cards.iter().find(|c| c.entry.is_self())?;
        let Status::Update { to, .. } = &c.status else { return None };
        if c.job.is_some() {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .mx(px(24.))
                .mt(px(18.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(12.))
                .bg(t.accent_soft)
                .child(glyph(ui::UPDATE, 18., t.accent))
                .child(div().flex_1().text_size(px(13.)).child(i18n::t!("app.banner.self", app = crate::APP_NAME, version = to)))
                .child(ui::filled_button(t, "self-update", t.accent).child(i18n::t!("app.action.update")).on_click(cx.listener(|this, _, _, cx| this.begin(catalog::SELF_ID, Kind::Update, cx)))),
        )
    }

    fn render_body(&self, t: &Tokens, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let visible = self.visible(cx);
        if visible.is_empty() {
            let q = self.search.read(cx).value().to_string();
            return div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .size_full()
                .child(ui::icon(IconName::Search, 28., t.ink_muted))
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.empty.title")))
                .child(div().text_size(px(13.)).text_color(t.ink_muted).child(i18n::t!("app.empty.body", query = q)))
                .into_any_element();
        }
        let content = if self.is_list() { self.render_list(t, &visible, cx).into_any_element() } else { self.render_grid(t, &visible, window, cx).into_any_element() };
        div()
            .id("apps-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .children(self.render_self_banner(t, cx))
            .child(div().p(px(24.)).child(content))
            .into_any_element()
    }

    fn sources_label(&self) -> String {
        let names: Vec<String> = self
            .sources
            .iter()
            .map(|s| match s {
                Source::Website => i18n::t!("app.source.website"),
                Source::GitHub => i18n::t!("app.source.github"),
                Source::Saved => i18n::t!("app.source.saved"),
                Source::Bundled => i18n::t!("app.source.bundled"),
            })
            .collect();
        names.join(" + ")
    }

    fn render_footer(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let left = if self.checking {
            i18n::t!("app.footer.checking")
        } else {
            match self.settings.last_check() {
                Some(at) if sources::is_live(&self.sources) => i18n::t!("app.footer.checked", when = util::ago(at, util::now()), sources = self.sources_label()),
                _ => i18n::t!("app.footer.offline", sources = self.sources_label()),
            }
        };
        let errors = self.last_errors.join("\n");
        let ink = t.ink;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(28.))
            .px(px(16.))
            .border_t_1()
            .border_color(t.border)
            .bg(t.surface)
            .text_size(px(11.))
            .text_color(t.ink_muted)
            .child(
                div()
                    .id("footer-status")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .when(!self.last_errors.is_empty() && !self.checking, |d| {
                        d.child(ui::icon(IconName::TriangleAlert, 12., t.update)).tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(errors.clone()).build(window, cx))
                    })
                    .child(div().truncate().child(left)),
            )
            .child(crate::star::footer_link(t, cx))
            .child(
                div()
                    .id("footer-about")
                    .flex_none()
                    .cursor_pointer()
                    .hover(move |s| s.text_color(ink).underline())
                    .child(i18n::t!("app.about.footer", app = crate::APP_NAME, version = crate::VERSION, company = crate::COMPANY))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.modal = Some(Modal::About);
                        cx.notify();
                    })),
            )
    }

    fn render_toast(&self, t: &Tokens) -> Option<impl IntoElement> {
        let (text, _) = self.toast.as_ref()?;
        Some(div().absolute().bottom(px(40.)).left_0().right_0().flex().justify_center().child(div().px(px(16.)).py(px(10.)).rounded(px(12.)).bg(t.ink).text_color(t.on_fill()).text_size(px(13.)).shadow_lg().child(text.clone())))
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_placeholder(window, cx);
        let t = tokens(cx);
        let body = match self.page {
            Page::Apps => self.render_body(&t, window, cx),
            Page::Today => self.render_today(&t, cx),
            Page::Entities => self.render_entities(&t, cx),
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(false, cx)))
            .on_action(cx.listener(|this, _: &ShowAbout, _, cx| {
                this.modal = Some(Modal::About);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowSettings, _, cx| {
                this.modal = Some(Modal::Settings);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| this.search.update(cx, |s, cx| s.focus(window, cx))))
            .on_action(cx.listener(|this, _: &CloseModal, _, cx| this.close_modal(cx)))
            .on_action(cx.listener(|this, _: &ShowApps, _, cx| this.set_page(Page::Apps, cx)))
            .on_action(cx.listener(|this, _: &ShowToday, _, cx| this.set_page(Page::Today, cx)))
            .on_action(cx.listener(|this, _: &ShowEntities, _, cx| this.set_page(Page::Entities, cx)))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.canvas)
            .text_color(t.ink)
            .font_family(SANS)
            .child(self.render_topbar(&t, cx))
            .child(div().flex_1().min_h_0().child(body))
            .child(self.render_footer(&t, cx))
            .children(self.render_toast(&t))
            .children(self.render_modal(&t, window, cx))
    }
}

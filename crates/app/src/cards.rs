//! App cards (grid) and rows (list), with their action button and "…" menu.
use crate::app::{Card, Launcher};
use crate::jobs::{Kind, Phase};
use crate::theme::Tokens;
use crate::ui::{self, glyph};
use crate::util;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{IconName, Sizable as _};
use otherwise_core::catalog;
use otherwise_core::pipeline::Status;
use otherwise_core::platform::Os;
use otherwise_core::version;
use std::sync::atomic::Ordering;

const CARD_MIN: f32 = 300.;
const GAP: f32 = 16.;

fn os_name(os: Os) -> String {
    match os {
        Os::Macos => i18n::t!("app.os.macos"),
        Os::Windows => i18n::t!("app.os.windows"),
        Os::Linux => i18n::t!("app.os.linux"),
    }
}

pub(crate) fn reveal_label(os: Os) -> String {
    match os {
        Os::Macos => i18n::t!("app.menu.reveal_macos"),
        Os::Windows => i18n::t!("app.menu.reveal_windows"),
        Os::Linux => i18n::t!("app.menu.reveal_linux"),
    }
}

fn phase_label(p: Phase, kind: Kind) -> String {
    match p {
        Phase::Preparing => i18n::t!("app.job.preparing"),
        Phase::Downloading => i18n::t!("app.job.downloading"),
        Phase::Verifying => i18n::t!("app.job.verifying"),
        Phase::Quitting => i18n::t!("app.job.quitting"),
        Phase::Installing if kind == Kind::Update => i18n::t!("app.job.updating"),
        Phase::Installing => i18n::t!("app.job.installing"),
        Phase::Removing => i18n::t!("app.job.removing"),
    }
}

impl Launcher {
    fn icon_el(&self, c: &Card, size: f32, t: &Tokens) -> AnyElement {
        match self.icons.get(&c.entry.id) {
            Some(img_) => img(img_.clone()).flex_none().size(px(size)).into_any_element(),
            None => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(size))
                .rounded(px(size * 0.22))
                .bg(t.accent_soft)
                .text_color(t.accent)
                .text_size(px(size * 0.42))
                .font_weight(FontWeight::SEMIBOLD)
                .child(c.entry.name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default())
                .into_any_element(),
        }
    }

    /// One line under the title: installed version, update, or why it can't be installed.
    fn status_line(&self, c: &Card, t: &Tokens) -> Div {
        let row = div().flex().items_center().gap(px(6.)).min_w_0().text_size(px(12.));
        match &c.status {
            Status::NotInstalled => row.text_color(t.ink_muted).child(i18n::t!("app.status.not_installed")),
            Status::UpToDate { version } => row
                .text_color(t.ok)
                .child(ui::icon(IconName::CircleCheck, 13., t.ok))
                .child(match version {
                    Some(v) => i18n::t!("app.status.installed", version = v),
                    None => i18n::t!("app.status.installed_unknown"),
                }),
            Status::Update { from, to } => row.child(ui::chip(i18n::t!("app.status.update", from = from, to = to), t.update, t.update_soft)),
            Status::NoRelease => row.text_color(t.ink_muted).child(i18n::t!("app.status.no_release")),
            Status::Unsupported => row.text_color(t.ink_muted).child(i18n::t!("app.status.unsupported", os = os_name(self.target.os))),
        }
    }

    /// The main button, a progress bar while a job runs, or the error with Retry.
    fn action_area(&self, c: &Card, t: &Tokens, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = c.entry.id.clone();
        let key = |s: &str| SharedString::from(format!("{s}-{id}"));
        if let Some(job) = &c.job {
            let p = &job.progress;
            let phase = p.phase();
            let frac = if phase == Phase::Downloading { p.fraction() } else { None };
            let mut label = phase_label(phase, job.kind);
            if let Some(f) = frac {
                label = i18n::t!("app.job.progress", phase = label, pct = (f * 100.).round() as u32, done = util::bytes(p.done.load(Ordering::Relaxed)), total = util::bytes(p.total.load(Ordering::Relaxed)));
            }
            let width = frac.unwrap_or(if matches!(phase, Phase::Installing | Phase::Verifying | Phase::Removing) { 1. } else { 0.06 });
            let id2 = id.clone();
            return div()
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(12.)).text_color(t.ink_muted).truncate().child(label))
                        .child(div().h(px(6.)).w_full().rounded_full().bg(t.border).child(div().h_full().rounded_full().bg(t.accent).w(relative(width)))),
                )
                .when(job.cancellable(), |d| {
                    d.child(ui::icon_button(t, key("cancel"), true).child(ui::icon(IconName::Close, 14., t.ink_muted)).tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.action.cancel")).build(window, cx)).on_click(cx.listener(move |this, _, _, cx| {
                        this.cancel(&id2);
                        cx.notify();
                    })))
                })
                .into_any_element();
        }
        if let Some(err) = &c.error {
            let kind = match c.status {
                Status::Update { .. } => Kind::Update,
                Status::UpToDate { .. } => Kind::Reinstall,
                _ => Kind::Install,
            };
            let id2 = id.clone();
            return div()
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .child(
                    div()
                        .id(key("error"))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .flex_1()
                        .min_w_0()
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(t.danger_soft)
                        .text_color(t.danger)
                        .text_size(px(12.))
                        .child(ui::icon(IconName::TriangleAlert, 13., t.danger))
                        .child(div().min_w_0().when(compact, |d| d.truncate()).when(!compact, |d| d.line_clamp(3)).child(err.clone())),
                )
                .child(ui::outline_button(t, key("retry")).child(i18n::t!("app.action.retry")).on_click(cx.listener(move |this, _, _, cx| this.begin(&id2, kind, cx))))
                .into_any_element();
        }
        let size = otherwise_core::pipeline::plan(&c.entry, self.target).map(|p| p.asset.size).filter(|s| *s > 0);
        let btn = match &c.status {
            Status::NotInstalled => ui::filled_button(t, key("install"), t.accent)
                .child(glyph(ui::DOWNLOAD, 14., t.on_fill()))
                .child(i18n::t!("app.action.install"))
                .when_some(size.filter(|_| !compact), |d, s| d.child(div().text_size(px(11.)).opacity(0.8).child(util::bytes(s))))
                .on_click(cx.listener(move |this, _, _, cx| this.begin(&id, Kind::Install, cx))),
            Status::Update { to, .. } => {
                let bg = if t.dark { t.update.opacity(0.9) } else { t.update };
                ui::filled_button(t, key("update"), bg).child(glyph(ui::UPDATE, 14., t.on_fill())).child(i18n::t!("app.action.update_to", version = to)).on_click(cx.listener(move |this, _, _, cx| this.begin(&id, Kind::Update, cx)))
            }
            Status::UpToDate { .. } if c.entry.is_self() => ui::outline_button(t, key("self")).text_color(t.ink_muted).cursor_default().child(i18n::t!("app.action.running")),
            Status::UpToDate { .. } => ui::outline_button(t, key("open")).child(ui::icon(IconName::ExternalLink, 13., t.ink)).child(i18n::t!("app.action.open")).on_click(cx.listener(move |this, _, _, cx| this.open_app(&id, cx))),
            Status::NoRelease => ui::outline_button(t, key("soon")).text_color(t.ink_muted).cursor_default().child(i18n::t!("app.action.soon")),
            Status::Unsupported => ui::outline_button(t, key("na")).text_color(t.ink_muted).cursor_default().child(i18n::t!("app.action.unavailable")),
        };
        btn.when(!compact, |b| b.flex_1()).into_any_element()
    }

    fn more_menu(&self, c: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let id = c.entry.id.clone();
        let installed = c.installed.is_some() && c.job.is_none();
        let is_self = c.entry.is_self();
        let can_reinstall = installed && otherwise_core::pipeline::plan(&c.entry, self.target).is_some();
        let release = c.entry.release.as_ref().map(|r| r.url.clone());
        let repo = c.entry.repo_url();
        let homepage = c.entry.homepage.clone();
        let os = self.target.os;
        Button::new(SharedString::from(format!("more-{id}"))) // i18n-ok: element id
            .ghost().icon(IconName::Ellipsis).with_size(px(32.)).tooltip(i18n::t!("app.menu.more")).dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            let act = |f: fn(&mut Launcher, &str, &mut Context<Launcher>)| {
                let (w, id) = (weak.clone(), id.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    w.update(cx, |this, cx| f(this, &id, cx)).ok();
                }
            };
            if installed && !is_self {
                menu = menu.item(PopupMenuItem::new(i18n::t!("app.action.open")).on_click(act(|this, id, cx| this.open_app(id, cx))));
            }
            if let Some(url) = release.clone() {
                menu = menu.item(PopupMenuItem::new(i18n::t!("app.menu.release_notes")).on_click(move |_, _, cx| cx.open_url(&url)));
            }
            if installed {
                menu = menu.item(PopupMenuItem::new(reveal_label(os)).on_click(act(|this, id, _| this.reveal(id))));
            }
            menu = menu.separator();
            let r = repo.clone();
            menu = menu.item(PopupMenuItem::new(i18n::t!("app.menu.github")).on_click(move |_, _, cx| cx.open_url(&r)));
            if let Some(h) = homepage.clone() {
                menu = menu.item(PopupMenuItem::new(i18n::t!("app.menu.website")).on_click(move |_, _, cx| cx.open_url(&h)));
            }
            if can_reinstall || (installed && !is_self) {
                menu = menu.separator();
            }
            if can_reinstall {
                menu = menu.item(PopupMenuItem::new(i18n::t!("app.menu.reinstall")).on_click(act(|this, id, cx| this.begin(id, Kind::Reinstall, cx))));
            }
            if installed && !is_self {
                menu = menu.item(PopupMenuItem::new(i18n::t!("app.menu.uninstall")).on_click(act(|this, id, cx| this.ask_uninstall(id, cx))));
            }
            menu
        })
    }

    fn version_and_date(&self, c: &Card, t: &Tokens) -> Div {
        let r = c.entry.release.as_ref();
        let ver = r.map(|r| format!("v{}", version::display(&r.tag)));
        let date = r.and_then(|r| r.published_at.as_deref()).and_then(util::parse_iso).map(|at| i18n::t!("app.card.updated", when = util::ago(at, util::now())));
        div().flex().items_center().gap(px(6.)).text_size(px(11.)).text_color(t.ink_muted).children(ver.map(|v| ui::chip(v, t.ink_muted, t.raised))).children(date)
    }

    fn github_button(&self, c: &Card, t: &Tokens) -> impl IntoElement {
        let url = c.entry.repo_url();
        ui::icon_button(t, SharedString::from(format!("gh-{}", c.entry.id)), true) // i18n-ok: element id
           
            .size(px(28.))
            .child(ui::icon(IconName::Github, 15., t.ink_muted))
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new(i18n::t!("app.menu.github")).build(window, cx))
            .on_click(move |_, _, cx| cx.open_url(&url))
    }

    pub fn render_grid(&self, t: &Tokens, visible: &[usize], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let avail = f32::from(window.viewport_size().width) - 48.;
        let cols = (((avail + GAP) / (CARD_MIN + GAP)).floor() as usize).clamp(1, 4);
        let w = ((avail - GAP * (cols as f32 - 1.)) / cols as f32).floor();
        let lang = i18n::locale();
        div().flex().flex_wrap().gap(px(GAP)).children(visible.iter().map(|&i| {
            let c = &self.cards[i];
            let highlight = matches!(c.status, Status::Update { .. });
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .w(px(w))
                .p(px(18.))
                .rounded(px(16.))
                .border_1()
                .border_color(if highlight { t.update.opacity(0.45) } else { t.border })
                .bg(t.surface)
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(14.))
                        .child(self.icon_el(c, 56., t))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .flex_1()
                                .min_w_0()
                                .child(div().flex().items_center().gap(px(6.)).child(div().flex_1().min_w_0().truncate().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(c.entry.name.clone())).child(self.github_button(c, t)))
                                .child(self.version_and_date(c, t)),
                        ),
                )
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).line_height(px(18.)).child(catalog::pick(&c.entry.tagline, lang).to_string()))
                .child(div().h(px(54.)).text_size(px(12.5)).line_height(px(18.)).text_color(t.ink_muted).line_clamp(3).child(catalog::pick(&c.entry.description, lang).to_string()))
                .child(div().h(px(1.)).bg(t.border))
                .child(self.status_line(c, t))
                .child(div().flex().items_center().gap(px(6.)).child(self.action_area(c, t, false, cx)).child(self.more_menu(c, cx)))
        }))
    }

    pub fn render_list(&self, t: &Tokens, visible: &[usize], cx: &mut Context<Self>) -> impl IntoElement {
        let lang = i18n::locale();
        div().flex().flex_col().rounded(px(14.)).border_1().border_color(t.border).bg(t.surface).overflow_hidden().children(visible.iter().enumerate().map(|(n, &i)| {
            let c = &self.cards[i];
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(16.))
                .py(px(12.))
                .when(n > 0, |d| d.border_t_1().border_color(t.border))
                .child(self.icon_el(c, 40., t))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(div().flex().items_center().gap(px(8.)).child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(c.entry.name.clone())).child(self.version_and_date(c, t)))
                        .child(div().text_size(px(12.)).text_color(t.ink_muted).truncate().child(catalog::pick(&c.entry.tagline, lang).to_string())),
                )
                .child(div().w(px(200.)).flex_none().child(self.status_line(c, t)))
                .child(div().flex().flex_none().items_center().justify_end().gap(px(4.)).w(px(280.)).child(self.action_area(c, t, true, cx)).child(self.github_button(c, t)).child(self.more_menu(c, cx)))
        }))
    }
}

//! Modals: confirmations, notices, the restart prompt after a self-update, settings and About.
use crate::app::{Launcher, Modal};
use crate::settings::HOUR_CHOICES;
use crate::theme::Tokens;
use crate::ui;
use gpui::*;
use gpui_component::IconName;
use gpui_component::input::Input;
use gpui_component::switch::Switch;
use otherwise_core::layout::Layout;
use otherwise_core::platform::Os;

impl Launcher {
    fn backdrop(&self, t: &Tokens, card: impl IntoElement, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("modal-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(if t.dark { 0.55 } else { 0.32 }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_modal(cx)))
            .child(card)
            .into_any_element()
    }

    fn panel(&self, t: &Tokens, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("modal-card")
            .relative()
            .flex()
            .flex_col()
            .gap(px(12.))
            .w(px(width))
            .max_h(relative(0.9))
            .p(px(24.))
            .rounded(px(18.))
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .shadow_lg()
            // clicks inside the card must not reach the backdrop
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().absolute().top(px(10.)).right(px(10.)).child(ui::icon_button(t, "modal-close", true).child(ui::icon(IconName::Close, 15., t.ink_muted)).on_click(cx.listener(|this, _, _, cx| this.close_modal(cx)))))
    }

    pub fn render_modal(&self, t: &Tokens, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = match self.modal.as_ref()? {
            Modal::Confirm { title, body, confirm, danger, .. } => {
                let bg = if *danger { t.danger } else { t.accent };
                self.panel(t, 440., cx)
                    .child(div().pr(px(28.)).text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title.clone()))
                    .child(div().text_size(px(13.)).line_height(px(19.)).text_color(t.ink_muted).child(body.clone()))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .mt(px(6.))
                            .child(ui::outline_button(t, "confirm-cancel").child(i18n::t!("app.action.cancel")).on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))))
                            .child(ui::filled_button(t, "confirm-ok", bg).child(confirm.clone()).on_click(cx.listener(|this, _, _, cx| this.confirm(cx)))),
                    )
                    .into_any_element()
            }
            Modal::Info { title, body } => self
                .panel(t, 440., cx)
                .child(div().pr(px(28.)).text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title.clone()))
                .child(div().text_size(px(13.)).line_height(px(19.)).text_color(t.ink_muted).child(body.clone()))
                .child(div().flex().justify_end().mt(px(6.)).child(ui::filled_button(t, "info-ok", t.accent).child(i18n::t!("app.action.ok")).on_click(cx.listener(|this, _, _, cx| this.close_modal(cx)))))
                .into_any_element(),
            Modal::Restart { version, .. } => self
                .panel(t, 420., cx)
                .items_center()
                .child(img(self.logo.clone()).size(px(72.)))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.restart.title", app = crate::APP_NAME, version = version)))
                .child(div().text_center().text_size(px(13.)).text_color(t.ink_muted).child(i18n::t!("app.restart.body", app = crate::APP_NAME)))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mt(px(6.))
                        .child(ui::outline_button(t, "restart-later").child(i18n::t!("app.restart.later")).on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))))
                        .child(ui::filled_button(t, "restart-now", t.accent).child(i18n::t!("app.restart.now")).on_click(cx.listener(|this, _, _, cx| this.restart(cx)))),
                )
                .into_any_element(),
            Modal::Settings => self.render_settings(t, window, cx).into_any_element(),
            Modal::About => self.render_about(t, cx).into_any_element(),
        };
        Some(self.backdrop(t, card, cx))
    }

    fn setting_row(t: &Tokens, label: String, hint: Option<String>, control: impl IntoElement) -> Div {
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
    }

    fn render_settings(&self, t: &Tokens, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = i18n::locale();
        let mut langs = ui::segmented(t);
        for (code, name) in i18n::available() {
            langs = langs.child(ui::segment(t, SharedString::from(code), code == current).child(name).on_click(cx.listener(move |this, _, _, cx| this.set_language(code, cx))));
        }
        let theme = self.settings.theme.clone();
        let mut themes = ui::segmented(t);
        for (key, label) in [(None, i18n::t!("app.settings.theme_system")), (Some("light"), i18n::t!("app.settings.theme_light")), (Some("dark"), i18n::t!("app.settings.theme_dark"))] {
            let on = theme.as_deref() == key || (key.is_none() && !matches!(theme.as_deref(), Some("light" | "dark")));
            themes = themes.child(ui::segment(t, SharedString::from(key.unwrap_or("system")), on) // i18n-ok: element id
               .child(label).on_click(cx.listener(move |this, _, window, cx| this.set_theme(key, window, cx))));
        }
        let hours = self.settings.check_hours();
        let mut every = ui::segmented(t);
        for h in HOUR_CHOICES {
            let label = if h == 0 { i18n::t!("app.settings.check_off") } else { i18n::tn!("app.settings.check_hours", h) };
            every = every.child(ui::segment(t, ("hours", h as usize), h == hours).child(label).on_click(cx.listener(move |this, _, _, cx| this.set_check_hours(h, cx))));
        }
        let weak = cx.entity().downgrade();
        let notify = Switch::new("notify").checked(self.settings.notify()).label(i18n::t!("app.settings.notify_label")).on_click(move |on, _, cx| {
            weak.update(cx, |this, cx| this.set_notify(*on, cx)).ok();
        });
        let location: AnyElement = match self.target.os {
            Os::Macos => div().text_size(px(13.)).child(i18n::t!("app.settings.location_macos")).into_any_element(),
            Os::Windows | Os::Linux => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().h(px(32.)).px(px(8.)).flex().items_center().rounded(px(9.)).border_1().border_color(t.border).bg(t.canvas).text_size(px(13.)).child(Input::new(&self.root_input).appearance(false)))
                .child(ui::outline_button(t, "root-reset").child(i18n::t!("app.settings.location_default")).on_click(cx.listener(|this, _, window, cx| this.reset_root(window, cx))))
                .into_any_element(),
        };
        let location_hint = match self.target.os {
            Os::Macos => None,
            os => Some(i18n::t!("app.settings.location_hint", path = Layout::default_root(os).display())),
        };
        self.panel(t, 560., cx)
            .gap(px(4.))
            .child(div().pb(px(8.)).text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.settings.title")))
            .child(Self::setting_row(t, i18n::t!("app.settings.language"), None, langs))
            .child(Self::setting_row(t, i18n::t!("app.settings.theme"), None, themes))
            .child(Self::setting_row(t, i18n::t!("app.settings.check"), Some(i18n::t!("app.settings.check_hint")), every))
            .child(Self::setting_row(t, i18n::t!("app.settings.notify"), None, notify))
            .child(Self::setting_row(t, i18n::t!("app.settings.location"), location_hint, location))
            .child(Self::setting_row(t, i18n::t!("app.settings.verify"), None, div().text_size(px(12.)).line_height(px(17.)).text_color(t.ink_muted).child(i18n::t!("app.settings.verify_body"))))
    }

    fn render_about(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = t.accent;
        self.panel(t, 420., cx)
            .items_center()
            .px(px(32.))
            .pt(px(32.))
            .child(img(self.logo.clone()).size(px(88.)))
            .child(div().flex().items_baseline().gap(px(8.)).child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(crate::APP_NAME)).child(div().text_size(px(13.)).text_color(t.ink_muted).child(crate::VERSION)))
            .child(div().text_center().text_size(px(13.)).line_height(px(19.)).text_color(t.ink_muted).child(i18n::t!("app.about.description")))
            .child(div().w_full().h(px(1.)).my(px(4.)).bg(t.border))
            .child(div().flex().flex_col().items_center().gap(px(2.)).child(div().text_size(px(13.)).child(i18n::t!("app.about.by", company = crate::COMPANY))).child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.about.place"))))
            .child(ui::filled_button(t, "about-site", t.accent).mt(px(4.)).child(i18n::t!("app.about.visit")).on_click(|_, _, cx| cx.open_url(crate::COMPANY_URL)))
            .child(div().id("about-source").text_size(px(12.)).text_color(accent).cursor_pointer().hover(|s| s.underline()).child(i18n::t!("app.about.source")).on_click(|_, _, cx| cx.open_url(crate::REPO_URL)))
            .child(div().text_size(px(11.)).text_color(t.ink_muted).child(i18n::t!("app.about.license")))
    }
}

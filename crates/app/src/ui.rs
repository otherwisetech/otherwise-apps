//! Small styled building blocks shared by the top bar, cards and dialogs.
use crate::theme::Tokens;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{Icon, IconName};

pub fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
}

/// One of our own line icons in `assets/icons` (grid, list, download…).
pub fn glyph(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg().path(path).flex_none().size(px(size)).text_color(color)
}

pub const GRID: &str = "icons/layout-grid.svg";
pub const LIST: &str = "icons/list.svg";
pub const DOWNLOAD: &str = "icons/download.svg";
pub const REFRESH: &str = "icons/refresh.svg";
pub const UPDATE: &str = "icons/arrow-up-circle.svg";
pub const TRASH: &str = "icons/trash.svg";
pub const SORT: &str = "icons/arrow-up-down.svg";

/// Outlined 32 px control.
pub fn outline_button(t: &Tokens, id: impl Into<ElementId>) -> Stateful<Div> {
    let hover = t.border_strong;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(32.))
        .px(px(12.))
        .rounded(px(9.))
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .text_color(t.ink)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(move |s| s.border_color(hover))
}

/// Filled 32 px control.
pub fn filled_button(t: &Tokens, id: impl Into<ElementId>, bg: Hsla) -> Stateful<Div> {
    let hover = bg.opacity(0.88);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(32.))
        .px(px(14.))
        .rounded(px(9.))
        .bg(bg)
        .text_color(t.on_fill())
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
}

/// Square icon-only 32 px control.
pub fn icon_button(t: &Tokens, id: impl Into<ElementId>, enabled: bool) -> Stateful<Div> {
    let hover = t.canvas;
    div().id(id).flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(9.)).when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
}

pub fn section_title(t: &Tokens, text: impl Into<SharedString>) -> Div {
    div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(t.ink_muted).child(text.into())
}

/// Rounded label with a tinted background.
pub fn chip(text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    div().flex().flex_none().items_center().gap(px(4.)).h(px(20.)).px(px(7.)).rounded(px(6.)).bg(bg).text_color(fg).text_size(px(11.)).font_weight(FontWeight::MEDIUM).child(text.into())
}

/// A segment of a segmented control (view toggle, settings choices).
pub fn segment(t: &Tokens, id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let muted = t.ink_muted;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .h(px(26.))
        .px(px(9.))
        .rounded(px(7.))
        .text_size(px(12.))
        .cursor_pointer()
        .when(on, |d| d.bg(t.surface).font_weight(FontWeight::SEMIBOLD).shadow_sm())
        .when(!on, |d| d.text_color(muted))
}

pub fn segmented(t: &Tokens) -> Div {
    div().flex().flex_none().items_center().gap(px(2.)).p(px(2.)).rounded(px(9.)).bg(t.border.opacity(0.6))
}

/// The app's own mark, tinted.
pub fn logo_glyph(size: f32, color: Hsla) -> Svg {
    svg().path(crate::LOGO_SVG).flex_none().size(px(size)).text_color(color)
}

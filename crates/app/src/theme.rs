//! Design tokens (same family as genealogy-rs, diskmap and rama) with Otherwise Apps' petrol
//! accent, plus the light / dark / follow-the-system switch.
use gpui::{App, Hsla, Window, rgb};
use gpui_component::{ActiveTheme, Theme, ThemeMode};

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub canvas: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub ink: Hsla,
    pub ink_muted: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub ok: Hsla,
    pub update: Hsla,
    pub update_soft: Hsla,
    /// logo badge background (the icon's petrol)
    pub brand: Hsla,
    pub dark: bool,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn a(hex: u32, alpha: f32) -> Hsla {
    let mut h = c(hex);
    h.a = alpha;
    h
}

impl Tokens {
    pub fn light() -> Self {
        Tokens {
            canvas: c(0xF2F1EE),
            surface: c(0xFFFFFF),
            raised: c(0xF8F7F4),
            border: c(0xE2DFD8),
            border_strong: c(0xC9C5BB),
            ink: c(0x1C1B19),
            ink_muted: c(0x6B675F),
            accent: c(0x1F6A72),
            accent_soft: c(0xE0EEEE),
            danger: c(0xA3382A),
            danger_soft: a(0xD0503F, 0.10),
            ok: c(0x1F7A44),
            update: c(0x9A5B0C),
            update_soft: a(0xF2A93B, 0.18),
            brand: c(0x1F5F67),
            dark: false,
        }
    }

    pub fn dark() -> Self {
        Tokens {
            canvas: c(0x141413),
            surface: c(0x1C1B1A),
            raised: c(0x232220),
            border: c(0x302E2B),
            border_strong: c(0x4A4741),
            ink: c(0xF2F0EA),
            ink_muted: c(0xA19C93),
            accent: c(0x6CC3C9),
            accent_soft: a(0x3FA6AE, 0.20),
            danger: c(0xE07767),
            danger_soft: a(0xE5675A, 0.14),
            ok: c(0x6FD39A),
            update: c(0xF2B95A),
            update_soft: a(0xF2A93B, 0.16),
            brand: c(0x2B7A82),
            dark: true,
        }
    }

    pub fn on_fill(&self) -> Hsla {
        if self.dark { c(0x141413) } else { c(0xFFFFFF) }
    }
}

pub fn tokens(cx: &App) -> Tokens {
    if cx.theme().mode.is_dark() { Tokens::dark() } else { Tokens::light() }
}

/// `settings.theme`: "light", "dark", anything else follows the system.
pub fn apply(pref: Option<&str>, window: Option<&mut Window>, cx: &mut App) {
    match pref {
        Some("light") => Theme::change(ThemeMode::Light, window, cx),
        Some("dark") => Theme::change(ThemeMode::Dark, window, cx),
        _ => Theme::sync_system_appearance(window, cx),
    }
    Theme::global_mut(cx).font_family = SANS.into();
}

pub const SANS: &str = "Inter";

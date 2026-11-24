//! The family's tokens come from otherwise-ui (github.com/otherwisetech/otherwise-ui, by tag);
//! Otherwise Apps only picks its petrol accent (`otherwise_ui::accents::HUB`, set in main.rs).
pub use otherwise_ui::Theme as Tokens;
pub use otherwise_ui::tokens::SANS;
use gpui::{App, Window};

pub fn tokens(cx: &App) -> Tokens {
    otherwise_ui::theme(cx)
}

/// `settings.theme`: "light", "dark", anything else follows the system.
pub fn apply(pref: Option<&str>, window: Option<&mut Window>, cx: &mut App) {
    otherwise_ui::apply(otherwise_ui::Appearance::parse(pref), window, cx);
}

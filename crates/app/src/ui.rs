//! The shared building blocks come from otherwise-ui; only the hub's own icons live here.
use gpui::*;

pub use otherwise_ui::{chip, filled_button, glyph, icon, icon_button, outline_button, section_title, segment, segmented};

pub const GRID: &str = "icons/layout-grid.svg";
pub const LIST: &str = "icons/list.svg";
pub const DOWNLOAD: &str = "icons/download.svg";
pub const REFRESH: &str = "icons/refresh.svg";
pub const UPDATE: &str = "icons/arrow-up-circle.svg";
pub const TRASH: &str = "icons/trash.svg";
pub const SORT: &str = "icons/arrow-up-down.svg";

/// The app's own mark, tinted.
pub fn logo_glyph(size: f32, color: Hsla) -> Svg {
    otherwise_ui::glyph(crate::LOGO_SVG, size, color)
}

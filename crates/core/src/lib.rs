//! Everything Otherwise Hub does that isn't UI: the catalog and where it comes from, version
//! comparison, picking the right release file, verified downloads, installing / replacing /
//! uninstalling, and finding what is already installed; the Calendar page's data (`calendar`) and
//! the optional Vcity source (`vcity`).
pub mod archive;
pub mod background;
pub mod calendar;
pub mod catalog;
pub mod detect;
pub mod digest;
pub mod install;
pub mod layout;
pub mod net;
pub mod pipeline;
pub mod platform;
pub mod process;
pub mod sources;
pub mod vcity;
pub mod version;

// No console window behind the app on Windows release builds.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod agent;
mod calendar;
mod cards;
mod data_root;
mod dialogs;
mod hub;
mod jobs;
mod notify;
mod secrets;
mod settings;
mod star;
mod theme;
mod ui;
mod util;

use gpui::*;
use gpui_component::Root;
use gpui_component_assets::Assets;
use otherwise_link::Link;
use std::borrow::Cow;
use std::sync::Mutex;

actions!(otherwise_apps, [Quit, OpenWebsite, OpenVcity]);

pub(crate) const APP_NAME: &str = "Otherwise Apps"; // i18n-ok: product name
pub(crate) const COMPANY: &str = "Otherwise Technologies LLC"; // i18n-ok: company name
pub(crate) const COMPANY_URL: &str = "https://otherwisetech.com";
/// The invitation to Vcity (About, Help menu, the Apps page).
pub(crate) const VCITY_URL: &str = "https://vcity.io/?utm_source=otherwise-apps&utm_medium=app&utm_campaign=otherwise-apps";
pub(crate) const REPO_URL: &str = "https://github.com/otherwisetech/otherwise-apps";
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The native menu bar holds plain strings, so it is rebuilt on every language switch.
pub(crate) fn set_app_menus(cx: &mut App) {
    let items = vec![
        MenuItem::action(i18n::t!("app.about.menu", app = APP_NAME), app::ShowAbout),
        MenuItem::action(i18n::t!("app.menu.check"), app::Refresh),
        MenuItem::action(i18n::t!("app.menu.settings"), app::ShowSettings),
        MenuItem::separator(),
        MenuItem::action(i18n::t!("app.credit.menu", company = COMPANY), OpenWebsite),
        MenuItem::separator(),
        MenuItem::action(i18n::t!("app.menu.quit"), Quit),
    ];
    let help = vec![MenuItem::action(i18n::t!("app.vcity.menu"), OpenVcity)];
    cx.set_menus(vec![Menu { name: APP_NAME.into(), items, disabled: false }, Menu { name: i18n::t!("app.menu.help").into(), items: help, disabled: false }]);
}

pub(crate) const LOGO_SVG: &str = "icons/otherwise-apps-logo.svg";
pub(crate) static ICON_PNG: &[u8] = include_bytes!("../assets/logo/otherwise-apps-icon-512.png");

/// gpui-component's icon set plus our own `icons/*.svg`.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let ours: Option<&'static [u8]> = match path {
            LOGO_SVG => Some(include_bytes!("../assets/icons/otherwise-apps-logo.svg")),
            ui::GRID => Some(include_bytes!("../assets/icons/layout-grid.svg")),
            ui::LIST => Some(include_bytes!("../assets/icons/list.svg")),
            ui::DOWNLOAD => Some(include_bytes!("../assets/icons/download.svg")),
            ui::REFRESH => Some(include_bytes!("../assets/icons/refresh.svg")),
            ui::UPDATE => Some(include_bytes!("../assets/icons/arrow-up-circle.svg")),
            ui::TRASH => Some(include_bytes!("../assets/icons/trash.svg")),
            ui::SORT => Some(include_bytes!("../assets/icons/arrow-up-down.svg")),
            _ => None,
        };
        match ours {
            Some(b) => Ok(Some(Cow::Borrowed(b))),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Assets.list(path)
    }
}

/// Sets the Dock icon at runtime: covers `cargo run` (no bundle) and stale Dock icon caches.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` feature
fn set_dock_icon() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    static PNG: &[u8] = include_bytes!("../assets/logo/otherwise-apps-icon-1024.png");
    // SAFETY: plain AppKit calls on the main thread with a valid byte buffer that NSData copies.
    unsafe {
        let data: *mut Object = msg_send![class!(NSData), dataWithBytes: PNG.as_ptr() length: PNG.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        let _: () = msg_send![image, release];
    }
}

const FONTS: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
];

/// `otherwise://` links waiting for the window (macOS delivers them before it exists, and
/// outside any `App` context).
static INBOUND: Mutex<Vec<Link>> = Mutex::new(Vec::new());

fn push_inbound(links: impl IntoIterator<Item = Link>) {
    INBOUND.lock().unwrap_or_else(|e| e.into_inner()).extend(links);
}

pub(crate) fn take_inbound() -> Vec<Link> {
    std::mem::take(&mut *INBOUND.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Windows and Linux learn about `otherwise://` at run time (macOS from Info.plist).
fn register_scheme() {
    let Ok(exe) = std::env::current_exe() else { return };
    std::thread::spawn(move || {
        if cfg!(windows) {
            let _ = otherwise_link::register::register_windows(otherwise_link::link::SCHEME, &exe, APP_NAME);
        } else if cfg!(target_os = "linux") {
            let file = "io.github.otherwisetech.otherwise-apps.desktop"; // i18n-ok: file name
            let entry = otherwise_link::paths::home().join(".local/share/applications").join(file);
            if entry.is_file() {
                let _ = otherwise_link::register::register_linux(file, &[otherwise_link::link::SCHEME]);
            }
        }
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // the MCP hub: stdin / stdout only, no window
    if args.first().map(String::as_str) == Some("mcp") {
        let read_only = args.iter().any(|a| a == agent::READ_ONLY_FLAG);
        let served = if read_only {
            // the AI panel's "Solo lectura": write tools hidden and refused whatever hub.json says
            let mut cfg = otherwise_mcp_hub::Config::system();
            cfg.writes = Some(false);
            otherwise_mcp_hub::Hub::start(cfg).serve(std::io::stdin().lock(), std::io::stdout().lock())
        } else {
            otherwise_mcp_hub::run_stdio()
        };
        if let Err(e) = served {
            eprintln!("otherwise-hub: {e}"); // i18n-ok: developer diagnostic
            std::process::exit(1);
        }
        return;
    }
    let settings = settings::Settings::load();
    i18n::set_locale(settings::initial_language(&settings));
    if matches!(std::env::args().nth(1).as_deref(), Some("-h" | "--help")) {
        println!("{}", i18n::t!("app.cli.usage"));
        return;
    }
    // a Windows self-update leaves the previous exe renamed next to us
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        otherwise_core::install::cleanup_overlay(dir, "otherwise-apps");
    }
    push_inbound(Link::from_args(&args));
    register_scheme();
    let application = gpui_platform::application().with_assets(AppAssets);
    application.on_open_urls(|urls| push_inbound(urls.iter().filter_map(|u| Link::parse(u).ok())));
    application.run(move |cx| {
        if let Err(e) = cx.text_system().add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect()) {
            eprintln!("fonts: {e}"); // i18n-ok: developer diagnostic
        }
        #[cfg(target_os = "macos")]
        set_dock_icon();
        gpui_component::init(cx);
        theme::apply(settings.theme.as_deref(), None, cx);
        app::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &OpenWebsite, cx| cx.open_url(COMPANY_URL));
        cx.on_action(|_: &OpenVcity, cx| cx.open_url(VCITY_URL));
        gpui_component::set_locale(i18n::locale());
        otherwise_agent_panel::ui::init(cx);
        otherwise_agent_panel::i18n::set_locale(i18n::locale());
        set_app_menus(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1180.), px(800.)), cx)),
            titlebar: Some(TitlebarOptions { title: Some(APP_NAME.into()), ..Default::default() }),
            window_min_size: Some(size(px(560.), px(420.))),
            ..Default::default()
        };
        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                let view = cx.new(|cx| app::Launcher::new(window, cx));
                let focus = view.read(cx).focus_handle();
                let host: std::rc::Rc<dyn otherwise_agent_panel::ui::AgentHost> = std::rc::Rc::new(agent::Host { launcher: view.downgrade() });
                let dock = cx.new(|cx| otherwise_agent_panel::ui::AgentDock::new(view.clone(), Some(focus), host, window, cx));
                let panel = dock.read(cx).panel().downgrade();
                view.update(cx, |v, _| v.panel = Some(panel));
                cx.new(|cx| Root::new(dock, window, cx))
            })
            .expect("open window");
        })
        .detach();
    });
}

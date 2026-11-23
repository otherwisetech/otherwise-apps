//! The macOS shell: an accessory app (no Dock icon, no window) with an `NSStatusItem`,
//! `UNUserNotificationCenter` notifications (attributed to the helper's bundle; a click opens the
//! notification's `otherwise://` link) and start at login through `SMAppService`.
use otherwise_helper::menu::{Action, Item};
use otherwise_helper::{Command, Shared};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAlert, NSApplication, NSApplicationActivationPolicy, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{NSBundle, NSError, NSObject, NSProcessInfo, NSString, NSTimer, ns_string};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use otherwise_link::Link;
use otherwise_link::background::Notification;
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// Separates the shown key from the link inside a notification request id.
const SEP: char = '\u{1f}';

pub struct Ivars {
    shared: Arc<Shared>,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    /// App ids of the "open app" items, by tag.
    apps: RefCell<Vec<String>>,
    generation: Cell<u64>,
    icon: Cell<u8>,
    center: Option<Retained<UNUserNotificationCenter>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; Controller doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OtherwiseHelperController"]
    #[ivars = Ivars]
    pub struct Controller;

    impl Controller {
        #[unsafe(method(pause:))]
        fn pause(&self, _sender: Option<&AnyObject>) {
            self.ivars().shared.send(Command::Pause);
        }

        #[unsafe(method(resume:))]
        fn resume(&self, _sender: Option<&AnyObject>) {
            self.ivars().shared.send(Command::Resume);
        }

        #[unsafe(method(openHub:))]
        fn open_hub(&self, _sender: Option<&AnyObject>) {
            let _ = otherwise_link::open::open_link(&Link::new(otherwise_link::apps::HUB_ID));
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            let _ = otherwise_link::open::open_link(&Link::new(otherwise_link::apps::HUB_ID).segment("settings").segment("helper"));
        }

        #[unsafe(method(about:))]
        fn about(&self, _sender: Option<&AnyObject>) {
            show_about(self.mtm());
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let s = &self.ivars().shared;
            s.quit.store(true, Ordering::Relaxed);
            s.wake.notify_all();
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }

        #[unsafe(method(openApp:))]
        fn open_app(&self, sender: Option<&NSMenuItem>) {
            let Some(tag) = sender.map(|s| s.tag()) else { return };
            if let Some(id) = self.ivars().apps.borrow().get(tag as usize) {
                let _ = otherwise_link::open::open_link(&Link::new(id));
            }
        }
    }

    unsafe impl NSObjectProtocol for Controller {}
);

define_class!(
    // SAFETY: as above; no ivars, so calls from any thread are fine.
    #[unsafe(super(NSObject))]
    #[name = "OtherwiseHelperNotificationDelegate"]
    pub struct NoteDelegate;

    unsafe impl NSObjectProtocol for NoteDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NoteDelegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, handler: &block2::DynBlock<dyn Fn()>) {
            let id = response.notification().request().identifier().to_string();
            open_from_id(&id);
            handler.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(&self, _center: &UNUserNotificationCenter, _n: &UNNotification, handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
            handler.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
        }
    }
);

/// `<app>/<id>` + SEP + link → open the link, else the app.
fn open_from_id(id: &str) {
    let (key, link) = id.split_once(SEP).unwrap_or((id, ""));
    let parsed = Link::parse(link).ok().or_else(|| key.split('/').next().filter(|a| otherwise_link::link::valid_app_id(a)).map(Link::new));
    if let Some(l) = parsed {
        let _ = otherwise_link::open::open_link(&l);
    }
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Notifications only work from inside a bundle with an identifier.
fn bundled() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

pub fn low_power() -> bool {
    let pi = NSProcessInfo::processInfo();
    // macOS 12+
    pi.respondsToSelector(sel!(isLowPowerModeEnabled)) && pi.isLowPowerModeEnabled()
}

impl Controller {
    fn new(mtm: MainThreadMarker, shared: Arc<Shared>) -> Retained<Controller> {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        item.setMenu(Some(&menu));
        let center = bundled().then(UNUserNotificationCenter::currentNotificationCenter);
        let this = Controller::alloc(mtm).set_ivars(Ivars { shared, item, menu, apps: RefCell::new(vec![]), generation: Cell::new(u64::MAX), icon: Cell::new(u8::MAX), center });
        // SAFETY: NSObject's init.
        let this: Retained<Controller> = unsafe { msg_send![super(this), init] };
        this.set_icon(0);
        this
    }

    fn set_icon(&self, which: u8) {
        if self.ivars().icon.get() == which {
            return;
        }
        self.ivars().icon.set(which);
        let name = match which {
            1 => ns_string!("pause.circle"),
            2 => ns_string!("arrow.triangle.2.circlepath.circle"),
            _ => ns_string!("circle.hexagongrid.circle"),
        };
        let Some(button) = self.ivars().item.button(self.mtm()) else { return };
        let desc = ns(&i18n::t!("helper.tooltip"));
        if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(name, Some(&desc)) {
            img.setTemplate(true);
            button.setImage(Some(&img));
        } else {
            button.setTitle(ns_string!("O"));
        }
        button.setToolTip(Some(&desc));
    }

    /// Called by the timer: menu, icon, pending notifications.
    fn refresh(&self) {
        let iv = self.ivars();
        let icon = if iv.shared.paused.load(Ordering::Relaxed) {
            1
        } else if iv.shared.busy.load(Ordering::Relaxed) {
            2
        } else {
            0
        };
        self.set_icon(icon);
        let (generation, items) = {
            let m = iv.shared.menu.lock().unwrap();
            (m.0, m.1.clone())
        };
        if generation != iv.generation.get() {
            iv.generation.set(generation);
            self.rebuild(&items);
        }
        let notes = std::mem::take(&mut *iv.shared.notes.lock().unwrap());
        for n in notes {
            self.post(&n);
        }
    }

    fn rebuild(&self, items: &[Item]) {
        let mtm = self.mtm();
        let iv = self.ivars();
        iv.menu.removeAllItems();
        let mut apps = vec![];
        for it in items {
            let mi = match it {
                Item::Separator => NSMenuItem::separatorItem(mtm),
                Item::Text(t) => {
                    // SAFETY: no action.
                    let mi = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(t), None, ns_string!("")) };
                    mi.setEnabled(false);
                    mi
                }
                Item::Action(a, title) => {
                    let sel = match a {
                        Action::Pause => sel!(pause:),
                        Action::Resume => sel!(resume:),
                        Action::OpenHub => sel!(openHub:),
                        Action::Settings => sel!(openSettings:),
                        Action::About => sel!(about:),
                        Action::Quit => sel!(quit:),
                        Action::OpenApp(_) => sel!(openApp:),
                    };
                    let key = if matches!(a, Action::Quit) { ns_string!("q") } else { ns_string!("") };
                    // SAFETY: the selectors exist on Controller, the target set below.
                    let mi = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), Some(sel), key) };
                    // SAFETY: Controller lives for the whole run (kept by the timer block).
                    unsafe { mi.setTarget(Some(self)) };
                    if let Action::OpenApp(id) = a {
                        mi.setTag(apps.len() as isize);
                        apps.push(id.clone());
                    }
                    mi
                }
            };
            iv.menu.addItem(&mi);
        }
        *iv.apps.borrow_mut() = apps;
    }

    fn post(&self, n: &Notification) {
        let Some(center) = &self.ivars().center else {
            otherwise_helper::post_plain(&n.title, n.body.as_deref().unwrap_or(""));
            return;
        };
        let content = UNMutableNotificationContent::new();
        content.setTitle(&ns(&n.title));
        content.setSubtitle(&ns(&otherwise_link::apps::display_name(&n.app)));
        if let Some(b) = &n.body {
            content.setBody(&ns(b));
        }
        let id = format!("{}{SEP}{}", n.key(), n.link.as_deref().unwrap_or(""));
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&ns(&id), &content, None);
        center.addNotificationRequest_withCompletionHandler(&req, None);
    }
}

fn show_about(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(&i18n::t!("helper.about.title", version = otherwise_helper::VERSION)));
    alert.setInformativeText(&ns(&format!("{}\n\n{}", i18n::t!("helper.about.credit"), i18n::t!("helper.about.vcity"))));
    alert.addButtonWithTitle(&ns(&i18n::t!("helper.about.ok")));
    alert.addButtonWithTitle(&ns(&i18n::t!("helper.about.website")));
    alert.addButtonWithTitle(&ns(&i18n::t!("helper.about.vcity_button")));
    let r = alert.runModal();
    let url = match r {
        1001 => Some("https://otherwisetech.com"),
        1002 => Some("https://vcity.io"),
        _ => None,
    };
    if let Some(u) = url {
        let _ = otherwise_link::open::open_url(u);
    }
}

/// Runs the menu-bar app until Quit. The worker thread does the work.
pub fn run(shared: Arc<Shared>) {
    let mtm = MainThreadMarker::new().expect("the helper's UI runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let controller = Controller::new(mtm, shared);
    if let Some(center) = &controller.ivars().center {
        let delegate: Retained<NoteDelegate> = unsafe { msg_send![NoteDelegate::alloc(), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // the center keeps a weak reference
        std::mem::forget(delegate);
        let done = RcBlock::new(|_granted: Bool, _err: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
    }
    controller.refresh();
    let c = controller.clone();
    let tick = RcBlock::new(move |_t: std::ptr::NonNull<NSTimer>| c.refresh());
    // SAFETY: a repeating timer on the main run loop; the block owns the controller.
    let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &tick) };
    app.run();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login {
    On,
    Off,
    /// Registered, waiting for the user in System Settings → Login Items.
    NeedsApproval,
    /// macOS before 13, or not running from a bundle.
    Unsupported,
}

fn main_app_service() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"SMAppService")?;
    if !bundled() {
        return None;
    }
    // SAFETY: +[SMAppService mainAppService] returns the service of this bundle.
    Some(unsafe { msg_send![cls, mainAppService] })
}

pub fn login_status() -> Login {
    let Some(svc) = main_app_service() else { return Login::Unsupported };
    // SAFETY: -[SMAppService status] → SMAppServiceStatus (NSInteger).
    let st: isize = unsafe { msg_send![&*svc, status] };
    match st {
        1 => Login::On,
        2 => Login::NeedsApproval,
        _ => Login::Off,
    }
}

/// Turns start at login on or off for this bundle; Err with the system's message.
pub fn set_login(on: bool) -> Result<Login, String> {
    let Some(svc) = main_app_service() else { return Ok(Login::Unsupported) };
    // SAFETY: the documented register / unregister calls; errors come back as NSError.
    let r: Result<(), Retained<NSError>> = unsafe { if on { msg_send![&*svc, registerAndReturnError: _] } else { msg_send![&*svc, unregisterAndReturnError: _] } };
    match r {
        Ok(()) => Ok(login_status()),
        Err(e) if !on && login_status() == Login::Off => {
            let _ = e;
            Ok(Login::Off)
        }
        Err(e) => Err(e.localizedDescription().to_string()),
    }
}

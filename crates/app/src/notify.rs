//! "Updates available": a system notification and the Dock badge (macOS).
use std::process::{Command, Stdio};

/// Posts a notification through the platform's own tool; failures are ignored (it's a nicety).
pub fn post(title: &str, body: &str) {
    if cfg!(target_os = "macos") {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("display notification \"{}\" with title \"{}\"", esc(body), esc(title));
        let _ = Command::new("osascript").args(["-e", &script]).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    } else if cfg!(target_os = "linux") {
        let _ = Command::new("notify-send").args(["--app-name", crate::APP_NAME, title, body]).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    } else if cfg!(windows) {
        // a balloon from the shell's tray API; quiet and dependency-free
        let ps = |s: &str| s.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; $n.ShowBalloonTip(8000, '{}', '{}', 'Info'); Start-Sleep -Seconds 9; $n.Dispose()",
            ps(title),
            ps(body)
        );
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]).stdout(Stdio::null()).stderr(Stdio::null());
        otherwise_core::process::no_window(&mut c);
        let _ = c.spawn();
    }
}

/// The number on the Dock icon; 0 clears it.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` feature
pub fn badge(count: usize) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    let label = if count == 0 { String::new() } else { count.to_string() };
    let Ok(c) = std::ffi::CString::new(label) else { return };
    // SAFETY: AppKit calls on the main thread; the NSString is autoreleased by AppKit.
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let tile: *mut Object = msg_send![app, dockTile];
        if tile.is_null() {
            return;
        }
        let s: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
        let _: () = msg_send![tile, setBadgeLabel: s];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn badge(_count: usize) {}

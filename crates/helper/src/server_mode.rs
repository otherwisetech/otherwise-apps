//! `otherwise-helper server …`: Otherwise Helper's server mode (otherwise-server). `run` is what
//! the server computer's launchd agent starts: the `/v1` server on loopback plus the apps'
//! background jobs (unless the menu-bar helper already runs them). The other commands edit
//! `<device dir>/server/server.json` and check the setup; they print JSON.
use otherwise_remote::Role;
use otherwise_server::config::{self, Config, DocRole, NoneTag};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

pub fn server_dir() -> PathBuf {
    config::dir(&otherwise_link::paths::device_dir())
}

fn flag<'a>(args: &'a [String], k: &str) -> Option<&'a str> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(a.as_str());
    }
    out
}

fn fail(e: impl std::fmt::Display) -> i32 {
    eprintln!("{}", i18n::t!("helper.server.failed", error = e));
    1
}

fn usage() -> i32 {
    eprintln!("{}", i18n::t!("helper.server.usage"));
    2
}

/// The command to publish the server on the tailnet (the user runs it; the helper never changes
/// Tailscale's configuration).
pub fn serve_command(port: u16) -> String {
    let cli = otherwise_remote::discover::tailscale_cli().map(|p| p.display().to_string()).unwrap_or_else(|| "tailscale".into()); // i18n-ok: command
    format!("\"{cli}\" serve --bg --https=443 http://127.0.0.1:{port}") // i18n-ok: command
}

pub fn main(args: &[String]) -> i32 {
    let dir = server_dir();
    let rest = &args[1.min(args.len())..];
    let edit = |f: &dyn Fn(&mut Config) -> Result<serde_json::Value, String>| -> i32 {
        let mut c = match Config::load(&dir) {
            Ok(c) => c,
            Err(e) => return fail(e),
        };
        match f(&mut c) {
            Ok(v) => match c.save(&dir) {
                Ok(()) => {
                    println!("{}", json!({"ok": true, "result": v}));
                    0
                }
                Err(e) => fail(e),
            },
            Err(e) => fail(e),
        }
    };
    match args.first().map(String::as_str) {
        Some("run") => run(&dir),
        Some("status") => {
            let c = Config::load(&dir).unwrap_or_default();
            let s = server(&dir);
            let mut v = s.status_json(&c);
            v["dir"] = json!(dir);
            v["tailscaleServe"] = json!(serve_command(c.port));
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Some("init") => {
            let Some(owner) = flag(rest, "--owner") else { return usage() };
            let name = flag(rest, "--name").unwrap_or("").to_string();
            let owner = owner.to_string();
            let tier = flag(rest, "--max-members").and_then(|n| n.parse::<u32>().ok());
            edit(&|c| {
                c.set_member(&owner, &name, Role::Owner);
                if tier.is_some() {
                    c.tier.max_members = tier;
                }
                Ok(json!({"owner": owner, "dir": dir}))
            })
        }
        Some("share") => {
            let p = positional(rest);
            let (Some(app), Some(folder)) = (p.first(), p.get(1)) else { return usage() };
            let path = PathBuf::from(folder);
            let name = flag(rest, "--name").map(str::to_string).unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let id = flag(rest, "--id").map(str::to_string);
            let app = app.to_string();
            edit(&|c| c.share(&app, &path, &name, id.as_deref()).map(|id| json!({"app": app, "id": id, "name": name})).map_err(|e| e.to_string()))
        }
        Some("unshare") => {
            let p = positional(rest);
            let (Some(app), Some(id)) = (p.first(), p.get(1)) else { return usage() };
            let (app, id) = (app.to_string(), id.to_string());
            edit(&|c| if c.unshare(&app, &id) { Ok(json!({"unshared": id})) } else { Err(format!("no document {app}/{id}")) })
        }
        Some("member") => {
            let p = positional(rest);
            let (Some(login), Some(role)) = (p.first(), p.get(1)) else { return usage() };
            let (login, role) = (login.to_string(), role.to_string());
            let name = flag(rest, "--name").unwrap_or("").to_string();
            let doc = flag(rest, "--doc").map(str::to_string);
            edit(&|c| {
                if role == "remove" {
                    return if c.remove_member(&login) { Ok(json!({"removed": login})) } else { Err(format!("no member {login}")) };
                }
                match (&doc, role.as_str()) {
                    (Some(d), "none") => {
                        let m = c.members.iter_mut().find(|m| m.login.eq_ignore_ascii_case(&login)).ok_or(format!("no member {login}"))?;
                        m.docs.insert(d.clone(), DocRole::None(NoneTag::None));
                    }
                    (Some(d), r) => {
                        let r = Role::parse(r).ok_or(format!("role: owner, editor, viewer, agent or none ({r})"))?;
                        let m = c.members.iter_mut().find(|m| m.login.eq_ignore_ascii_case(&login)).ok_or(format!("no member {login}"))?;
                        m.docs.insert(d.clone(), DocRole::Role(r));
                    }
                    (None, r) => c.set_member(&login, &name, Role::parse(r).ok_or(format!("role: owner, editor, viewer, agent or remove ({r})"))?),
                }
                Ok(json!({"member": login, "role": role, "doc": doc}))
            })
        }
        Some("tier") => {
            let p = positional(rest);
            let max = p.first().and_then(|n| n.parse::<u32>().ok());
            let name = flag(rest, "--name").unwrap_or("").to_string();
            edit(&|c| {
                c.tier.max_members = max;
                c.tier.name = name.clone();
                Ok(json!({"maxMembers": max}))
            })
        }
        Some("doctor") => doctor(&dir),
        _ => usage(),
    }
}

fn server(dir: &std::path::Path) -> otherwise_server::Server {
    otherwise_server::Server::new(dir, Box::new(otherwise_server::identity::System::default()), Box::new(otherwise_server::supervisor::Installed), Box::new(crate::sys::rss), crate::language())
}

fn doctor(dir: &std::path::Path) -> i32 {
    let c = Config::load(dir);
    let cfg = c.as_ref().cloned().unwrap_or_default();
    let cli = otherwise_remote::discover::tailscale_cli();
    let serve_status = cli.as_ref().and_then(|p| std::process::Command::new(p).args(["serve", "status", "--json"]).env("TAILSCALE_BE_CLI", "1").output().ok()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let published = serve_status.contains(&format!("127.0.0.1:{}", cfg.port));
    let listening = std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], cfg.port)), std::time::Duration::from_secs(1)).is_ok();
    let apps = otherwise_server::supervisor::Launcher::apps(&otherwise_server::supervisor::Installed);
    let missing: Vec<String> = cfg.docs.iter().filter(|d| !d.path.is_dir()).map(|d| format!("{}/{}", d.app, d.id)).collect();
    let v = json!({
        "config": c.as_ref().map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string()),
        "owners": cfg.owners().map(|m| m.login.clone()).collect::<Vec<_>>(),
        "docs": cfg.docs.len(),
        "missingDocs": missing,
        "servableApps": apps,
        "listening": listening,
        "tailscaleCli": cli,
        "published": published,
        "publishCommand": serve_command(cfg.port),
        "trustLoopback": cfg.trust_loopback,
    });
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    let ok = c.is_ok() && cfg.owners().next().is_some() && listening && published && missing.is_empty();
    if ok { 0 } else { 1 }
}

/// The server process: HTTP on loopback, plus the jobs worker when no other helper runs them.
fn run(dir: &std::path::Path) -> i32 {
    let cfg = match Config::load(dir) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let s = Arc::new(server(dir));
    let listener = match otherwise_server::bind(cfg.port) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    if cfg.trust_loopback {
        s.log("WARNING trustLoopback is on: any process of this user can claim any identity");
    }
    // one helper runs the jobs: this process, unless the menu-bar helper already does
    let lock = crate::single_instance();
    if lock.is_some() {
        let shared = Arc::new(crate::Shared::default());
        let engine = crate::system_engine();
        let sh = shared.clone();
        let _ = std::thread::Builder::new().name("jobs".into()).spawn(move || crate::work(sh, engine, &|| false, &|| false));
        let (sh, log) = (shared.clone(), s.clone());
        let _ = std::thread::Builder::new().name("notes".into()).spawn(move || {
            loop {
                for n in std::mem::take(&mut *sh.notes.lock().unwrap_or_else(|e| e.into_inner())) {
                    // no screen to show them on: the log keeps that they happened (ids only)
                    log.log(&format!("notification {}", n.id));
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        s.log("jobs: run by this server process");
    } else {
        s.log("jobs: run by the menu-bar Otherwise Helper");
    }
    match otherwise_server::run(s, listener) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

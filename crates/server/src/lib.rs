//! Otherwise Helper's server mode (otherwise-link SPEC §12): this computer's shared documents
//! over `/v1`, on loopback only, published to the tailnet with `tailscale serve`.
//!
//! - [`identity`]: Tailscale identity (serve headers + `whois`), and the check that loopback
//!   connections come from the Tailscale process (CI runners on the same OS user don't).
//! - [`config`]: shared documents, members and roles, tier, backups (`<device>/server/`).
//! - [`supervisor`]: the apps' `serve` processes (one per open document).
//! - [`seats`], [`audit`], [`idem`], [`backup`].
//!
//! No GPUI and no app code; English API messages (the apps translate).
pub mod audit;
pub mod backup;
pub mod config;
pub mod http;
pub mod identity;
pub mod idem;
pub mod seats;
pub mod supervisor;

use config::Config;
use http::{Request, Response};
use identity::{Identity, Tailnet};
use otherwise_remote::{ApiError, DocHello, DocInfo, Event, Hello, OpRequest, OpResult, Role, UndoRequest};
use serde_json::{Value, json};
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long an SSE stream stays open before `bye rotate` (the client reconnects at once).
pub const STREAM_ROTATE: Duration = Duration::from_secs(240);
pub const KEEPALIVE: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_STREAMS: usize = 64;

pub struct Server {
    pub dir: PathBuf,
    pub tailnet: Box<dyn Tailnet>,
    pub sup: supervisor::Supervisor,
    pub audit: audit::Audit,
    idem: Mutex<idem::Idem>,
    seats: Mutex<seats::Seats>,
    pub host: String,
    pub quit: AtomicBool,
    streams: Arc<std::sync::atomic::AtomicUsize>,
    log: Mutex<()>,
}

fn err(code: &str, msg: &str) -> ApiError {
    ApiError::new(code, msg)
}

impl Server {
    pub fn new(server_dir: &Path, tailnet: Box<dyn Tailnet>, launcher: Box<dyn supervisor::Launcher>, rss: supervisor::Rss, lang: &str) -> Server {
        let _ = std::fs::create_dir_all(server_dir);
        let _ = config::private_dir(server_dir);
        Server {
            dir: server_dir.to_path_buf(),
            tailnet,
            sup: supervisor::Supervisor::new(launcher, rss, lang, Some(server_dir.join("serve.log"))),
            audit: audit::Audit::new(server_dir),
            idem: Mutex::new(idem::Idem::open(server_dir)),
            seats: Mutex::new(seats::Seats::load(server_dir)),
            host: host_name(),
            quit: AtomicBool::new(false),
            streams: Default::default(),
            log: Mutex::new(()),
        }
    }

    /// Whether a connection from `peer` to `local` comes from the Tailscale process.
    pub fn peer_ok(&self, peer: SocketAddr, local: SocketAddr) -> bool {
        self.tailnet.peer_is_tailscale(peer, local)
    }

    pub fn config(&self) -> Config {
        Config::load(&self.dir).unwrap_or_else(|e| {
            self.log(&format!("config: {e}"));
            // an unreadable or newer file shares nothing rather than guessing
            Config { docs: Vec::new(), members: Vec::new(), ..Config::default() }
        })
    }

    /// `<server dir>/server.log`: starts, refusals, seats, processes. No document data.
    pub fn log(&self, line: &str) {
        let _g = self.log.lock();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("server.log")) {
            let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        }
    }

    /// Answer one request. `peer_is_tailscale`: the connection's other end is the Tailscale
    /// process (checked once per connection, [`Server::peer_ok`]).
    pub fn handle(&self, req: &Request, peer_is_tailscale: bool) -> Response {
        let cfg = self.config();
        let id = match identity::identify(req, peer_is_tailscale || cfg.trust_loopback, self.tailnet.as_ref()) {
            Ok(i) => i,
            Err(e) => {
                self.log(&format!("refused {} {}: {}", req.method, req.path, e.message));
                return Response::error(&e);
            }
        };
        match self.route(req, &id, &cfg) {
            Ok(r) => r,
            Err(e) => Response::error(&e),
        }
    }

    fn route(&self, req: &Request, id: &Identity, cfg: &Config) -> Result<Response, ApiError> {
        let parts: Vec<&str> = req.path.trim_end_matches('/').split('/').skip(1).collect();
        let m = req.method.as_str();
        match (m, parts.as_slice()) {
            ("GET", ["v1", "hello"]) => Ok(Response::json(200, &serde_json::to_value(self.hello(id, cfg)).unwrap_or_default())),
            ("GET", ["v1", "apps", app, "docs"]) => self.docs(id, cfg, app),
            ("GET", ["v1", "apps", app, "docs", doc, "hello"]) => self.doc_hello(id, cfg, app, doc),
            ("GET", ["v1", "apps", app, "docs", doc, "snapshot"]) => self.snapshot(req, id, cfg, app, doc),
            ("GET", ["v1", "apps", app, "docs", doc, "events"]) => self.events(req, id, cfg, app, doc),
            ("POST", ["v1", "apps", app, "docs", doc, "ops"]) => self.ops(req, id, cfg, app, doc),
            ("POST", ["v1", "apps", app, "docs", doc, "undo"]) => self.undo(req, id, cfg, app, doc),
            ("GET", ["v1", "server"]) => self.status(id, cfg),
            ("GET", ["v1", "audit"]) => {
                self.owner(id, cfg)?;
                Ok(Response::json(200, &json!({"lines": self.audit.read(http::query_param(&req.query, "month"), 500)})))
            }
            (_, ["v1", ..]) if !matches!(m, "GET" | "POST") => Err(err("bad_request", "method not allowed")),
            _ => Err(err("not_found", "no such resource")),
        }
    }

    fn owner(&self, id: &Identity, cfg: &Config) -> Result<(), ApiError> {
        match cfg.member(&id.login) {
            Some(m) if m.role == Role::Owner => Ok(()),
            Some(_) => Err(err("forbidden", "only the server's owner")),
            None => Err(err("forbidden", "you aren't a member of this server: ask its owner")),
        }
    }

    pub fn hello(&self, id: &Identity, cfg: &Config) -> Hello {
        let me = cfg.member(&id.login).map(|m| otherwise_remote::Me { login: id.login.clone(), name: if m.name.is_empty() { id.name.clone() } else { m.name.clone() }, device: id.device.clone(), role: Some(m.role) });
        let mut apps: Vec<String> = cfg.docs.iter().map(|d| d.app.clone()).collect();
        apps.sort();
        apps.dedup();
        let apps = if me.is_some() { apps.into_iter().map(|a| otherwise_remote::AppInfo { version: self.sup.launcher().version(&a), protocol: self.sup.launcher().plan(&a).map(|p| p.protocol).unwrap_or(0), id: a }).collect() } else { Vec::new() };
        Hello { server: otherwise_remote::SERVER.into(), version: VERSION.into(), protocols: otherwise_remote::Protocols { min: otherwise_remote::PROTOCOL_MIN, max: otherwise_remote::PROTOCOL_MAX }, host: self.host.clone(), me, apps }
    }

    /// The caller's role on a document, counting their seat. `404` when it isn't theirs.
    fn role(&self, id: &Identity, cfg: &Config, app: &str, doc: &str) -> Result<(Role, config::Doc), ApiError> {
        if cfg.member(&id.login).is_none() {
            return Err(err("forbidden", "you aren't a member of this server: ask its owner"));
        }
        let role = cfg.role_on(&id.login, app, doc).ok_or_else(|| err("not_found", "no such document"))?;
        let d = cfg.doc(app, doc).cloned().ok_or_else(|| err("not_found", "no such document"))?;
        self.count_seat(id, cfg);
        Ok((role, d))
    }

    fn count_seat(&self, id: &Identity, cfg: &Config) {
        let today = chrono::Local::now().date_naive();
        let Ok(mut s) = self.seats.lock() else { return };
        let changed = s.record(&id.login, &id.device, today);
        let members: Vec<String> = cfg.members.iter().map(|m| m.login.clone()).collect();
        let (_, line) = s.check(&members, cfg.tier.max_members, today);
        if changed || line.is_some() {
            let _ = s.save(&self.dir);
        }
        if let Some(l) = line {
            drop(s);
            self.log(&l);
            self.audit.write(json!({"login": "", "op": "seats", "result": l}));
        }
    }

    pub fn seats(&self, cfg: &Config) -> otherwise_remote::Seats {
        let today = chrono::Local::now().date_naive();
        let s = self.seats.lock().map(|s| s.clone()).unwrap_or_default();
        let members: Vec<String> = cfg.members.iter().map(|m| m.login.clone()).collect();
        otherwise_remote::Seats { active: s.active(&members, today).len() as u32, max: cfg.tier.max_members, over_since: s.over_since.clone() }
    }

    fn docs(&self, id: &Identity, cfg: &Config, app: &str) -> Result<Response, ApiError> {
        if cfg.member(&id.login).is_none() {
            return Err(err("forbidden", "you aren't a member of this server: ask its owner"));
        }
        let docs: Vec<DocInfo> = cfg.docs.iter().filter(|d| d.app == app).filter_map(|d| cfg.role_on(&id.login, app, &d.id).map(|role| DocInfo { id: d.id.clone(), name: d.name.clone(), role, revision: None })).collect();
        Ok(Response::json(200, &json!({"docs": docs})))
    }

    fn call(&self, d: &config::Doc, req: Value) -> Result<Value, ApiError> {
        self.sup.call(&d.app, &d.id, &d.path, req, CALL_TIMEOUT)
    }

    fn doc_hello(&self, id: &Identity, cfg: &Config, app: &str, doc: &str) -> Result<Response, ApiError> {
        let (role, d) = self.role(id, cfg, app, doc)?;
        let v = self.call(&d, json!({"op": "hello"}))?;
        let r = v.get("result").cloned().unwrap_or_default();
        let open = r.get("writable").and_then(Value::as_bool).unwrap_or(false);
        let reason = if !role.can_write() {
            Some("role".to_string())
        } else if !open {
            Some("read_only".to_string())
        } else {
            None
        };
        let h = DocHello {
            app: app.into(),
            doc: doc.into(),
            name: d.name.clone(),
            role,
            writable: reason.is_none(),
            revision: r.get("revision").and_then(Value::as_i64).unwrap_or(0).max(0) as u64,
            protocol: r.get("protocol").and_then(Value::as_u64).unwrap_or(0) as u32,
            read_only_reason: reason,
            seats: (role == Role::Owner).then(|| self.seats(cfg)),
        };
        Ok(Response::json(200, &serde_json::to_value(h).unwrap_or_default()))
    }

    fn snapshot(&self, req: &Request, id: &Identity, cfg: &Config, app: &str, doc: &str) -> Result<Response, ApiError> {
        let (_, d) = self.role(id, cfg, app, doc)?;
        let v = self.call(&d, json!({"op": "hello"}))?;
        let rev = v.get("result").and_then(|r| r.get("revision")).and_then(Value::as_i64).unwrap_or(0);
        let etag = req.header("if-none-match").map(|s| s.trim_start_matches("W/").trim_matches('"').to_string());
        if etag.as_deref() == Some(rev.to_string().as_str()) {
            return Ok(Response::empty(304).header("ETag", &format!("\"{rev}\"")));
        }
        let cache = self.dir.join("cache").join(app).join(doc);
        std::fs::create_dir_all(&cache).map_err(|e| err("failed", &e.to_string()))?;
        let _ = config::private_dir(&self.dir.join("cache"));
        let file = cache.join(format!("{rev}.db"));
        let (rev, file) = if file.is_file() {
            (rev, file)
        } else {
            let tmp = cache.join(format!(".{}-{}.db", std::process::id(), otherwise_remote::new_key()));
            let v = self.call(&d, json!({"op": "snapshot", "to": tmp.to_string_lossy()}))?;
            if v.get("ok").and_then(Value::as_bool) != Some(true) {
                let _ = std::fs::remove_file(&tmp);
                return Err(app_error(&v));
            }
            let got = v["result"]["revision"].as_i64().unwrap_or(rev);
            let dest = cache.join(format!("{got}.db"));
            let _ = config::private_file(&tmp);
            std::fs::rename(&tmp, &dest).map_err(|e| err("failed", &e.to_string()))?;
            // older copies go (the newest two stay for readers mid-download)
            let mut old: Vec<(i64, PathBuf)> = std::fs::read_dir(&cache).into_iter().flatten().flatten().filter_map(|e| Some((e.file_name().to_string_lossy().strip_suffix(".db")?.parse::<i64>().ok()?, e.path()))).collect();
            old.sort();
            for (_, p) in old.iter().rev().skip(2) {
                let _ = std::fs::remove_file(p);
            }
            (got, dest)
        };
        let f = std::fs::File::open(&file).map_err(|e| err("failed", &e.to_string()))?;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        self.audit.write(json!({"login": id.login, "device": id.device, "app": app, "doc": doc, "op": "snapshot", "result": "ok", "revision": rev}));
        Ok(Response { status: 200, headers: vec![("Content-Type".into(), "application/vnd.sqlite3".into()), ("ETag".into(), format!("\"{rev}\"")), ("X-Otherwise-Revision".into(), rev.to_string())], body: http::Body::File(f, len) })
    }

    fn events(&self, req: &Request, id: &Identity, cfg: &Config, app: &str, doc: &str) -> Result<Response, ApiError> {
        let (_, d) = self.role(id, cfg, app, doc)?;
        if self.streams.load(Ordering::Relaxed) >= MAX_STREAMS {
            return Err(err("unavailable", "too many live streams"));
        }
        let (rx, rev) = self.sup.subscribe(&d.app, &d.id, &d.path)?;
        let last: Option<i64> = req.header("last-event-id").and_then(|s| s.parse().ok());
        let streams = self.streams.clone();
        streams.fetch_add(1, Ordering::Relaxed);
        Ok(Response {
            status: 200,
            headers: vec![],
            body: http::Body::Stream(Box::new(move |s: &mut TcpStream| {
                stream_events(s, rx, rev, last);
                streams.fetch_sub(1, Ordering::Relaxed);
            })),
        })
    }

    fn ops(&self, req: &Request, id: &Identity, cfg: &Config, app: &str, doc: &str) -> Result<Response, ApiError> {
        let (role, d) = self.role(id, cfg, app, doc)?;
        let op: OpRequest = serde_json::from_slice(&req.body).map_err(|e| err("bad_request", &e.to_string()))?;
        if !role.can_write() {
            return Err(err("forbidden", "your role on this document is read-only"));
        }
        if !idem::valid_key(&op.idempotency_key) {
            return Err(err("bad_request", "idempotencyKey: 16-128 characters of [A-Za-z0-9_-]"));
        }
        if op.op.is_empty() || op.op.len() > 64 || !op.op.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(err("unknown_op", &op.op));
        }
        let source = if role == Role::Agent { "mcp" } else { "remote" };
        let mut child = json!({"op": "run", "name": op.op, "args": op.args, "source": source, "command": op.command.clone().unwrap_or_else(|| op.op.clone()), "actor": id.actor()});
        if let Some(b) = op.base_revision {
            child["base_revision"] = json!(b);
        }
        let args_sha = audit::sha256(&op.args);
        self.write_once(id, &op.idempotency_key, &d, child, &op.op, &args_sha)
    }

    fn undo(&self, req: &Request, id: &Identity, cfg: &Config, app: &str, doc: &str) -> Result<Response, ApiError> {
        let (role, d) = self.role(id, cfg, app, doc)?;
        let u: UndoRequest = serde_json::from_slice(&req.body).map_err(|e| err("bad_request", &e.to_string()))?;
        if !role.can_write() {
            return Err(err("forbidden", "your role on this document is read-only"));
        }
        if !idem::valid_key(&u.idempotency_key) {
            return Err(err("bad_request", "idempotencyKey: 16-128 characters of [A-Za-z0-9_-]"));
        }
        let cid = u.change_id.clone().unwrap_or_default();
        let child = json!({"op": "undo", "id": cid, "actor": id.actor(), "any": role.undoes_anyones() && !cid.is_empty()});
        self.write_once(id, &u.idempotency_key, &d, child, "undo", &audit::sha256(&json!(cid)))
    }

    /// A write through the document's process, answered once per idempotency key.
    fn write_once(&self, id: &Identity, key: &str, d: &config::Doc, child: Value, op: &str, args_sha: &str) -> Result<Response, ApiError> {
        let now = chrono::Utc::now().timestamp();
        match self.idem.lock().map_err(|_| err("failed", "lock"))?.begin(&id.login, key, now) {
            idem::Begin::Replay(s) => {
                let mut body = s.body;
                if body.get("ok").and_then(Value::as_bool) == Some(true) {
                    body["replayed"] = json!(true);
                }
                return Ok(Response::json(s.status, &body));
            }
            idem::Begin::Busy => return Err(err("busy", "this request is being answered; retry")),
            idem::Begin::New => {}
        }
        let res = self.call(d, child);
        let (resp, stored) = match res {
            Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => {
                let result = v.get("result").cloned().unwrap_or_default();
                let change_id = result.get("change").or_else(|| result.get("undone")).and_then(Value::as_str).map(str::to_string);
                let out = OpResult { ok: true, result, revision: v.get("revision").and_then(Value::as_i64).unwrap_or(0).max(0) as u64, change_id: change_id.clone(), replayed: false };
                let body = serde_json::to_value(&out).unwrap_or_default();
                self.audit.write(json!({"login": id.login, "device": id.device, "app": d.app, "doc": d.id, "op": op, "argsSha256": args_sha, "result": "ok", "revision": out.revision, "changeId": change_id}));
                (Response::json(200, &body), Some(idem::Stored { at: now, status: 200, body }))
            }
            Ok(v) => {
                let e = app_error(&v);
                self.audit.write(json!({"login": id.login, "device": id.device, "app": d.app, "doc": d.id, "op": op, "argsSha256": args_sha, "result": e.code}));
                (Response::error(&e), Some(idem::Stored { at: now, status: e.status(), body: e.envelope() }))
            }
            Err(e) => (Response::error(&e), None),
        };
        if let Ok(mut i) = self.idem.lock() {
            i.finish(&id.login, key, now, stored);
        }
        Ok(resp)
    }

    fn status(&self, id: &Identity, cfg: &Config) -> Result<Response, ApiError> {
        self.owner(id, cfg)?;
        Ok(Response::json(200, &self.status_json(cfg)))
    }

    pub fn status_json(&self, cfg: &Config) -> Value {
        let running: Vec<Value> = self.sup.running().into_iter().map(|(a, d, pid, subs)| json!({"app": a, "doc": d, "pid": pid, "subscribers": subs})).collect();
        let today = chrono::Local::now().date_naive();
        let members: Vec<String> = cfg.members.iter().map(|m| m.login.clone()).collect();
        let devices = self.seats.lock().map(|s| s.devices(&members, today)).unwrap_or(0);
        json!({"version": VERSION, "host": self.host, "port": cfg.port, "docs": cfg.docs, "members": cfg.members, "tier": cfg.tier, "seats": self.seats(cfg), "devices": devices, "running": running, "telemetry": "off: seats are counted and logged on this computer only"})
    }

    /// Nightly copies, once a day after `backups.at`.
    pub fn backups_due(&self, cfg: &Config, now: chrono::DateTime<chrono::Local>) -> Vec<String> {
        if !cfg.backups.enabled || cfg.docs.is_empty() {
            return Vec::new();
        }
        let at = chrono::NaiveTime::parse_from_str(&cfg.backups.at, "%H:%M").unwrap_or(chrono::NaiveTime::from_hms_opt(3, 30, 0).expect("valid"));
        if now.time() < at {
            return Vec::new();
        }
        let today = now.date_naive().format("%Y-%m-%d").to_string();
        let stamp = self.dir.join("backup-day");
        if std::fs::read_to_string(&stamp).ok().as_deref() == Some(today.as_str()) {
            return Vec::new();
        }
        let _ = std::fs::write(&stamp, &today);
        let base = cfg.backups.dir.clone().unwrap_or_else(backup::default_dir);
        let mut out = Vec::new();
        for d in &cfg.docs {
            if base.starts_with(&d.path) {
                out.push(format!("backup {}/{}: the backup folder is inside the document", d.app, d.id));
                continue;
            }
            let dir = base.join(&d.app).join(&d.id);
            let dest = dir.join(format!("{today}.db"));
            if dest.exists() {
                continue;
            }
            if std::fs::create_dir_all(&dir).is_err() {
                continue;
            }
            let _ = config::private_dir(&base);
            let r = self.call(d, json!({"op": "snapshot", "to": dest.to_string_lossy()}));
            let ok = matches!(&r, Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true));
            if ok {
                let _ = config::private_file(&dest);
                let gone = backup::prune(&dir, &cfg.backups);
                out.push(format!("backup {}/{}: {} ({} old copies removed)", d.app, d.id, dest.display(), gone.len()));
            } else {
                out.push(format!("backup {}/{}: failed", d.app, d.id));
            }
        }
        out
    }
}

/// An app's error answer as an API error (code, message and extra fields kept).
fn app_error(v: &Value) -> ApiError {
    let e = v.get("error").cloned().unwrap_or_default();
    serde_json::from_value::<ApiError>(e).unwrap_or_else(|_| err("failed", "the app answered with an error"))
}

fn stream_events(s: &mut TcpStream, rx: std::sync::mpsc::Receiver<Event>, rev: i64, last: Option<i64>) {
    let send = |s: &mut TcpStream, e: &Event| -> bool {
        let id = match e {
            Event::Changed { revision, .. } => Some(revision.to_string()),
            _ => None,
        };
        let data = serde_json::to_string(e).unwrap_or_default();
        s.write_all(otherwise_remote::sse::format(e.name(), id.as_deref(), &data).as_bytes()).and_then(|_| s.flush()).is_ok()
    };
    if last != Some(rev) && rev >= 0 && !send(s, &Event::Changed { revision: rev as u64, tables: vec![], actor: None }) {
        return;
    }
    let start = Instant::now();
    loop {
        if start.elapsed() >= STREAM_ROTATE {
            let _ = send(s, &Event::Bye { reason: "rotate".into() });
            return;
        }
        match rx.recv_timeout(KEEPALIVE) {
            Ok(e) => {
                let bye = matches!(e, Event::Bye { .. });
                if !send(s, &e) || bye {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if s.write_all(otherwise_remote::sse::KEEPALIVE.as_bytes()).and_then(|_| s.flush()).is_err() {
                    return;
                }
            }
            Err(_) => {
                let _ = send(s, &Event::Bye { reason: "restart".into() });
                return;
            }
        }
    }
}

fn host_name() -> String {
    let mut buf = [0u8; 256];
    #[cfg(unix)]
    {
        // SAFETY: buf is valid for its length.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let h = String::from_utf8_lossy(&buf[..n]).into_owned();
            return h.split('.').next().unwrap_or(&h).to_string();
        }
    }
    let _ = &mut buf;
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// Bind `127.0.0.1:<port>`. Anything but loopback is refused by construction.
pub fn bind(port: u16) -> std::io::Result<TcpListener> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    assert!(addr.ip().is_loopback());
    TcpListener::bind(addr)
}

/// Accept and answer until `server.quit`; a sweeper thread stops idle processes and takes the
/// nightly backups.
pub fn run(server: Arc<Server>, listener: TcpListener) -> std::io::Result<()> {
    let local = listener.local_addr()?;
    server.log(&format!("listening on {local} (loopback only; publish it with tailscale serve)"));
    {
        let s = server.clone();
        std::thread::Builder::new().name("server-sweep".into()).spawn(move || {
            while !s.quit.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(10));
                for l in s.sup.sweep() {
                    s.log(&l);
                }
                let cfg = s.config();
                for l in s.backups_due(&cfg, chrono::Local::now()) {
                    s.log(&l);
                }
            }
        })?;
    }
    listener.set_nonblocking(false)?;
    for conn in listener.incoming() {
        if server.quit.load(Ordering::Relaxed) {
            break;
        }
        let Ok(conn) = conn else { continue };
        let s = server.clone();
        let _ = std::thread::Builder::new().name("server-conn".into()).spawn(move || serve_conn(&s, conn, local));
    }
    server.sup.stop_all("shutdown");
    Ok(())
}

fn serve_conn(server: &Server, conn: TcpStream, local: SocketAddr) {
    let Ok(peer) = conn.peer_addr() else { return };
    if !peer.ip().is_loopback() {
        return;
    }
    let _ = conn.set_read_timeout(Some(Duration::from_secs(75)));
    let _ = conn.set_nodelay(true);
    let Ok(mut out) = conn.try_clone() else { return };
    let mut reader = std::io::BufReader::new(conn);
    // the connecting process is checked once per connection (it can't change); each request
    // still carries its own identity, confirmed with whois
    let mut peer_is_tailscale: Option<bool> = None;
    loop {
        let req = match http::read_request(&mut reader, peer) {
            Ok(r) => r,
            Err(http::ReadError::Closed) | Err(http::ReadError::Io(_)) => return,
            Err(http::ReadError::TooLarge) => {
                let _ = http::write_response(&mut out, Response::error(&err("too_large", "request too large")), false);
                return;
            }
            Err(http::ReadError::Bad) => {
                let _ = http::write_response(&mut out, Response::error(&err("bad_request", "bad request")), false);
                return;
            }
        };
        let keep = req.keep_alive();
        let ok = *peer_is_tailscale.get_or_insert_with(|| server.peer_ok(peer, local));
        let resp = server.handle(&req, ok);
        match http::write_response(&mut out, resp, keep) {
            Ok(true) => continue,
            _ => return,
        }
    }
}

//! The server over real loopback HTTP with a fake Tailscale and a fake app (`serve` as a small
//! shell script speaking the app socket protocol). Never the network, never a real app.
use otherwise_remote::Role;
use otherwise_server::config::{Config, DocRole, NoneTag};
use otherwise_server::identity::{Tailnet, Whois};
use otherwise_server::supervisor::{Launcher, Spawn};
use otherwise_server::{Server, bind, run};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FAKE_APP: &str = r#"#!/bin/sh
# answers hello / run / undo / snapshot like `expense serve --stdio`
rev=1
while IFS= read -r line; do
  seq=$(printf '%s' "$line" | sed -n 's/.*"seq":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"op":"hello"'*) printf '{"ok":true,"result":{"revision":%s,"writable":true,"protocol":1},"seq":%s}\n' "$rev" "$seq" ;;
    *'"name":"close_month"'*'"source":"mcp"'*) printf '{"ok":false,"error":{"code":"people_only","message":"a person closes months","op":"close_month"},"seq":%s}\n' "$seq" ;;
    *'"op":"run"'*) rev=$((rev+1)); printf '{"event":"changed","revision":%s,"actor":"someone"}\n' "$rev"; printf '{"ok":true,"result":{"change":"c%s"},"revision":%s,"seq":%s}\n' "$rev" "$rev" "$seq" ;;
    *'"op":"undo"'*) rev=$((rev+1)); printf '{"ok":true,"result":{"undone":"c2"},"revision":%s,"seq":%s}\n' "$rev" "$seq" ;;
    *'"op":"snapshot"'*) to=$(printf '%s' "$line" | sed -n 's/.*"to":"\([^"]*\)".*/\1/p'); printf 'SQLite format 3 rev %s' "$rev" > "$to"; printf '{"ok":true,"result":{"revision":%s},"seq":%s}\n' "$rev" "$seq" ;;
    *) printf '{"ok":false,"error":{"code":"bad_request","message":"?"},"seq":%s}\n' "$seq" ;;
  esac
done
"#;

struct FakeTailnet {
    tailscale: Arc<AtomicBool>,
}

impl Tailnet for FakeTailnet {
    fn whois(&self, ip: IpAddr) -> Option<Whois> {
        let (login, device) = match ip.to_string().as_str() {
            "100.100.1.2" => ("ana@example.com", "laptop"),
            "100.100.1.3" => ("bob@example.com", "pc"),
            "100.100.1.4" => ("tag:agent", "bot"),
            "100.100.1.5" => ("mallory@example.com", "evil"),
            _ => return None,
        };
        Some(Whois { login: login.into(), name: String::new(), device: device.into() })
    }
    fn peer_is_tailscale(&self, _: SocketAddr, _: SocketAddr) -> bool {
        self.tailscale.load(Ordering::Relaxed)
    }
}

struct FakeApp {
    exe: PathBuf,
    idle: Duration,
}

impl Launcher for FakeApp {
    fn plan(&self, app: &str) -> Option<Spawn> {
        (app == "expense").then(|| Spawn { program: self.exe.clone(), args: vec![], memory_mb: Some(400), idle: self.idle, protocol: 1 })
    }
    fn apps(&self) -> Vec<String> {
        vec!["expense".into()]
    }
}

struct Env {
    _t: tempfile::TempDir,
    addr: SocketAddr,
    server: Arc<Server>,
    tailscale: Arc<AtomicBool>,
    dir: PathBuf,
}

fn setup(idle: Duration) -> Env {
    let t = tempfile::tempdir().unwrap();
    let exe = t.path().join("fake-serve");
    std::fs::write(&exe, FAKE_APP).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let book = t.path().join("Acme");
    std::fs::create_dir_all(&book).unwrap();
    let hidden = t.path().join("Hidden");
    std::fs::create_dir_all(&hidden).unwrap();
    let dir = t.path().join("server");
    let mut c = Config::default();
    c.share("expense", &book, "Acme SAS", None).unwrap();
    c.share("expense", &hidden, "Hidden", None).unwrap();
    c.set_member("ana@example.com", "Ana", Role::Owner);
    c.set_member("bob@example.com", "Bob", Role::Editor);
    c.members[1].docs.insert("expense/acme-sas".into(), DocRole::Role(Role::Viewer));
    c.members[1].docs.insert("expense/hidden".into(), DocRole::None(NoneTag::None));
    c.set_member("tag:agent", "", Role::Agent);
    c.tier.max_members = Some(2);
    c.backups.dir = Some(t.path().join("backups"));
    c.save(&dir).unwrap();
    let tailscale = Arc::new(AtomicBool::new(true));
    let server = Arc::new(Server::new(&dir, Box::new(FakeTailnet { tailscale: tailscale.clone() }), Box::new(FakeApp { exe, idle }), Box::new(|_| Some(1)), "en"));
    let l = bind(0).unwrap();
    let addr = l.local_addr().unwrap();
    let s = server.clone();
    std::thread::spawn(move || run(s, l));
    Env { _t: t, addr, server, tailscale, dir }
}

/// One request as `tailscale serve` would forward it for `ip`; (status, headers, body).
fn req(env: &Env, method: &str, path: &str, ip: Option<&str>, extra: &[(&str, &str)], body: Option<Value>) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut s = TcpStream::connect(env.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let b = body.map(|b| b.to_string()).unwrap_or_default();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: studio\r\nConnection: close\r\nContent-Length: {}\r\n", b.len());
    if let Some(ip) = ip {
        head.push_str(&format!("X-Forwarded-For: {ip}\r\n"));
    }
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(b.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    let mut len = 0;
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap();
        if k.eq_ignore_ascii_case("content-length") {
            len = v.trim().parse().unwrap();
        }
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    (status, headers, body)
}

fn json_req(env: &Env, method: &str, path: &str, ip: &str, body: Option<Value>) -> (u16, Value) {
    let (st, _, b) = req(env, method, path, Some(ip), &[], body);
    (st, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

const ANA: &str = "100.100.1.2";
const BOB: &str = "100.100.1.3";
const BOT: &str = "100.100.1.4";
const MALLORY: &str = "100.100.1.5";

#[test]
fn identity_roles_snapshots_ops_and_audit() {
    let env = setup(Duration::from_secs(600));
    // no identity, an unknown address, or a connection not from Tailscale: 401
    assert_eq!(req(&env, "GET", "/v1/hello", None, &[], None).0, 401);
    assert_eq!(json_req(&env, "GET", "/v1/hello", "100.100.9.9", None).0, 401);
    env.tailscale.store(false, Ordering::Relaxed);
    assert_eq!(json_req(&env, "GET", "/v1/hello", ANA, None).0, 401, "a CI runner on loopback");
    env.tailscale.store(true, Ordering::Relaxed);

    let (st, h) = json_req(&env, "GET", "/v1/hello", ANA, None);
    assert_eq!(st, 200);
    assert_eq!((h["server"].as_str(), h["me"]["role"].as_str(), h["apps"][0]["id"].as_str()), (Some("otherwise"), Some("owner"), Some("expense")));
    let (_, h) = json_req(&env, "GET", "/v1/hello", MALLORY, None);
    assert!(h.get("me").is_none() && h["apps"].as_array().unwrap().is_empty(), "a non-member learns nothing: {h}");

    // documents per member and role
    let (_, d) = json_req(&env, "GET", "/v1/apps/expense/docs", BOB, None);
    assert_eq!(d["docs"].as_array().unwrap().iter().map(|d| (d["id"].as_str().unwrap(), d["role"].as_str().unwrap())).collect::<Vec<_>>(), [("acme-sas", "viewer")]);
    assert_eq!(json_req(&env, "GET", "/v1/apps/expense/docs", MALLORY, None).0, 403);
    assert_eq!(json_req(&env, "GET", "/v1/apps/expense/docs/hidden/hello", BOB, None).0, 404, "hidden is 404, not 403");
    assert_eq!(json_req(&env, "GET", "/v1/apps/expense/docs/nope/hello", ANA, None).0, 404);

    let (st, dh) = json_req(&env, "GET", "/v1/apps/expense/docs/acme-sas/hello", BOB, None);
    assert_eq!(st, 200);
    assert_eq!((dh["writable"].as_bool(), dh["readOnlyReason"].as_str()), (Some(false), Some("role")));
    assert!(dh.get("seats").is_none(), "seats are the owner's");
    let (_, dh) = json_req(&env, "GET", "/v1/apps/expense/docs/acme-sas/hello", ANA, None);
    assert_eq!((dh["writable"].as_bool(), dh["revision"].as_u64()), (Some(true), Some(1)));
    assert_eq!(dh["seats"]["active"], 2);

    // snapshot with ETag / 304
    let (st, hs, body) = req(&env, "GET", "/v1/apps/expense/docs/acme-sas/snapshot", Some(BOB), &[], None);
    assert_eq!(st, 200);
    assert!(body.starts_with(b"SQLite format 3"));
    assert!(hs.iter().any(|(k, v)| k == "etag" && v == "\"1\""));
    assert_eq!(req(&env, "GET", "/v1/apps/expense/docs/acme-sas/snapshot", Some(BOB), &[("If-None-Match", "\"1\"")], None).0, 304);

    // writes: viewers can't, owners can, a retried key writes once
    let op = json!({"op": "set_receipt", "args": {"id": "r1", "merchant": "Café Secreto"}, "idempotencyKey": "k-0123456789abcdef"});
    assert_eq!(json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", BOB, Some(op.clone())).0, 403);
    let (st, r) = json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", ANA, Some(op.clone()));
    assert_eq!(st, 200, "{r}");
    assert_eq!((r["revision"].as_u64(), r["changeId"].as_str()), (Some(2), Some("c2")));
    let (st, again) = json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", ANA, Some(op.clone()));
    assert_eq!((st, again["replayed"].as_bool(), again["revision"].as_u64()), (200, Some(true), Some(2)));
    let (_, dh) = json_req(&env, "GET", "/v1/apps/expense/docs/acme-sas/hello", ANA, None);
    assert_eq!(dh["revision"], 2, "the retry wrote nothing");
    let bad = json!({"op": "set_receipt", "args": {}, "idempotencyKey": "x"});
    assert_eq!(json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", ANA, Some(bad)).0, 400);
    let weird = json!({"op": "../hello", "args": {}, "idempotencyKey": "k-weird-0123456789"});
    assert_eq!(json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", ANA, Some(weird)).0, 400);

    // an agent can't close a month (the app refuses people-only operations for source mcp)
    let close = json!({"op": "close_month", "args": {"month": "2026-10"}, "idempotencyKey": "k-agent-0123456789"});
    let (st, e) = json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", BOT, Some(close));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("people_only")), "{e}");

    let (st, u) = json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/undo", ANA, Some(json!({"idempotencyKey": "k-undo-0123456789"})));
    assert_eq!((st, u["changeId"].as_str()), (200, Some("c2")), "{u}");

    // the audit has who and what, never the arguments
    let (st, a) = json_req(&env, "GET", "/v1/audit", ANA, None);
    assert_eq!(st, 200);
    let lines = a["lines"].as_array().unwrap();
    assert!(lines.iter().any(|l| l["op"] == "snapshot" && l["login"] == "bob@example.com"));
    assert!(lines.iter().any(|l| l["op"] == "set_receipt" && l["argsSha256"].as_str().is_some_and(|h| h.len() == 64)));
    assert!(!a.to_string().contains("Secreto"));
    assert_eq!(json_req(&env, "GET", "/v1/audit", BOB, None).0, 403);

    // seats: 3 members active, tier 2 → over, logged (only logged)
    let (_, st) = json_req(&env, "GET", "/v1/server", ANA, None);
    assert_eq!((st["seats"]["active"].as_u64(), st["seats"]["max"].as_u64()), (Some(3), Some(2)));
    assert!(st["seats"]["overSince"].is_string());
    let log = std::fs::read_to_string(env.dir.join("server.log")).unwrap();
    assert!(log.contains("3 active members in 30 days, the tier allows 2"), "{log}");
    assert!(log.contains("refused"), "refusals are logged");

    // nightly backup through the app's snapshot
    let cfg = env.server.config();
    let at = chrono::Local::now().date_naive().and_hms_opt(23, 59, 0).unwrap().and_local_timezone(chrono::Local).unwrap();
    let done = env.server.backups_due(&cfg, at);
    assert_eq!(done.len(), 2, "{done:?}");
    assert!(env.server.backups_due(&cfg, at).is_empty(), "once a day");
    assert!(Path::new(&cfg.backups.dir.clone().unwrap()).join("expense/acme-sas").read_dir().unwrap().count() == 1);
}

#[test]
fn events_follow_writes_and_idle_processes_stop() {
    let env = setup(Duration::from_millis(200));
    let mut s = TcpStream::connect(env.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(s, "GET /v1/apps/expense/docs/acme-sas/events HTTP/1.1\r\nHost: s\r\nX-Forwarded-For: {BOB}\r\nLast-Event-ID: 0\r\n\r\n").unwrap();
    let mut r = BufReader::new(s);
    let mut status = String::new();
    r.read_line(&mut status).unwrap();
    assert!(status.contains("200"), "{status}");
    let mut next_data = || loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap() == 0 {
            return None;
        }
        if let Some(d) = l.strip_prefix("data: ") {
            return Some(serde_json::from_str::<Value>(d.trim()).unwrap());
        }
    };
    let first = next_data().unwrap();
    assert_eq!((first["event"].as_str(), first["revision"].as_u64()), (Some("changed"), Some(1)), "a stale client hears the current revision at once");
    let op = json!({"op": "set_receipt", "args": {"id": "r1"}, "idempotencyKey": "k-events-0123456789"});
    assert_eq!(json_req(&env, "POST", "/v1/apps/expense/docs/acme-sas/ops", ANA, Some(op)).0, 200);
    let e = next_data().unwrap();
    assert_eq!((e["event"].as_str(), e["revision"].as_u64()), (Some("changed"), Some(2)));
    // a subscriber keeps the process; without one it stops when idle
    std::thread::sleep(Duration::from_millis(400));
    assert!(env.server.sup.sweep().is_empty(), "a live stream keeps it");
    let started = env.server.sup.running();
    assert_eq!(started.len(), 1);
}

#[test]
fn idle_sweep_and_untrusted_config() {
    let env = setup(Duration::from_millis(100));
    assert_eq!(json_req(&env, "GET", "/v1/apps/expense/docs/acme-sas/hello", ANA, None).0, 200);
    assert_eq!(env.server.sup.running().len(), 1);
    std::thread::sleep(Duration::from_millis(300));
    let done = env.server.sup.sweep();
    assert_eq!(done, ["expense/acme-sas: stopped (idle)"]);
    assert!(env.server.sup.running().is_empty());
    // started again on demand
    assert_eq!(json_req(&env, "GET", "/v1/apps/expense/docs/acme-sas/hello", ANA, None).0, 200);
    // an unreadable config shares nothing
    std::fs::write(Config::path(&env.dir), "{not json").unwrap();
    assert_eq!(json_req(&env, "GET", "/v1/apps/expense/docs/acme-sas/hello", ANA, None).0, 403);
}

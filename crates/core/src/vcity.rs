//! Vcity as a calendar source (optional): the same device-code sign-in as the `vcity` CLI
//! (`/api/cli/auth/device/code` → the user approves at `<base>/cli-auth` → `/device/token`),
//! token refresh (`/api/cli/auth/refresh`), then read-only `GET /api/v1/me` (the user's orgs),
//! `/api/v1/orgs/{org}/events` and `/api/v1/orgs/{org}/bookings` for a date range.
//!
//! Tokens are handed to the caller, who keeps them in the OS keychain only; nothing here logs
//! or prints them, and errors never include them. Redirects are refused (a redirect would
//! forward the bearer token). Tests run against a local mock server only.
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use otherwise_link::events::{Event, Kind, Status};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_BASE: &str = "https://vcity.io";
/// Keychain service name (the account is the base URL's host).
pub const KEYCHAIN_SERVICE: &str = "otherwise-apps-vcity";
const MAX_TOKEN: usize = 2048;
const MAX_CODE: usize = 512;
const MAX_BODY: u64 = 8 << 20;
const PAGE: usize = 200;
/// Pages read per org and kind (200 each), a bound against runaway pagination.
const MAX_PAGES: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The base URL isn't https (or plain http to this computer, for tests).
    UnsafeBase,
    /// The session is gone (refresh failed): sign in again.
    SignedOut,
    Status(u16),
    Transport(String),
    /// The server answered something we don't accept (sizes, shapes).
    Invalid(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnsafeBase => f.write_str("unsafe Vcity address"),
            Error::SignedOut => f.write_str("signed out"),
            Error::Status(c) => write!(f, "HTTP {c}"),
            Error::Transport(e) => f.write_str(e),
            Error::Invalid(w) => write!(f, "invalid response ({w})"),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix ms.
    pub access_expires_at: i64,
    pub refresh_expires_at: i64,
}

// never print tokens, even in debug output
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens").field("access_expires_at", &self.access_expires_at).field("refresh_expires_at", &self.refresh_expires_at).finish_non_exhaustive() // i18n-ok: debug field names
    }
}

impl Tokens {
    pub fn to_secret(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_secret(s: &str) -> Option<Tokens> {
        serde_json::from_str(s).ok().filter(|t: &Tokens| valid_token(&t.access_token) && valid_token(&t.refresh_token))
    }
}

fn valid_token(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_TOKEN && !s.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    /// Built from the trusted base, never from the server's answer.
    pub approve_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Poll {
    Pending { interval: u64 },
    Approved(Tokens),
    Expired,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Org {
    pub id: String,
    pub slug: String,
    pub name: String,
}

/// `https://…` (or `http://127.0.0.1` / `localhost`), without a trailing slash, no path.
pub fn safe_base(base: &str) -> Result<String, Error> {
    let b = base.trim().trim_end_matches('/');
    let lower = b.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        let host = r.split([':', '/']).next().unwrap_or("");
        if host != "127.0.0.1" && host != "localhost" {
            return Err(Error::UnsafeBase);
        }
        r
    } else {
        return Err(Error::UnsafeBase);
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return Err(Error::UnsafeBase);
    }
    Ok(b.to_string())
}

/// The keychain account for `base`: its host.
pub fn account(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    b.split("://").nth(1).unwrap_or(b).to_string()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(format!("otherwise-apps/{}", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn send(req: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, Value), Error> {
    let mut resp = req.map_err(|e| Error::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(MAX_BODY).read_to_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn post(base: &str, path: &str, body: Value, bearer: Option<&str>) -> Result<(u16, Value), Error> {
    let a = agent();
    let mut r = a.post(format!("{base}{path}")).header("content-type", "application/json");
    if let Some(t) = bearer {
        r = r.header("authorization", format!("Bearer {t}"));
    }
    send(r.send(body.to_string().as_bytes()))
}

fn get(base: &str, path_and_query: &str, token: &str) -> Result<(u16, Value), Error> {
    send(agent().get(format!("{base}{path_and_query}")).header("authorization", format!("Bearer {token}")).header("accept", "application/json").call())
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn str_field<'a>(v: &'a Value, k: &str, max: usize) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= max)
}

pub fn start_device(base: &str) -> Result<DeviceCode, Error> {
    let base = safe_base(base)?;
    let (status, v) = post(&base, "/api/cli/auth/device/code", serde_json::json!({}), None)?;
    if !(200..300).contains(&status) {
        return Err(Error::Status(status));
    }
    let user_code = str_field(&v, "user_code", 64).ok_or(Error::Invalid("user_code"))?.to_string();
    if user_code.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid("user_code"));
    }
    let device_code = str_field(&v, "device_code", MAX_CODE).ok_or(Error::Invalid("device_code"))?.to_string();
    let expires_in = v.get("expires_in").and_then(Value::as_u64).unwrap_or(0).clamp(30, 15 * 60);
    let interval = v.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(1, 60);
    let approve_url = format!("{base}/cli-auth?user_code={}", encode(&user_code));
    Ok(DeviceCode { device_code, user_code, approve_url, expires_in, interval })
}

fn tokens_from(v: &Value, now_ms: i64) -> Result<Tokens, Error> {
    let t = Tokens {
        access_token: str_field(v, "access_token", MAX_TOKEN).ok_or(Error::Invalid("access_token"))?.to_string(),
        refresh_token: str_field(v, "refresh_token", MAX_TOKEN).ok_or(Error::Invalid("refresh_token"))?.to_string(),
        access_expires_at: v.get("access_expires_at").and_then(Value::as_i64).ok_or(Error::Invalid("access_expires_at"))?,
        refresh_expires_at: v.get("refresh_expires_at").and_then(Value::as_i64).ok_or(Error::Invalid("refresh_expires_at"))?,
    };
    if !valid_token(&t.access_token) || !valid_token(&t.refresh_token) {
        return Err(Error::Invalid("token"));
    }
    // a minute of grace for clock skew, like the CLI
    if t.refresh_expires_at <= now_ms - 60_000 {
        return Err(Error::Invalid("expired"));
    }
    Ok(t)
}

pub fn poll(base: &str, device_code: &str, now_ms: i64) -> Result<Poll, Error> {
    let base = safe_base(base)?;
    let (status, v) = post(&base, "/api/cli/auth/device/token", serde_json::json!({"device_code": device_code}), None)?;
    if !(200..300).contains(&status) && status != 400 {
        return Err(Error::Status(status));
    }
    Ok(match v.get("status").and_then(Value::as_str) {
        Some("pending") => Poll::Pending { interval: v.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(1, 60) },
        Some("approved") => Poll::Approved(tokens_from(&v, now_ms)?),
        Some("denied") => Poll::Denied,
        Some("expired") => Poll::Expired,
        _ => return Err(Error::Invalid("status")),
    })
}

pub fn refresh(base: &str, t: &Tokens, now_ms: i64) -> Result<Tokens, Error> {
    let base = safe_base(base)?;
    let (status, v) = post(&base, "/api/cli/auth/refresh", serde_json::json!({}), Some(&t.refresh_token))?;
    if status == 401 || status == 400 || status == 403 {
        return Err(Error::SignedOut);
    }
    if !(200..300).contains(&status) {
        return Err(Error::Status(status));
    }
    tokens_from(&v, now_ms)
}

/// A session: tokens that rotate when the access token expires (the caller saves `changed`).
pub struct Session {
    pub base: String,
    pub tokens: Tokens,
    pub changed: bool,
}

impl Session {
    pub fn new(base: &str, tokens: Tokens) -> Result<Session, Error> {
        Ok(Session { base: safe_base(base)?, tokens, changed: false })
    }

    fn refresh_now(&mut self, now_ms: i64) -> Result<(), Error> {
        self.tokens = refresh(&self.base, &self.tokens, now_ms)?;
        self.changed = true;
        Ok(())
    }

    /// GET with the access token; refreshed first when it is about to expire, and once more
    /// on a 401.
    pub fn get(&mut self, path_and_query: &str, now_ms: i64) -> Result<Value, Error> {
        if self.tokens.access_expires_at <= now_ms + 30_000 {
            self.refresh_now(now_ms)?;
        }
        let (mut status, mut v) = get(&self.base, path_and_query, &self.tokens.access_token)?;
        if status == 401 {
            self.refresh_now(now_ms)?;
            (status, v) = get(&self.base, path_and_query, &self.tokens.access_token)?;
        }
        if status == 401 {
            return Err(Error::SignedOut);
        }
        if !(200..300).contains(&status) {
            return Err(Error::Status(status));
        }
        Ok(v)
    }

    pub fn orgs(&mut self, now_ms: i64) -> Result<Vec<Org>, Error> {
        let v = self.get("/api/v1/me", now_ms)?;
        let orgs = v.get("orgs").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(orgs.into_iter().filter_map(|o| serde_json::from_value::<Org>(o).ok()).filter(|o| safe_slug(&o.slug) && safe_slug(&o.id)).take(200).collect())
    }

    fn list(&mut self, org: &Org, what: &str, from_ms: i64, to_ms: i64, now_ms: i64) -> Result<Vec<Value>, Error> {
        let mut out = Vec::new();
        for page in 0..MAX_PAGES {
            let q = format!("/api/v1/orgs/{}/{what}?skip={}&limit={PAGE}&fromTs={from_ms}&toTs={to_ms}", org.id, page * PAGE);
            let v = self.get(&q, now_ms)?;
            let data = v.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
            let n = data.len();
            out.extend(data);
            if !v.get("hasMore").and_then(Value::as_bool).unwrap_or(false) || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    /// The orgs' events and bookings between two local days of `tz` as calendar events (app
    /// `vcity`, links to vcity.io). Bookings are busy-only: they carry visitors' names.
    pub fn calendar<Tz: TimeZone>(&mut self, orgs: &[Org], from: NaiveDate, to: NaiveDate, tz: &Tz, now_ms: i64) -> Result<Vec<Event>, Error> {
        let ms = |d: NaiveDate| tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default()).earliest().map(|t| t.timestamp_millis()).unwrap_or(0);
        let (from_ms, to_ms) = (ms(from), ms(to.succ_opt().unwrap_or(to)));
        let mut out = Vec::new();
        for org in orgs {
            for e in self.list(org, "events", from_ms, to_ms, now_ms)? {
                out.extend(event_of(&self.base, org, &e));
            }
            for b in self.list(org, "bookings", from_ms, to_ms, now_ms)? {
                out.extend(booking_of(&self.base, org, &b));
            }
        }
        Ok(out)
    }
}

fn safe_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn rfc(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

pub fn event_of(base: &str, org: &Org, v: &Value) -> Option<Event> {
    let id = str_field(v, "id", 256)?;
    let start = rfc(v.get("startsAt").and_then(Value::as_i64)?)?;
    let title = v.get("name").and_then(Value::as_str).unwrap_or("").chars().take(200).collect::<String>();
    let mut e = Event::new(format!("event-{}-{id}", org.slug), Kind::Meeting, title, start);
    if let Some(end) = v.get("endsAt").and_then(Value::as_i64).filter(|m| *m > 0).and_then(rfc) {
        e = e.end(end);
    }
    e.app = crate::calendar::VCITY_APP.into();
    e.category = Some(org.name.clone());
    e.link = Some(format!("{base}/orgs/{}/events", org.slug));
    if v.get("status").and_then(Value::as_str) == Some("disabled") {
        e.status = Status::Tentative;
    }
    Some(e)
}

pub fn booking_of(base: &str, org: &Org, v: &Value) -> Option<Event> {
    let id = str_field(v, "id", 256)?;
    let start = rfc(v.get("requestedDate").and_then(Value::as_i64)?)?;
    let status = match v.get("status").and_then(Value::as_str).unwrap_or("PENDING") {
        "REJECTED" | "CANCELED" => Status::Cancelled,
        "PENDING" => Status::Tentative,
        "COMPLETED" | "NO_SHOW" => Status::Done,
        _ => Status::Confirmed,
    };
    let name = v.get("name").and_then(Value::as_str).unwrap_or("").chars().take(120).collect::<String>();
    let mut e = Event::new(format!("booking-{}-{id}", org.slug), Kind::Booking, name, start).status(status).busy_only(true);
    if let Some(end) = v.get("requestedEndDate").and_then(Value::as_i64).filter(|m| *m > 0).and_then(rfc) {
        e = e.end(end);
    }
    e.timezone = v.get("timezone").and_then(Value::as_str).filter(|s| s.len() <= 64).map(str::to_string);
    e.app = crate::calendar::VCITY_APP.into();
    e.category = Some(org.name.clone());
    e.link = Some(format!("{base}/orgs/{}/bookings", org.slug));
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

    /// A tiny HTTP server: answers by path; records (method, path, authorization).
    fn mock(handler: impl Fn(&str, &str, Option<&str>, &str) -> (u16, String) + Send + Sync + 'static) -> (String, Log) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut parts = first.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let mut auth = None;
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    match k.to_ascii_lowercase().as_str() {
                        "authorization" => auth = Some(v.trim().to_string()),
                        "content-length" => len = v.trim().parse().unwrap(),
                        _ => {}
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                log2.lock().unwrap().push((method.clone(), path.clone(), auth.clone()));
                let (code, text) = handler(&method, &path, auth.as_deref(), &String::from_utf8_lossy(&body));
                let _ = write!(s, "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
            }
        });
        (base, log)
    }

    const NOW: i64 = 1_790_000_000_000;

    fn tok(a: &str, r: &str) -> String {
        serde_json::json!({"status": "approved", "access_token": a, "refresh_token": r, "access_expires_at": NOW + 3_600_000, "refresh_expires_at": NOW + 86_400_000, "user": {"id": "u1"}}).to_string()
    }

    #[test]
    fn device_flow() {
        let polls = Arc::new(Mutex::new(0));
        let p2 = polls.clone();
        let (base, _) = mock(move |_, path, _, body| match path {
            "/api/cli/auth/device/code" => (200, r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://evil.example/","expires_in":600,"interval":0}"#.into()),
            "/api/cli/auth/device/token" => {
                assert!(body.contains("dc1"));
                let mut n = p2.lock().unwrap();
                *n += 1;
                if *n == 1 { (400, r#"{"status":"pending","interval":2}"#.into()) } else { (200, tok("at1", "rt1")) }
            }
            _ => (404, "{}".into()),
        });
        let dc = start_device(&base).unwrap();
        assert_eq!(dc.approve_url, format!("{base}/cli-auth?user_code=ABCD-1234"), "never the server's verification_uri");
        assert_eq!((dc.interval, dc.expires_in), (1, 600));
        assert_eq!(poll(&base, &dc.device_code, NOW).unwrap(), Poll::Pending { interval: 2 });
        let Poll::Approved(t) = poll(&base, &dc.device_code, NOW).unwrap() else { panic!() };
        assert_eq!(t.access_token, "at1");
        assert!(!format!("{t:?}").contains("at1"), "Debug never shows tokens");
        assert_eq!(Tokens::from_secret(&t.to_secret()), Some(t));
    }

    #[test]
    fn calendar_with_refresh_on_401() {
        let (base, log) = mock(move |_, path, auth, _| {
            if path == "/api/cli/auth/refresh" {
                return if auth == Some("Bearer rt1") { (200, tok("at2", "rt2")) } else { (401, "{}".into()) };
            }
            if auth != Some("Bearer at2") {
                return (401, r#"{"error":{"code":"unauthorized"}}"#.into());
            }
            match path.split('?').next().unwrap() {
                "/api/v1/me" => (200, r#"{"user":{"id":"u1","email":"a@b.c","name":"A"},"orgs":[{"id":"o1","slug":"acme","name":"Acme"},{"id":"../x","slug":"bad slug","name":"Bad"}]}"#.into()),
                "/api/v1/orgs/o1/events" => (200, format!(r#"{{"data":[{{"id":"e1","name":"Feria","startsAt":{},"endsAt":{},"status":"active"}}],"total":1,"skip":0,"limit":200,"hasMore":false}}"#, NOW, NOW + 7_200_000)),
                "/api/v1/orgs/o1/bookings" => (200, format!(r#"{{"data":[{{"id":"b1","name":"Visitor Name","email":"v@x.y","requestedDate":{},"requestedEndDate":{},"timezone":"America/Bogota","status":"APPROVED","createdAt":0,"updatedAt":0}},{{"id":"b2","name":"Gone","email":"g@x.y","requestedDate":{},"requestedEndDate":0,"timezone":"UTC","status":"CANCELED","createdAt":0,"updatedAt":0}}],"total":2,"skip":0,"limit":200,"hasMore":false}}"#, NOW + 86_400_000, NOW + 88_200_000, NOW)),
                _ => (404, "{}".into()),
            }
        });
        let old = Tokens { access_token: "at1".into(), refresh_token: "rt1".into(), access_expires_at: NOW + 3_600_000, refresh_expires_at: NOW + 86_400_000 };
        let mut s = Session::new(&base, old).unwrap();
        let orgs = s.orgs(NOW).unwrap();
        assert_eq!(orgs, [Org { id: "o1".into(), slug: "acme".into(), name: "Acme".into() }]);
        assert!(s.changed && s.tokens.access_token == "at2");
        let day = DateTime::<Utc>::from_timestamp_millis(NOW).unwrap().date_naive();
        let evs = s.calendar(&orgs, day, day + chrono::Duration::days(7), &Utc, NOW).unwrap();
        assert_eq!(evs.len(), 3);
        assert_eq!((evs[0].kind, evs[0].title.as_str(), evs[0].link.as_deref()), (Kind::Meeting, "Feria", Some(format!("{base}/orgs/acme/events").as_str())));
        assert!(evs[1].busy_only && evs[1].kind == Kind::Booking && evs[1].body.is_none());
        assert_eq!(evs[2].status, Status::Cancelled);
        let q = log.lock().unwrap().iter().find(|(_, p, _)| p.contains("/events?")).unwrap().1.clone();
        assert!(q.contains("limit=200") && q.contains("fromTs="), "{q}");
        // a dead refresh token means signed out
        let (base2, _) = mock(|_, _, _, _| (401, "{}".into()));
        let mut s2 = Session::new(&base2, Tokens { access_token: "x".into(), refresh_token: "y".into(), access_expires_at: 0, refresh_expires_at: NOW + 1 }).unwrap();
        assert_eq!(s2.orgs(NOW), Err(Error::SignedOut));
    }

    #[test]
    fn bases() {
        assert_eq!(safe_base("https://vcity.io/").unwrap(), "https://vcity.io");
        assert!(safe_base("http://vcity.io").is_err());
        assert!(safe_base("https://vcity.io/path").is_err());
        assert!(safe_base("https://user@evil.example").is_err());
        assert!(safe_base("http://127.0.0.1:8080").is_ok());
        assert!(safe_base("ftp://x").is_err());
        assert_eq!(account("https://vcity.io"), "vcity.io");
        assert!(Tokens::from_secret(r#"{"access_token":"a b","refresh_token":"r","access_expires_at":0,"refresh_expires_at":0}"#).is_none());
    }
}

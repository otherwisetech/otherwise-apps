//! Who is asking (SPEC §12.2). `tailscale serve` adds `Tailscale-User-Login` and the peer's
//! tailnet address (`X-Forwarded-For`); the local Tailscale confirms it (`whois`). Loopback is
//! **not** trusted: other processes of this OS user (CI runners on the Studio run as the same
//! user) can connect to 127.0.0.1 and write any header, so on macOS the connecting end of every
//! connection must belong to the Tailscale app's own process.
use crate::http::Request;
use otherwise_remote::ApiError;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whois {
    /// The user's login, or the node's first tag for a tagged node.
    pub login: String,
    pub name: String,
    /// The node's short name (`macbookpro`).
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub login: String,
    pub name: String,
    pub device: String,
}

impl Identity {
    /// `<login>@<device>`, as the documents' journals record it.
    pub fn actor(&self) -> String {
        format!("{}@{}", self.login, self.device)
    }
}

pub trait Tailnet: Send + Sync {
    fn whois(&self, ip: IpAddr) -> Option<Whois>;
    /// The process at the other end of this loopback connection (`peer` → `local`) is
    /// Tailscale's.
    fn peer_is_tailscale(&self, peer: SocketAddr, local: SocketAddr) -> bool;
}

fn unauthorized(why: &str) -> ApiError {
    ApiError::new("unauthorized", why)
}

/// Tailscale's ranges: CGNAT `100.64.0.0/10` and `fd7a:115c:a1e0::/48`.
pub fn is_tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            o[0] == 100 && (64..128).contains(&o[1])
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// The identity of `req`, or why not. `peer_is_tailscale`: the connection comes from the Tailscale
/// process ([`Tailnet::peer_is_tailscale`], or a test server's `trustLoopback`).
pub fn identify(req: &Request, peer_is_tailscale: bool, t: &dyn Tailnet) -> Result<Identity, ApiError> {
    if !req.peer.ip().is_loopback() {
        return Err(unauthorized("not through tailscale serve"));
    }
    if req.header("tailscale-funnel-request").is_some() {
        return Err(unauthorized("Funnel requests are refused: this server is for the tailnet only"));
    }
    if !peer_is_tailscale {
        return Err(unauthorized("the connection doesn't come from Tailscale"));
    }
    // serve appends the real peer last; anything before it came from the client
    let ip: IpAddr = req.all("x-forwarded-for").flat_map(|v| v.split(',')).map(str::trim).filter(|s| !s.is_empty()).last().and_then(|s| s.parse().ok()).ok_or_else(|| unauthorized("no tailnet address"))?;
    if !is_tailnet(ip) {
        return Err(unauthorized("not a tailnet address"));
    }
    let w = t.whois(ip).ok_or_else(|| unauthorized("Tailscale doesn't know this address"))?;
    if let Some(h) = req.header("tailscale-user-login")
        && !h.eq_ignore_ascii_case(&w.login)
    {
        return Err(unauthorized("the identity headers don't match Tailscale"));
    }
    let name = if w.name.is_empty() { req.header("tailscale-user-name").unwrap_or("").to_string() } else { w.name };
    Ok(Identity { login: w.login.to_ascii_lowercase(), name, device: w.device })
}

/// `tailscale whois --json` output.
pub fn parse_whois(json: &str) -> Option<Whois> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let node = v.get("Node")?;
    let device = node.get("ComputedName").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).map(str::to_string).or_else(|| node.get("Name").and_then(|n| n.as_str()).map(|n| n.split('.').next().unwrap_or("").to_string()))?;
    let tag = node.get("Tags").and_then(|t| t.as_array()).and_then(|t| t.first()).and_then(|t| t.as_str()).map(str::to_string);
    let user = v.get("UserProfile");
    let login = match tag {
        Some(t) => t,
        None => user?.get("LoginName")?.as_str()?.to_string(),
    };
    if login.is_empty() {
        return None;
    }
    let name = user.and_then(|u| u.get("DisplayName")).and_then(|n| n.as_str()).unwrap_or("").to_string();
    Some(Whois { login, name, device })
}

/// This computer's Tailscale: its CLI for `whois`, `lsof` for the loopback check.
pub struct System {
    cli: Option<PathBuf>,
    cache: Mutex<HashMap<IpAddr, (Instant, Option<Whois>)>>,
}

impl Default for System {
    fn default() -> Self {
        System { cli: otherwise_remote::discover::tailscale_cli(), cache: Mutex::new(HashMap::new()) }
    }
}

impl System {
    pub fn cli(&self) -> Option<&PathBuf> {
        self.cli.as_ref()
    }
}

pub(crate) fn run_with_timeout(cmd: &mut std::process::Command, timeout: Duration) -> Option<String> {
    let mut child = cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut out, &mut s).ok().map(|_| s)
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok().flatten()
}

impl Tailnet for System {
    fn whois(&self, ip: IpAddr) -> Option<Whois> {
        if let Some((at, w)) = self.cache.lock().ok()?.get(&ip).cloned()
            && at.elapsed() < Duration::from_secs(60)
        {
            return w;
        }
        let cli = self.cli.as_ref()?;
        let w = run_with_timeout(std::process::Command::new(cli).args(["whois", "--json", &ip.to_string()]).env("TAILSCALE_BE_CLI", "1"), Duration::from_secs(4)).and_then(|s| parse_whois(&s));
        if let Ok(mut c) = self.cache.lock() {
            c.insert(ip, (Instant::now(), w.clone()));
        }
        w
    }

    fn peer_is_tailscale(&self, peer: SocketAddr, local: SocketAddr) -> bool {
        #[cfg(target_os = "macos")]
        {
            let Some(pid) = loopback_owner(peer, local) else { return false };
            return pid_path(pid).is_some_and(|p| is_tailscale_exe(&p));
        }
        #[allow(unreachable_code)]
        {
            let _ = (peer, local);
            false
        }
    }
}

/// The pid owning the client end (`peer`) of a loopback connection to `local`, from `lsof`
/// (only this user's processes are visible to it, which is the point: a CI runner is).
#[cfg(target_os = "macos")]
fn loopback_owner(peer: SocketAddr, local: SocketAddr) -> Option<i32> {
    let out = run_with_timeout(std::process::Command::new("/usr/sbin/lsof").args(["-nP", "-a", &format!("-iTCP@{}:{}", peer.ip(), peer.port()), "-sTCP:ESTABLISHED", "-F", "pn"]), Duration::from_secs(3))?;
    parse_lsof(&out, peer, local, std::process::id() as i32)
}

/// `lsof -F pn` lines: the pid whose socket is `peer->local` (not ours).
pub fn parse_lsof(out: &str, peer: SocketAddr, local: SocketAddr, me: i32) -> Option<i32> {
    let want = format!("{peer}->{local}");
    let mut pid = None;
    for l in out.lines() {
        if let Some(p) = l.strip_prefix('p') {
            pid = p.parse::<i32>().ok();
        } else if let Some(n) = l.strip_prefix('n')
            && n == want
            && let Some(p) = pid.filter(|p| *p != me)
        {
            return Some(p);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn pid_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: buf is valid for its length.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buf[..n as usize]).into_owned()))
}

/// The Tailscale app's own executables (the App Store app and its `IPNExtension`, the
/// standalone app's system extension, `tailscaled`), by path and, on macOS, by code signature:
/// Apple's App Store signature for Tailscale's bundle ids, or a Developer ID of Tailscale's team.
/// A copy elsewhere or a look-alike signed by anyone else doesn't pass.
pub fn is_tailscale_exe(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy();
    let known = s.starts_with("/Applications/Tailscale.app/") || (s.starts_with("/Library/SystemExtensions/") && s.to_ascii_lowercase().contains("tailscale")) || s == "/usr/local/bin/tailscaled" || s == "/opt/homebrew/bin/tailscaled";
    known && signed_by_tailscale(p)
}

/// Tailscale's team on Apple's developer program.
pub const TAILSCALE_TEAM: &str = "W5364U7YZB";

fn signed_by_tailscale(p: &std::path::Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        static CACHE: Mutex<Vec<(PathBuf, u64, i64, bool)>> = Mutex::new(Vec::new());
        let Ok(m) = std::fs::metadata(p) else { return false };
        let key = (p.to_path_buf(), m.ino(), m.mtime());
        if let Some(c) = CACHE.lock().ok().and_then(|c| c.iter().find(|c| (c.0.clone(), c.1, c.2) == key).map(|c| c.3)) {
            return c;
        }
        let req = format!("(anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.9] exists and (identifier \"io.tailscale.ipn.macos\" or identifier \"io.tailscale.ipn.macos.network-extension\")) or (anchor apple generic and certificate leaf[subject.OU] = {TAILSCALE_TEAM})"); // i18n-ok: code requirement
        let ok = std::process::Command::new("/usr/bin/codesign").arg("--verify").arg(format!("-R={req}")).arg(p).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        if let Ok(mut c) = CACHE.lock() {
            c.retain(|c| c.0 != key.0);
            c.push((key.0, key.1, key.2, ok));
        }
        ok
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = p;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        tailscale: bool,
    }

    impl Tailnet for Fake {
        fn whois(&self, ip: IpAddr) -> Option<Whois> {
            (ip.to_string() == "100.100.1.2").then(|| Whois { login: "Ana@Example.com".into(), name: "Ana".into(), device: "laptop".into() })
        }
        fn peer_is_tailscale(&self, _: SocketAddr, _: SocketAddr) -> bool {
            self.tailscale
        }
    }

    fn req(headers: &[(&str, &str)]) -> Request {
        Request { method: "GET".into(), path: "/v1/hello".into(), query: String::new(), headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body: vec![], peer: "127.0.0.1:50000".parse().unwrap() }
    }

    #[test]
    fn identity_needs_tailscale_all_the_way() {
        let ok = Fake { tailscale: true };
        let id_of = |r: &Request, f: &Fake, trust: bool| identify(r, trust || f.peer_is_tailscale(r.peer, "127.0.0.1:47900".parse().unwrap()), f);
        let good = req(&[("x-forwarded-for", "100.100.1.2"), ("tailscale-user-login", "ana@example.com")]);
        let id = id_of(&good, &ok, false).unwrap();
        assert_eq!((id.login.as_str(), id.actor().as_str()), ("ana@example.com", "ana@example.com@laptop"));
        // a CI runner on this computer writing the same headers
        assert_eq!(id_of(&good, &Fake { tailscale: false }, false).unwrap_err().code, "unauthorized");
        // headers that don't match whois, unknown or non-tailnet addresses, Funnel
        assert!(id_of(&req(&[("x-forwarded-for", "100.100.1.2"), ("tailscale-user-login", "bob@example.com")]), &ok, false).is_err());
        assert!(id_of(&req(&[("x-forwarded-for", "100.100.9.9")]), &ok, false).is_err());
        assert!(id_of(&req(&[("x-forwarded-for", "8.8.8.8")]), &ok, false).is_err());
        assert!(id_of(&req(&[]), &ok, false).is_err());
        assert!(id_of(&req(&[("x-forwarded-for", "100.100.1.2"), ("tailscale-funnel-request", "?1")]), &ok, false).is_err());
        // the client can't choose its address: serve appends the real one last
        assert!(id_of(&req(&[("x-forwarded-for", "100.100.1.2, 100.100.9.9")]), &ok, false).is_err());
        assert!(id_of(&req(&[("x-forwarded-for", "100.100.9.9"), ("x-forwarded-for", "100.100.1.2")]), &ok, false).is_ok());
        // test servers may skip the process check, never the rest
        assert!(id_of(&good, &Fake { tailscale: false }, true).is_ok());
        let mut far = good.clone();
        far.peer = "100.100.1.2:4000".parse().unwrap();
        assert!(id_of(&far, &ok, true).is_err(), "only through serve on loopback");
    }

    #[test]
    fn whois_and_lsof_output() {
        let w = parse_whois(r#"{"Node": {"Name": "macbookpro.t.ts.net.", "ComputedName": "macbookpro"}, "UserProfile": {"LoginName": "juan@github", "DisplayName": "Juan"}}"#).unwrap();
        assert_eq!(w, Whois { login: "juan@github".into(), name: "Juan".into(), device: "macbookpro".into() });
        let t = parse_whois(r#"{"Node": {"Name": "ci.t.ts.net.", "Tags": ["tag:ci"]}, "UserProfile": {"LoginName": "tagged-devices"}}"#).unwrap();
        assert_eq!((t.login.as_str(), t.device.as_str()), ("tag:ci", "ci"));
        assert!(parse_whois("{}").is_none());
        let peer: SocketAddr = "127.0.0.1:50123".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:47900".parse().unwrap();
        let out = "p100\nn127.0.0.1:47900->127.0.0.1:50123\np62465\nn127.0.0.1:50123->127.0.0.1:47900\n";
        assert_eq!(parse_lsof(out, peer, local, 100), Some(62465));
        assert_eq!(parse_lsof("p100\nn127.0.0.1:47900->127.0.0.1:50123\n", peer, local, 100), None);
        assert!(is_tailnet("100.64.0.1".parse().unwrap()) && !is_tailnet("100.128.0.1".parse().unwrap()) && is_tailnet("fd7a:115c:a1e0::1".parse().unwrap()));
        assert!(!is_tailscale_exe(std::path::Path::new("/tmp/Applications/Tailscale.app/x")));
    }
}

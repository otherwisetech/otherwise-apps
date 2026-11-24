//! The apps' `serve` processes, one per open document: started on demand from the app's own
//! verified executable (its manifest's `serve`, SPEC §12.8), spoken to as JSON lines, stopped
//! after being idle, killed above their memory ceiling, restarted when the app is updated.
use otherwise_remote::{ApiError, Event};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How to start one app's `serve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub memory_mb: Option<u64>,
    pub idle: Duration,
    pub protocol: u32,
}

/// Finds an app's `serve` (re-verified on every start).
pub trait Launcher: Send + Sync {
    fn plan(&self, app: &str) -> Option<Spawn>;
    /// The app's version, for `hello`.
    fn version(&self, _app: &str) -> String {
        String::new()
    }
    /// Apps installed here that can be served.
    fn apps(&self) -> Vec<String>;
}

/// The installed apps of this computer (otherwise-link discovery + `plan_serve`).
pub struct Installed;

impl Launcher for Installed {
    fn plan(&self, app: &str) -> Option<Spawn> {
        let loc = otherwise_link::apps::Locations::system();
        let spec = otherwise_link::apps::all_specs(&loc).into_iter().find(|s| s.id == app)?;
        let reg = otherwise_link::background::registered(&spec, &loc)?;
        let p = otherwise_link::background::plan_serve(&reg, &loc).ok()?;
        Some(Spawn { program: p.program, args: p.args, memory_mb: p.serve.memory_mb, idle: Duration::from_secs(p.serve.idle_secs.unwrap_or(600).clamp(30, 86_400)), protocol: p.serve.protocol })
    }

    fn version(&self, app: &str) -> String {
        let loc = otherwise_link::apps::Locations::system();
        otherwise_link::apps::all_specs(&loc).into_iter().find(|s| s.id == app).and_then(|s| otherwise_link::apps::detect_install(&s, &loc)).and_then(|i| i.version).unwrap_or_default()
    }

    fn apps(&self) -> Vec<String> {
        let loc = otherwise_link::apps::Locations::system();
        otherwise_link::background::scan(&loc).into_iter().filter(|r| r.manifest.serve.is_some()).map(|r| r.spec.id).collect()
    }
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Value>,
    seq: u64,
    spawn: Spawn,
    exe: Option<(u64, i64)>,
}

/// One document's process and its subscribers.
pub struct Proc {
    io: Mutex<Option<Running>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    revision: AtomicI64,
    last_used: Mutex<Instant>,
}

impl Proc {
    fn new() -> Proc {
        Proc { io: Mutex::new(None), subscribers: Mutex::new(Vec::new()), revision: AtomicI64::new(-1), last_used: Mutex::new(Instant::now()) }
    }

    fn broadcast(&self, e: &Event) {
        if let Ok(mut s) = self.subscribers.lock() {
            s.retain(|tx| tx.send(e.clone()).is_ok());
        }
    }

    fn subscribers(&self) -> usize {
        self.subscribers.lock().map(|s| s.len()).unwrap_or(0)
    }
}

pub type Rss = Box<dyn Fn(u32) -> Option<u64> + Send + Sync>;

pub struct Supervisor {
    launcher: Box<dyn Launcher>,
    procs: Mutex<HashMap<(String, String), Arc<Proc>>>,
    rss: Rss,
    lang: String,
    /// Where the processes' stderr goes (appended).
    log: Option<PathBuf>,
}

fn unavailable(why: &str) -> ApiError {
    ApiError::new("unavailable", why)
}

fn exe_identity(p: &Path) -> Option<(u64, i64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(p).ok()?;
        Some((m.ino(), m.mtime()))
    }
    #[cfg(not(unix))]
    {
        let m = std::fs::metadata(p).ok()?;
        Some((m.len(), m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64))
    }
}

impl Supervisor {
    pub fn new(launcher: Box<dyn Launcher>, rss: Rss, lang: &str, log: Option<PathBuf>) -> Supervisor {
        Supervisor { launcher, procs: Mutex::new(HashMap::new()), rss, lang: lang.into(), log }
    }

    pub fn launcher(&self) -> &dyn Launcher {
        self.launcher.as_ref()
    }

    fn proc(&self, app: &str, doc: &str) -> Arc<Proc> {
        let mut m = self.procs.lock().unwrap_or_else(|e| e.into_inner());
        m.entry((app.into(), doc.into())).or_insert_with(|| Arc::new(Proc::new())).clone()
    }

    fn start(&self, p: &Arc<Proc>, app: &str, path: &Path) -> Result<Running, ApiError> {
        let spawn = self.launcher.plan(app).ok_or_else(|| unavailable(&format!("{app} isn't installed on the server, or can't be served")))?;
        let mut cmd = Command::new(&spawn.program);
        cmd.args(&spawn.args).arg("--book").arg(path).env(otherwise_link::background::ENV_LANG, &self.lang).stdin(Stdio::piped()).stdout(Stdio::piped());
        match self.log.as_ref().and_then(|l| std::fs::OpenOptions::new().create(true).append(true).open(l).ok()) {
            Some(f) => cmd.stderr(f),
            None => cmd.stderr(Stdio::null()),
        };
        if let Some(dir) = spawn.program.parent() {
            cmd.current_dir(dir);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setpriority / setpgid are async-signal-safe; nothing else runs before exec.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                    libc::setpgid(0, 0);
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().map_err(|e| unavailable(&format!("can't start {app}: {e}")))?;
        let stdin = child.stdin.take().ok_or_else(|| unavailable("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| unavailable("no stdout"))?;
        let (tx, responses) = channel();
        let weak = Arc::downgrade(p);
        std::thread::Builder::new()
            .name(format!("serve-{app}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v.get("event").is_some() {
                        let Some(p) = weak.upgrade() else { break };
                        if let Some(r) = v.get("revision").and_then(Value::as_i64) {
                            p.revision.store(r, Ordering::Relaxed);
                        }
                        if let Ok(e) = serde_json::from_value::<Event>(v)
                            && e != Event::Unknown
                        {
                            p.broadcast(&e);
                        }
                    } else if tx.send(v).is_err() {
                        break;
                    }
                }
                // the process ended: subscribers reconnect (and start it again)
                if let Some(p) = weak.upgrade() {
                    p.broadcast(&Event::Bye { reason: "restart".into() });
                    p.subscribers.lock().map(|mut s| s.clear()).ok();
                }
            })
            .map_err(|e| unavailable(&e.to_string()))?;
        let exe = exe_identity(&spawn.program);
        Ok(Running { child, stdin, responses, seq: 0, spawn, exe })
    }

    /// One request to a document's process (started if needed), answered in order.
    pub fn call(&self, app: &str, doc: &str, path: &Path, mut req: Value, timeout: Duration) -> Result<Value, ApiError> {
        let p = self.proc(app, doc);
        *p.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        let mut io = p.io.lock().unwrap_or_else(|e| e.into_inner());
        if io.as_mut().is_some_and(|r| r.child.try_wait().ok().flatten().is_some()) {
            *io = None;
        }
        if io.is_none() {
            *io = Some(self.start(&p, app, path)?);
        }
        let r = io.as_mut().expect("started");
        r.seq += 1;
        let seq = r.seq;
        req["seq"] = json!(seq);
        if writeln!(r.stdin, "{req}").and_then(|_| r.stdin.flush()).is_err() {
            let _ = r.child.kill();
            *io = None;
            return Err(unavailable("the app's process ended; try again"));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match r.responses.recv_timeout(left) {
                Ok(v) if v.get("seq").and_then(Value::as_u64) == Some(seq) => {
                    if let Some(rev) = v.get("revision").or_else(|| v.get("result").and_then(|r| r.get("revision"))).and_then(Value::as_i64) {
                        p.revision.store(rev, Ordering::Relaxed);
                    }
                    return Ok(v);
                }
                Ok(_) => continue,
                Err(_) => {
                    // hung or gone: don't leave a half-answered stream behind
                    let _ = r.child.kill();
                    let _ = r.child.wait();
                    *io = None;
                    return Err(unavailable("the app's process didn't answer"));
                }
            }
        }
    }

    /// Live events of a document (its process is started so changes are seen). The current
    /// revision as last known (-1: not yet).
    pub fn subscribe(&self, app: &str, doc: &str, path: &Path) -> Result<(Receiver<Event>, i64), ApiError> {
        let v = self.call(app, doc, path, json!({"op": "hello"}), Duration::from_secs(30))?;
        let p = self.proc(app, doc);
        let rev = v.get("result").and_then(|r| r.get("revision")).and_then(Value::as_i64).unwrap_or(p.revision.load(Ordering::Relaxed));
        let (tx, rx) = channel();
        p.subscribers.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
        Ok((rx, rev))
    }

    /// Stop idle processes (no requests or subscribers for their `idle`), kill those above
    /// their memory ceiling, and restart (after `bye update`) those whose app was replaced.
    /// Returns what was done, for the log.
    pub fn sweep(&self) -> Vec<String> {
        let mut out = Vec::new();
        let procs: Vec<((String, String), Arc<Proc>)> = self.procs.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for ((app, doc), p) in procs {
            let Ok(mut io) = p.io.try_lock() else { continue };
            let Some(r) = io.as_mut() else { continue };
            let pid = r.child.id();
            let idle = p.subscribers() == 0 && p.last_used.lock().map(|t| t.elapsed() >= r.spawn.idle).unwrap_or(false);
            let over = r.spawn.memory_mb.is_some_and(|mb| (self.rss)(pid).is_some_and(|b| b > mb * 1024 * 1024));
            let updated = exe_identity(&r.spawn.program) != r.exe;
            if !(idle || over || updated) {
                continue;
            }
            let reason = if updated { "update" } else if over { "memory" } else { "idle" };
            p.broadcast(&Event::Bye { reason: reason.into() });
            let mut r = io.take().expect("checked");
            if over {
                let _ = r.child.kill();
            } else {
                drop(r.stdin);
                let start = Instant::now();
                while r.child.try_wait().ok().flatten().is_none() && start.elapsed() < Duration::from_secs(15) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = r.child.kill();
            }
            let _ = r.child.wait();
            out.push(format!("{app}/{doc}: stopped ({reason})"));
        }
        out
    }

    /// Stop everything (shutdown): `bye` to subscribers, stdin closed so each saves.
    pub fn stop_all(&self, reason: &str) {
        let procs: Vec<Arc<Proc>> = self.procs.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        for p in procs {
            p.broadcast(&Event::Bye { reason: reason.into() });
            let mut io = p.io.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut r) = io.take() {
                drop(r.stdin);
                let start = Instant::now();
                while r.child.try_wait().ok().flatten().is_none() && start.elapsed() < Duration::from_secs(15) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = r.child.kill();
                let _ = r.child.wait();
            }
        }
    }

    /// `(app, doc, pid, subscribers)` of the running processes.
    pub fn running(&self) -> Vec<(String, String, u32, usize)> {
        let procs: Vec<((String, String), Arc<Proc>)> = self.procs.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        procs.into_iter().filter_map(|((a, d), p)| p.io.try_lock().ok().and_then(|io| io.as_ref().map(|r| r.child.id())).map(|pid| (a, d, pid, p.subscribers()))).collect()
    }
}

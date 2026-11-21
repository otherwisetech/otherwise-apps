//! The Assistant through an agent command line the user already pays for: Claude Code
//! (`claude -p`), Codex (`codex exec`) or opencode (`opencode run`), run headless with the
//! Otherwise hub (`otherwise-apps mcp`) attached for that one invocation only. The user's agent
//! configuration files are never touched: Claude Code gets a temporary `--mcp-config` with
//! `--strict-mcp-config`, Codex `-c mcp_servers.otherwise.*` overrides, opencode an inline
//! `OPENCODE_CONFIG_CONTENT`. Built-in file, shell and web tools are turned off where the CLI
//! allows it, so the agent only sees the user's data through the hub's tools — which are
//! read-only unless the user enabled write tools in Settings → Hub (the hub checks `hub.json`).
//!
//! Output is read line by line (Claude Code `stream-json`, Codex `--json`, opencode
//! `--format json`) into [`StreamEvent`]s; a CLI that is not logged in is told apart so the app
//! can say which command to run. Same approach as expense's assistant.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// The MCP server name the hub gets inside the agent for the run.
pub const SERVER: &str = "otherwise";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    ClaudeCode,
    Codex,
    Opencode,
}

pub const BACKENDS: [Backend; 3] = [Backend::ClaudeCode, Backend::Codex, Backend::Opencode];

impl Backend {
    pub fn binary(self) -> &'static str {
        match self {
            Backend::ClaudeCode => "claude",
            Backend::Codex => "codex",
            Backend::Opencode => "opencode",
        }
    }

    pub fn from_binary(s: &str) -> Option<Backend> {
        BACKENDS.into_iter().find(|b| b.binary() == s)
    }

    /// Product name (not translated).
    pub fn label(self) -> &'static str {
        match self {
            Backend::ClaudeCode => "Claude Code", // i18n-ok: product name
            Backend::Codex => "Codex",            // i18n-ok: product name
            Backend::Opencode => "opencode",      // i18n-ok: product name
        }
    }

    /// What the user runs to log in.
    pub fn login_command(self) -> &'static str {
        match self {
            Backend::ClaudeCode => "claude login",      // i18n-ok: command
            Backend::Codex => "codex login",             // i18n-ok: command
            Backend::Opencode => "opencode auth login", // i18n-ok: command
        }
    }

    /// Who receives what the agent reads (shown in the privacy note).
    pub fn provider(self) -> &'static str {
        match self {
            Backend::ClaudeCode => "Anthropic", // i18n-ok: company name
            Backend::Codex => "OpenAI",          // i18n-ok: company name
            Backend::Opencode => "opencode",     // i18n-ok: product name
        }
    }
}

/// One headless run: program, arguments, environment additions, what goes to stdin, and the
/// temporary files to remove afterwards.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stdin: String,
    pub temp_files: Vec<PathBuf>,
}

/// How the agent starts the hub: this executable with `mcp`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl McpServer {
    pub fn hub(exe: &Path) -> McpServer {
        McpServer { program: exe.to_path_buf(), args: vec!["mcp".into()] }
    }
}

/// A TOML basic string (for Codex's `-c key=value` overrides, parsed as TOML).
pub fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The command line for one question. `tmp` is where Claude Code's MCP config file goes (a new
/// file, removed after the run). Nothing goes through a shell: arguments are passed as they are.
pub fn build(backend: Backend, program: &Path, server: &McpServer, prompt: &str, model: Option<&str>, tmp: &Path) -> std::io::Result<Invocation> {
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    let mut inv = Invocation { program: program.to_path_buf(), args: Vec::new(), env: Vec::new(), stdin: prompt.to_string(), temp_files: Vec::new() };
    let command = server.program.display().to_string();
    match backend {
        Backend::ClaudeCode => {
            let cfg = json!({"mcpServers": {SERVER: {"type": "stdio", "command": command, "args": server.args}}});
            let file = unique_temp(tmp, "otherwise-mcp", "json");
            write_new(&file, serde_json::to_string_pretty(&cfg).unwrap_or_default().as_bytes())?;
            inv.temp_files.push(file.clone());
            inv.args = ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence", "--strict-mcp-config", "--mcp-config"].iter().map(|s| s.to_string()).collect();
            inv.args.push(file.display().to_string());
            // the hub's tools only (write tools exist only when the user turned them on)
            inv.args.extend(["--allowedTools".to_string(), format!("mcp__{SERVER}__*"), "--tools".to_string(), String::new()]);
            if let Some(m) = model {
                inv.args.extend(["--model".to_string(), m.to_string()]);
            }
        }
        Backend::Codex => {
            inv.args = vec!["exec".into(), "--json".into(), "--skip-git-repo-check".into(), "--sandbox".into(), "read-only".into(), "--color".into(), "never".into()];
            let args: Vec<String> = server.args.iter().map(|a| toml_string(a)).collect();
            inv.args.extend(["-c".to_string(), format!("mcp_servers.{SERVER}.command={}", toml_string(&command))]);
            inv.args.extend(["-c".to_string(), format!("mcp_servers.{SERVER}.args=[{}]", args.join(", "))]);
            if let Some(m) = model {
                inv.args.extend(["--model".to_string(), m.to_string()]);
            }
            // the prompt comes on stdin
            inv.args.push("-".into());
        }
        Backend::Opencode => {
            let mut cmd = vec![command];
            cmd.extend(server.args.iter().cloned());
            let cfg = json!({
                "mcp": {SERVER: {"type": "local", "command": cmd, "enabled": true}},
                "tools": {"bash": false, "edit": false, "write": false, "patch": false, "webfetch": false, "read": false, "grep": false, "glob": false, "list": false},
                "permission": {"edit": "deny", "bash": "deny", "webfetch": "deny"},
            });
            inv.env.push(("OPENCODE_CONFIG_CONTENT".into(), cfg.to_string()));
            inv.args = vec!["run".into(), "--format".into(), "json".into()];
            if let Some(m) = model {
                inv.args.extend(["--model".to_string(), m.to_string()]);
            }
        }
    }
    Ok(inv)
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_temp(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    dir.join(format!("{stem}-{}-{nanos:08x}-{}.{ext}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
}

/// A fresh empty folder for one run (the agent's working directory: nothing to read there).
pub fn scratch_dir() -> PathBuf {
    unique_temp(&std::env::temp_dir(), "otherwise-assistant", "d")
}

/// What the agent is told before the question (English: it is for the model, not the user).
pub fn system_prompt(language: &str, today: &str) -> String {
    format!(
        "You are the assistant of Otherwise Apps, the launcher of Otherwise Technologies' desktop apps (expense, Vigía, Nodo and others). \
         Answer questions about the user's own data across their installed apps using only the tools of the MCP server \"{SERVER}\" (the Otherwise hub): \
         app tools are named <app>.<tool> (for example vigia.due, nodo.devices, expense.search_transactions), plus today (what needs attention), entities_search / entities_get (shared people, companies, devices…) and open_link. \
         Do not read files, run commands or browse the web. If a tool you need is not available, say which app would have to be installed or enabled. \
         Only change data when the user asks and a write tool is offered. Be brief and concrete; use short lists and amounts as the apps give them. \
         Today is {today}. Answer in the user's language (the app is set to \"{language}\")."
    )
}

/// The question with the conversation so far (the CLI starts fresh every time).
pub fn prompt(system: &str, history: &[(bool, String)], question: &str) -> String {
    let mut p = format!("{system}\n");
    if !history.is_empty() {
        p.push_str("\nConversation so far:\n");
        for (user, text) in history.iter().rev().take(12).rev() {
            p.push_str(if *user { "User: " } else { "Assistant: " });
            p.push_str(text.trim());
            p.push('\n');
        }
    }
    p.push_str("\nUser: ");
    p.push_str(question.trim());
    p.push('\n');
    p
}

/// Which app a tool belongs to (`None`: the hub's own tools) and its name there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub tool: String,
}

/// The hub's tool from the agent's name for it: `mcp__otherwise__vigia_due` (Claude Code
/// replaces dots), `otherwise_vigia_due` (opencode), `vigia.due` (Codex).
pub fn tool_ref(raw: &str) -> ToolRef {
    let s = raw.strip_prefix(&format!("mcp__{SERVER}__")).or_else(|| raw.strip_prefix(&format!("{SERVER}_"))).or_else(|| raw.strip_prefix(&format!("{SERVER}."))).unwrap_or(raw);
    if let Some((a, t)) = s.split_once('.').or_else(|| s.split_once("__"))
        && !a.is_empty()
        && !t.is_empty()
    {
        return ToolRef { app: Some(a.to_string()), tool: t.to_string() };
    }
    for app in otherwise_link::apps::KNOWN.iter().map(|k| k.id).filter(|id| *id != otherwise_link::apps::HUB_ID) {
        if let Some(t) = s.strip_prefix(app).and_then(|r| r.strip_prefix(['_', '-'])).filter(|t| !t.is_empty()) {
            return ToolRef { app: Some(app.to_string()), tool: t.to_string() };
        }
    }
    ToolRef { app: None, tool: s.to_string() }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// More text of the answer.
    Delta(String),
    ToolCall(ToolRef),
    ToolResult { tool: ToolRef, is_error: bool },
    /// The CLI said something went wrong.
    Error(String),
}

fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|x| x.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| content_text(x))).collect::<Vec<_>>().join(""),
        Value::Object(o) => o.get("content").map(content_text).or_else(|| o.get("text").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| v.to_string()),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Turns one CLI's output lines into events.
#[derive(Debug)]
pub struct Parser {
    backend: Backend,
    /// Claude Code: text already streamed as partial messages (the whole message repeats it).
    streamed: bool,
    /// tool_use id → tool, for results that only carry the id.
    tools: std::collections::HashMap<String, ToolRef>,
    pub final_text: String,
}

impl Parser {
    pub fn new(backend: Backend) -> Parser {
        Parser { backend, streamed: false, tools: Default::default(), final_text: String::new() }
    }

    pub fn line(&mut self, line: &str) -> Vec<StreamEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Vec::new() };
        let mut out = Vec::new();
        match self.backend {
            Backend::ClaudeCode => self.claude(&v, &mut out),
            Backend::Codex => self.codex(&v, &mut out),
            Backend::Opencode => self.opencode(&v, &mut out),
        }
        for e in &out {
            if let StreamEvent::Delta(t) = e {
                self.final_text.push_str(t);
            }
        }
        out
    }

    fn sep(&self) -> &'static str {
        if self.final_text.is_empty() { "" } else { "\n\n" }
    }

    fn claude(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        match v["type"].as_str().unwrap_or("") {
            "stream_event" => {
                let ev = &v["event"];
                if ev["type"] == "content_block_delta"
                    && ev["delta"]["type"] == "text_delta"
                    && let Some(t) = ev["delta"]["text"].as_str()
                {
                    if !self.streamed && !self.final_text.is_empty() && !self.final_text.ends_with(char::is_whitespace) {
                        out.push(StreamEvent::Delta("\n\n".into()));
                    }
                    self.streamed = true;
                    out.push(StreamEvent::Delta(t.to_string()));
                }
            }
            "assistant" => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str().unwrap_or("") {
                        "text" if !self.streamed => {
                            if let Some(t) = b["text"].as_str() {
                                out.push(StreamEvent::Delta(format!("{}{t}", self.sep())));
                            }
                        }
                        "tool_use" => {
                            let tool = tool_ref(b["name"].as_str().unwrap_or(""));
                            if let Some(id) = b["id"].as_str() {
                                self.tools.insert(id.to_string(), tool.clone());
                            }
                            out.push(StreamEvent::ToolCall(tool));
                        }
                        _ => {}
                    }
                }
                self.streamed = false;
            }
            "user" => {
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let tool = b["tool_use_id"].as_str().and_then(|id| self.tools.get(id)).cloned().unwrap_or(ToolRef { app: None, tool: String::new() });
                    out.push(StreamEvent::ToolResult { tool, is_error: b["is_error"].as_bool().unwrap_or(false) });
                }
            }
            "result" => {
                if v["is_error"].as_bool() == Some(true) || v["subtype"].as_str().is_some_and(|s| s.starts_with("error")) {
                    let msg = v["result"].as_str().map(str::to_string).or_else(|| v["errors"].as_array().map(|e| e.iter().map(content_text).collect::<Vec<_>>().join("; "))).unwrap_or_else(|| v["subtype"].as_str().unwrap_or("error").to_string());
                    out.push(StreamEvent::Error(msg));
                } else if self.final_text.is_empty()
                    && let Some(t) = v["result"].as_str()
                {
                    out.push(StreamEvent::Delta(t.to_string()));
                }
            }
            _ => {}
        }
    }

    fn codex(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let item = &v["item"];
        match v["type"].as_str().unwrap_or("") {
            "item.started" if item["type"] == "mcp_tool_call" => out.push(StreamEvent::ToolCall(tool_ref(item["tool"].as_str().unwrap_or("")))),
            "item.completed" => match item["type"].as_str().unwrap_or("") {
                "agent_message" => {
                    if let Some(t) = item["text"].as_str() {
                        out.push(StreamEvent::Delta(format!("{}{t}", self.sep())));
                    }
                }
                "mcp_tool_call" => {
                    let failed = item["status"] == "failed" || !item["error"].is_null();
                    out.push(StreamEvent::ToolResult { tool: tool_ref(item["tool"].as_str().unwrap_or("")), is_error: failed });
                }
                "error" => out.push(StreamEvent::Error(item["message"].as_str().unwrap_or("").to_string())),
                _ => {}
            },
            "turn.failed" => out.push(StreamEvent::Error(v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| v["error"].to_string()))),
            "error" => out.push(StreamEvent::Error(v["message"].as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))),
            _ => {}
        }
    }

    fn opencode(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let part = &v["part"];
        match v["type"].as_str().unwrap_or("") {
            "text" => {
                if let Some(t) = part["text"].as_str() {
                    out.push(StreamEvent::Delta(format!("{}{t}", self.sep())));
                }
            }
            "tool_use" => {
                let tool = tool_ref(part["tool"].as_str().unwrap_or(""));
                let failed = part["state"]["status"] == "error";
                out.push(StreamEvent::ToolCall(tool.clone()));
                out.push(StreamEvent::ToolResult { tool, is_error: failed });
            }
            "error" => {
                let e = &v["error"];
                out.push(StreamEvent::Error(e["data"]["message"].as_str().or(e["message"].as_str()).or(e["name"].as_str()).map(str::to_string).unwrap_or_else(|| e.to_string())));
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    /// The program wasn't found (on the login shell's PATH or the usual places).
    NotInstalled(Backend),
    /// The CLI needs `login_command()`.
    NotLoggedIn(Backend),
    Failed(String),
}

/// An error message (from the stream or stderr) that means "log in first".
pub fn is_login_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["not logged in", "please run /login", "run /login", "invalid api key", "login required", "authentication_error", "unauthorized", "codex login", "auth login", "no credentials", "not authenticated", "oauth token has expired", "please log in"].iter().any(|w| t.contains(w))
}

pub fn classify(backend: Backend, text: &str) -> AgentError {
    if is_login_error(text) { AgentError::NotLoggedIn(backend) } else { AgentError::Failed(text.trim().chars().take(600).collect()) }
}

/// `$PATH` as the user's login shell sets it (apps opened from Finder get a bare one), asked
/// once; `None` if the shell can't be run.
pub fn login_shell_path() -> Option<String> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        if cfg!(windows) {
            return std::env::var("PATH").ok();
        }
        let shell = std::env::var("SHELL").ok().filter(|s| s.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
        let out = Command::new(shell).args(["-l", "-c", "printf '%s' \"$PATH\""]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!p.is_empty()).then_some(p)
    })
    .clone()
}

/// Places these CLIs install to, for when the login shell says nothing.
pub fn usual_dirs(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [".local/bin", ".claude/local", ".opencode/bin", ".npm-global/bin", ".bun/bin", ".volta/bin", ".cargo/bin"].iter().map(|d| home.join(d)).collect();
    if cfg!(windows) {
        if let Ok(a) = std::env::var("APPDATA") {
            v.push(PathBuf::from(a).join("npm"));
        }
    } else {
        v.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].iter().map(PathBuf::from));
    }
    v
}

/// The program for `backend`: on `path` (PATH-style), else in the usual places.
pub fn find(backend: Backend, path: Option<&str>, home: &Path) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) { ["exe", "cmd"].iter().map(|e| format!("{}.{e}", backend.binary())).collect() } else { vec![backend.binary().to_string()] };
    let dirs: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    dirs.into_iter().chain(usual_dirs(home)).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// PATH for the child: the login shell's (Codex and opencode may need `node`), else ours.
fn child_path() -> String {
    login_shell_path().or_else(|| std::env::var("PATH").ok()).unwrap_or_default()
}

/// Run an invocation to the end, handing events to `on_event`. Returns the answer's text.
pub fn run(backend: Backend, inv: &Invocation, cwd: &Path, on_event: &mut dyn FnMut(StreamEvent)) -> Result<String, AgentError> {
    let mut cmd = Command::new(&inv.program);
    cmd.args(&inv.args).current_dir(cwd).env("PATH", child_path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in &inv.env {
        cmd.env(k, v);
    }
    let res = (|| {
        let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { AgentError::NotInstalled(backend) } else { AgentError::Failed(e.to_string()) })?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(inv.stdin.as_bytes());
        }
        let mut stderr = child.stderr.take();
        let err_reader = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(e) = stderr.as_mut() {
                let _ = e.take(64 * 1024).read_to_string(&mut s);
            }
            s
        });
        let mut parser = Parser::new(backend);
        let mut errors = Vec::new();
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                for e in parser.line(&line) {
                    if let StreamEvent::Error(m) = &e {
                        errors.push(m.clone());
                    }
                    on_event(e);
                }
            }
        }
        let status = child.wait().map_err(|e| AgentError::Failed(e.to_string()))?;
        let stderr = err_reader.join().unwrap_or_default();
        if let Some(e) = errors.first() {
            return Err(classify(backend, &format!("{e}\n{stderr}")));
        }
        if !status.success() && parser.final_text.trim().is_empty() {
            return Err(classify(backend, if stderr.trim().is_empty() { "exited without an answer" } else { &stderr }));
        }
        Ok(parser.final_text.clone())
    })();
    for f in &inv.temp_files {
        let _ = std::fs::remove_file(f);
    }
    res
}

/// One question: builds the command in a fresh scratch folder, runs it, removes the folder.
pub fn ask(backend: Backend, program: &Path, server: &McpServer, prompt: &str, model: Option<&str>, on_event: &mut dyn FnMut(StreamEvent)) -> Result<String, AgentError> {
    let scratch = scratch_dir();
    std::fs::create_dir_all(&scratch).map_err(|e| AgentError::Failed(e.to_string()))?;
    let res = build(backend, program, server, prompt, model, &scratch).map_err(|e| AgentError::Failed(e.to_string())).and_then(|inv| run(backend, &inv, &scratch, on_event));
    let _ = std::fs::remove_dir_all(&scratch);
    res
}

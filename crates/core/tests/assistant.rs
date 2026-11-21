//! The Assistant: command lines (temporary MCP config, never the user's), output parsing for
//! the three CLIs, login errors, lookup on PATH, fake CLI runs and the conversation store.
use otherwise_core::agent_cli::*;
use otherwise_core::conversation::{self, Conversation, Message, ToolUse};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn server() -> McpServer {
    McpServer::hub(Path::new("/Applications/Otherwise Apps.app/Contents/MacOS/otherwise-apps"))
}

fn tool(app: Option<&str>, t: &str) -> ToolRef {
    ToolRef { app: app.map(str::to_string), tool: t.to_string() }
}

#[test]
fn claude_gets_a_temporary_strict_mcp_config() {
    let dir = tempfile::tempdir().unwrap();
    let inv = build(Backend::ClaudeCode, Path::new("/usr/local/bin/claude"), &server(), "¿Qué vence?", Some("sonnet"), dir.path()).unwrap();
    assert_eq!(inv.args[0], "-p");
    for flag in ["--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--no-session-persistence", "--include-partial-messages"] {
        assert!(inv.args.iter().any(|a| a == flag), "{flag}");
    }
    let at = inv.args.iter().position(|a| a == "--allowedTools").unwrap();
    assert_eq!(inv.args[at + 1], "mcp__otherwise__*");
    let t = inv.args.iter().position(|a| a == "--tools").unwrap();
    assert_eq!(inv.args[t + 1], "", "no built-in tools");
    let m = inv.args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(inv.args[m + 1], "sonnet");
    assert_eq!(inv.stdin, "¿Qué vence?", "the prompt goes on stdin, never through a shell");
    let file = &inv.temp_files[0];
    assert!(file.starts_with(dir.path()), "the config lives in the run's scratch folder");
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(cfg["mcpServers"]["otherwise"]["command"], "/Applications/Otherwise Apps.app/Contents/MacOS/otherwise-apps");
    assert_eq!(cfg["mcpServers"]["otherwise"]["args"], serde_json::json!(["mcp"]), "no write flag: the hub follows hub.json");
    let again = build(Backend::ClaudeCode, Path::new("claude"), &server(), "x", None, dir.path()).unwrap();
    assert_ne!(inv.temp_files[0], again.temp_files[0], "a new file each run");
    assert!(!again.args.iter().any(|a| a == "--model"));
}

#[test]
fn codex_and_opencode_get_overrides_not_files() {
    let dir = tempfile::tempdir().unwrap();
    let inv = build(Backend::Codex, Path::new("codex"), &server(), "hola", None, dir.path()).unwrap();
    assert_eq!(&inv.args[..3], &["exec", "--json", "--skip-git-repo-check"]);
    let sb = inv.args.iter().position(|a| a == "--sandbox").unwrap();
    assert_eq!(inv.args[sb + 1], "read-only");
    let c: Vec<&String> = inv.args.iter().enumerate().filter(|(i, _)| *i > 0 && inv.args[i - 1] == "-c").map(|(_, a)| a).collect();
    assert_eq!(c[0], "mcp_servers.otherwise.command=\"/Applications/Otherwise Apps.app/Contents/MacOS/otherwise-apps\"");
    assert_eq!(c[1], "mcp_servers.otherwise.args=[\"mcp\"]");
    assert_eq!(inv.args.last().unwrap(), "-", "prompt from stdin");
    assert!(inv.temp_files.is_empty());
    assert_eq!(toml_string("a\\b\"\n\u{1}"), "\"a\\\\b\\\"\\n\\u0001\"");

    let inv = build(Backend::Opencode, Path::new("opencode"), &server(), "hola", Some("anthropic/claude-sonnet-5-5"), dir.path()).unwrap();
    assert_eq!(inv.args, vec!["run", "--format", "json", "--model", "anthropic/claude-sonnet-5-5"]);
    let (k, v) = &inv.env[0];
    assert_eq!(k, "OPENCODE_CONFIG_CONTENT");
    let cfg: Value = serde_json::from_str(v).unwrap();
    assert_eq!(cfg["mcp"]["otherwise"]["command"], serde_json::json!(["/Applications/Otherwise Apps.app/Contents/MacOS/otherwise-apps", "mcp"]));
    assert_eq!(cfg["tools"]["bash"], false);
    assert_eq!(cfg["permission"]["edit"], "deny");
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written for Codex or opencode");
}

#[test]
fn tool_names_map_to_apps() {
    assert_eq!(tool_ref("mcp__otherwise__vigia_due"), tool(Some("vigia"), "due"));
    assert_eq!(tool_ref("mcp__otherwise__expense_search_transactions"), tool(Some("expense"), "search_transactions"));
    assert_eq!(tool_ref("mcp__otherwise__entities_search"), tool(None, "entities_search"));
    assert_eq!(tool_ref("mcp__otherwise__today"), tool(None, "today"));
    assert_eq!(tool_ref("nodo.devices"), tool(Some("nodo"), "devices"));
    assert_eq!(tool_ref("otherwise_nodo_devices"), tool(Some("nodo"), "devices"));
    assert_eq!(tool_ref("vigia__due"), tool(Some("vigia"), "due"));
    assert_eq!(tool_ref("open_link"), tool(None, "open_link"));
}

#[test]
fn claude_stream_json() {
    let mut p = Parser::new(Backend::ClaudeCode);
    let ev: Vec<StreamEvent> = include_str!("fixtures/assistant/claude-stream.jsonl").lines().flat_map(|l| p.line(l)).collect();
    assert!(ev.contains(&StreamEvent::ToolCall(tool(Some("vigia"), "due"))), "{ev:?}");
    assert!(ev.contains(&StreamEvent::ToolResult { tool: tool(Some("vigia"), "due"), is_error: false }));
    assert!(ev.contains(&StreamEvent::ToolResult { tool: tool(None, "today"), is_error: true }));
    assert_eq!(p.final_text, "Reviso Vigía…\n\nEste mes vence el IVA bimestral (28 nov).", "partial deltas, whole messages not repeated");
    assert!(!ev.iter().any(|e| matches!(e, StreamEvent::Error(_))));

    let mut p = Parser::new(Backend::ClaudeCode);
    let ev = p.line(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#);
    let StreamEvent::Error(m) = &ev[0] else { panic!("{ev:?}") };
    assert_eq!(classify(Backend::ClaudeCode, m), AgentError::NotLoggedIn(Backend::ClaudeCode));
    assert!(p.line("not json").is_empty());
}

#[test]
fn codex_and_opencode_json() {
    let mut p = Parser::new(Backend::Codex);
    let ev: Vec<StreamEvent> = include_str!("fixtures/assistant/codex.jsonl").lines().flat_map(|l| p.line(l)).collect();
    assert_eq!(ev[0], StreamEvent::ToolCall(tool(Some("nodo"), "devices")));
    assert_eq!(ev[1], StreamEvent::ToolResult { tool: tool(Some("nodo"), "devices"), is_error: false });
    assert_eq!(p.final_text, "No hay equipos caídos.");
    let mut p = Parser::new(Backend::Codex);
    let ev = p.line(r#"{"type":"turn.failed","error":{"message":"You need to run `codex login` first"}}"#);
    assert!(matches!(&ev[0], StreamEvent::Error(m) if is_login_error(m)));

    let mut p = Parser::new(Backend::Opencode);
    let ev: Vec<StreamEvent> = include_str!("fixtures/assistant/opencode.jsonl").lines().flat_map(|l| p.line(l)).collect();
    assert_eq!(ev[0], StreamEvent::ToolCall(tool(Some("expense"), "totals_by")));
    assert_eq!(p.final_text, "En octubre: comida 1.");
    let mut p = Parser::new(Backend::Opencode);
    let ev = p.line(r#"{"type":"error","error":{"name":"ProviderAuthError","data":{"message":"No credentials found. Run opencode auth login"}}}"#);
    assert!(matches!(&ev[0], StreamEvent::Error(m) if classify(Backend::Opencode, m) == AgentError::NotLoggedIn(Backend::Opencode)));
    assert_eq!(classify(Backend::Codex, " boom "), AgentError::Failed("boom".into()));
}

#[test]
fn prompt_carries_history_and_language() {
    let sys = system_prompt("es", "2026-11-21");
    assert!(sys.contains("\"otherwise\"") && sys.contains("2026-11-21") && sys.contains("\"es\""));
    let p = prompt(&sys, &[(true, "hola".into()), (false, "¡Hola!".into())], " ¿y ahora? ");
    assert!(p.starts_with(&sys));
    assert!(p.contains("User: hola\nAssistant: ¡Hola!\n") && p.ends_with("User: ¿y ahora?\n"));
    let long: Vec<(bool, String)> = (0..30).map(|i| (i % 2 == 0, format!("m{i}"))).collect();
    let p = prompt("s", &long, "q");
    assert!(!p.contains("m17\n") && p.contains("m18\n") && p.contains("m29\n"), "only the last 12 turns");
}

#[test]
fn finds_programs_on_path_or_usual_places() {
    let home = tempfile::tempdir().unwrap();
    let bin_name = |b: &str| if cfg!(windows) { format!("{b}.exe") } else { b.to_string() };
    let usual = if cfg!(windows) { None } else { Some(home.path().join(".opencode/bin")) };
    if let Some(u) = &usual {
        std::fs::create_dir_all(u).unwrap();
        std::fs::write(u.join("opencode"), "").unwrap();
        assert_eq!(find(Backend::Opencode, Some("/nowhere"), home.path()), Some(u.join("opencode")));
    }
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join(bin_name("claude")), "").unwrap();
    assert_eq!(find(Backend::ClaudeCode, Some(&bin.display().to_string()), home.path()), Some(bin.join(bin_name("claude"))));
    assert_eq!(find(Backend::Codex, Some(&bin.display().to_string()), home.path()), None);
    assert_eq!(Backend::from_binary("codex"), Some(Backend::Codex));
}

#[cfg(unix)]
fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[cfg(unix)]
#[test]
fn runs_fake_clis_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    // echoes its own arguments' MCP config so we see it was passed, then answers
    let claude = fake(dir.path(), "claude", "cat > /dev/null\nfor a in \"$@\"; do case \"$a\" in *.json) test -f \"$a\" && echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"mcp__otherwise__vigia_due\"},{\"type\":\"text\",\"text\":\"listo\"}]}}';; esac; done\necho '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"listo\"}'\n");
    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let inv = build(Backend::ClaudeCode, &claude, &server(), "q", None, &scratch).unwrap();
    let mut seen = Vec::new();
    assert_eq!(run(Backend::ClaudeCode, &inv, &scratch, &mut |e| seen.push(e)).unwrap(), "listo");
    assert!(seen.contains(&StreamEvent::ToolCall(tool(Some("vigia"), "due"))), "{seen:?}");
    assert!(!inv.temp_files[0].exists(), "the MCP config is removed after the run");

    let missing = Invocation { program: dir.path().join("nope"), ..inv };
    assert_eq!(run(Backend::ClaudeCode, &missing, &scratch, &mut |_| {}), Err(AgentError::NotInstalled(Backend::ClaudeCode)));

    let logged_out = fake(dir.path(), "codex", "cat > /dev/null\necho 'Error: not logged in. Run codex login' >&2\nexit 1\n");
    assert_eq!(ask(Backend::Codex, &logged_out, &server(), "q", None, &mut |_| {}), Err(AgentError::NotLoggedIn(Backend::Codex)));

    let opencode = fake(dir.path(), "opencode", "test -n \"$OPENCODE_CONFIG_CONTENT\" || exit 3\ncat > /dev/null\necho '{\"type\":\"text\",\"part\":{\"text\":\"hola\"}}'\n");
    let mut text = String::new();
    let answer = ask(Backend::Opencode, &opencode, &server(), "q", None, &mut |e| {
        if let StreamEvent::Delta(d) = e {
            text.push_str(&d);
        }
    });
    assert_eq!((answer.unwrap().as_str(), text.as_str()), ("hola", "hola"));
    let failing = fake(dir.path(), "claude2", "cat > /dev/null\necho 'something broke' >&2\nexit 2\n");
    assert_eq!(ask(Backend::ClaudeCode, &failing, &server(), "q", None, &mut |_| {}), Err(AgentError::Failed("something broke".into())));
}

#[test]
fn conversations_are_saved_under_the_data_root() {
    let root = tempfile::tempdir().unwrap();
    let dir = conversation::dir_in(root.path());
    assert_eq!(dir, root.path().join("otherwise-apps").join("assistant"));
    assert!(conversation::list(&dir).is_empty(), "no folder yet: nothing, no error");
    let mut a = Conversation::new();
    a.backend = Some(Backend::Codex);
    a.push(Message { user: true, text: "¿Qué vence este mes en Otherwise SAS? Necesito el detalle completo de todo, por favor".into(), tools: vec![], error: None });
    a.push(Message { user: false, text: "El IVA.".into(), tools: vec![ToolUse { tool: tool(Some("vigia"), "due"), failed: false }], error: None });
    a.push(Message { user: false, text: String::new(), tools: vec![], error: Some("x".into()) });
    assert_eq!(a.title.chars().count(), 61);
    assert!(a.title.ends_with('…'));
    assert_eq!(a.pairs().len(), 2, "failed turns stay out of the prompt");
    a.save(&dir).unwrap();
    let mut b = Conversation::new();
    b.id = format!("{}-b", b.id);
    b.updated = a.updated + 10;
    b.push(Message { user: true, text: "Resumen de hoy".into(), tools: vec![], error: None });
    b.updated = a.updated + 10;
    b.save(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), "{").unwrap();
    std::fs::write(dir.join("newer.json"), r#"{"schema":99,"id":"n","title":"","created":0,"updated":9999999999,"messages":[]}"#).unwrap();
    let all = conversation::list(&dir);
    assert_eq!(all.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), vec!["Resumen de hoy", a.title.as_str()]);
    assert_eq!(all[1], a, "round trip");
    let saved = std::fs::read_to_string(dir.join(format!("{}.json", a.id))).unwrap();
    assert!(saved.contains("\"app\": \"vigia\"") && saved.contains("\"backend\": \"codex\""));
    let mut bad = Conversation::new();
    bad.id = "../x".into();
    assert!(bad.save(&dir).is_err(), "ids never leave the folder");
}

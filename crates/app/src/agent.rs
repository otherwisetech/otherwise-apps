//! The AI panel (otherwise-agent-panel), docked next to every page and toggled with ⌘J: the
//! user's own Claude Code, Codex or opencode with the Otherwise hub (`otherwise-apps mcp`)
//! attached for the session only. Read-only mode starts the hub with `--read-only`; the other
//! modes follow Settings → Hub (write tools only when the user turned them on there).
use crate::app::Launcher;
use crate::hub::Page;
use crate::theme::Tokens;
use gpui::*;
use otherwise_agent_panel::ui::{AgentHost, AgentPanel, PanelStyle};
use otherwise_agent_panel::{McpServer, PermissionMode};

/// `otherwise-apps mcp --read-only`: the hub with every write tool hidden and refused.
pub const READ_ONLY_FLAG: &str = "--read-only";

pub struct Host {
    pub launcher: WeakEntity<Launcher>,
}

impl AgentHost for Host {
    fn app_id(&self) -> String {
        otherwise_link::link::HUB_APP.to_string()
    }

    fn app_name(&self) -> String {
        crate::APP_NAME.to_string()
    }

    fn mcp_servers(&self, mode: PermissionMode, _cx: &App) -> Vec<McpServer> {
        let Ok(exe) = std::env::current_exe() else { return Vec::new() };
        vec![hub_server(&exe, mode)]
    }

    fn instructions(&self) -> Option<String> {
        Some(
            "This is Otherwise Hub, the launcher of Otherwise Technologies' desktop apps (expense, Vigía, Nodo and others). \
             The MCP server \"otherwise\" is the Otherwise hub: app tools are named <app>.<tool> (for example vigia.due, nodo.devices, expense.search_transactions), \
             plus today (what needs attention), calendar_events (the user's calendar across apps for a date range: due dates, payments, birthdays, audits, holidays), entities_search / entities_get (shared people, companies, devices…) and open_link. \
             If an app's tools are missing, say which app would have to be installed or enabled. Write tools exist only when the user turned them on in Settings → Hub."
                .into(),
        )
    }

    fn context(&self, cx: &App) -> Option<String> {
        let l = self.launcher.upgrade()?;
        Some(l.read(cx).agent_context(cx))
    }

    fn suggestions(&self) -> Vec<String> {
        suggestion_keys(&installed_ids()).into_iter().map(|k| i18n::t!(k)).collect()
    }
}

/// Starter questions for the apps that are installed: nothing about an app that isn't there.
/// The summary works with any app (Today, calendar).
pub fn suggestion_keys(installed: &[String]) -> Vec<&'static str> {
    let has = |id: &str| installed.iter().any(|i| i == id);
    let mut keys = vec![];
    if has("expense") {
        keys.push("app.assistant.suggest_due");
    }
    keys.push("app.assistant.suggest_today");
    if has("vigia") || has("nodo") {
        keys.push("app.assistant.suggest_down");
    }
    if has("expense") {
        keys.push("app.assistant.suggest_spend");
    }
    keys
}

/// Installed app ids (otherwise-link discovery), looked up at most every 30 s: the panel asks
/// for suggestions while it draws.
fn installed_ids() -> Vec<String> {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    type Cached = Option<(Instant, Vec<String>)>;
    static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
    let mut c = CACHE.get_or_init(|| Mutex::new(None)).lock().unwrap();
    if let Some((at, ids)) = c.as_ref()
        && at.elapsed() < Duration::from_secs(30)
    {
        return ids.clone();
    }
    let ids: Vec<String> = otherwise_link::apps::discover(&otherwise_link::apps::Locations::system()).into_iter().map(|(s, _)| s.id).collect();
    *c = Some((Instant::now(), ids.clone()));
    ids
}

/// This executable as the hub for a panel session in `mode`.
pub fn hub_server(exe: &std::path::Path, mode: PermissionMode) -> McpServer {
    let mut hub = McpServer::hub(exe);
    if mode == PermissionMode::ReadOnly {
        hub.args.push(READ_ONLY_FLAG.into());
    }
    hub
}

/// The panel in the launcher's colours.
pub fn style(t: &Tokens) -> PanelStyle {
    PanelStyle {
        background: t.canvas,
        surface: t.surface,
        raised: t.raised,
        border: t.border,
        ink: t.ink,
        muted: t.ink_muted,
        accent: t.accent,
        accent_soft: t.accent_soft,
        danger: t.danger,
        danger_soft: t.danger_soft,
        ok: t.ok,
        note: t.warn,
        note_soft: t.warn_soft,
        terminal_bg: if t.dark { rgb(0x161514).into() } else { rgb(0xFFFFFF).into() },
        terminal_fg: t.ink,
        mono_font: "Menlo".into(), // i18n-ok: font name
        dark: t.dark,
    }
}

impl Launcher {
    fn agent_panel(&self) -> Option<Entity<AgentPanel>> {
        self.panel.as_ref().and_then(|p| p.upgrade())
    }

    /// "Lo que estoy viendo": the page and what is selected on it (English, for the agent).
    pub fn agent_context(&self, cx: &App) -> String {
        let page = match self.page {
            Page::Apps => "Apps (the catalog of Otherwise apps: install, update)",
            Page::Today => "Today (what needs attention across the installed apps)",
            Page::Entities => "Entities (the shared registry of people, companies, devices)",
            Page::Calendar => return self.calendar_context().to_string(),
        };
        let mut v = serde_json::json!({ "page": page });
        if self.page == Page::Apps {
            let installed: Vec<String> = self.cards.iter().filter(|c| c.status.is_installed()).map(|c| c.entry.id.clone()).collect();
            v["installed_apps"] = serde_json::json!(installed);
            let q = self.search.read(cx).value().to_string();
            if !q.is_empty() {
                v["search"] = serde_json::json!(q);
            }
        }
        if self.page == Page::Entities
            && let Some(id) = self.hub.selected
            && let Some(r) = self.hub.rows.iter().find(|r| r.card.id == id)
        {
            v["selected_entity"] = serde_json::json!({ "id": id.to_string(), "kind": format!("{:?}", r.card.kind), "name": r.card.name });
        }
        if self.page == Page::Today {
            v["items"] = serde_json::json!(self.hub.today.events.iter().take(12).map(|e| format!("{} · {}", e.app, e.title)).collect::<Vec<_>>());
        }
        v.to_string()
    }

    /// From a Today item ("Preguntar") or a link: open the panel and ask.
    pub fn ask_about(&mut self, question: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.agent_panel() {
            p.update(cx, |p, cx| p.ask(question, window, cx));
        }
    }

    /// ⌘4 and `otherwise://otherwise-apps/assistant`.
    pub fn show_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.agent_panel() {
            p.update(cx, |p, cx| {
                if matches!(p.dock_mode(), otherwise_agent_panel::DockMode::Hidden | otherwise_agent_panel::DockMode::Bar) {
                    p.toggle(window, cx);
                    if p.dock_mode() == otherwise_agent_panel::DockMode::Bar {
                        p.set_dock(otherwise_agent_panel::DockMode::Right, cx);
                    }
                }
            });
        }
    }

    pub fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.agent_panel() {
            p.update(cx, |p, cx| p.toggle(window, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{READ_ONLY_FLAG, hub_server, suggestion_keys};
    use otherwise_agent_panel::PermissionMode;
    use std::path::Path;

    #[test]
    fn read_only_mode_starts_the_hub_read_only() {
        let exe = Path::new("/Applications/Otherwise Apps.app/Contents/MacOS/otherwise-apps");
        let ro = hub_server(exe, PermissionMode::ReadOnly);
        assert_eq!(ro.name, "otherwise");
        assert_eq!(ro.args, ["mcp", READ_ONLY_FLAG]);
        assert_eq!(hub_server(exe, PermissionMode::Ask).args, ["mcp"], "the other modes follow Settings → Hub");
        assert_eq!(hub_server(exe, PermissionMode::Auto).program, exe);
    }

    #[test]
    fn suggestions_follow_the_installed_apps() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(suggestion_keys(&[]), ["app.assistant.suggest_today"], "only the summary without apps");
        assert_eq!(suggestion_keys(&ids(&["expense"])), ["app.assistant.suggest_due", "app.assistant.suggest_today", "app.assistant.suggest_spend"]);
        assert_eq!(suggestion_keys(&ids(&["nodo"])), ["app.assistant.suggest_today", "app.assistant.suggest_down"]);
        assert_eq!(suggestion_keys(&ids(&["vigia", "rama"])), ["app.assistant.suggest_today", "app.assistant.suggest_down"]);
        assert_eq!(suggestion_keys(&ids(&["expense", "vigia", "nodo"])).len(), 4);
        // no company names or fixed months in any language
        for lang in ["es", "en", "pt", "fr"] {
            for k in suggestion_keys(&ids(&["expense", "vigia"])) {
                let t = i18n::tr_in(lang, k);
                assert!(!t.contains("SAS") && !t.to_lowercase().contains("octob") && !t.to_lowercase().contains("octub"), "{lang} {k}: {t}");
            }
        }
    }
}

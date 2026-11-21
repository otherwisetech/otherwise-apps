//! The Assistant page: questions across every installed Otherwise app, answered by the user's
//! own agent command line (Claude Code, Codex, opencode) with the hub attached for that run only
//! (`otherwise_core::agent_cli`). Conversations are kept under the data root
//! (`otherwise_core::conversation`). Read-only unless write tools are on in Settings → Hub.
use crate::app::Launcher;
use crate::hub::Page;
use crate::theme::Tokens;
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::IconName;
use gpui_component::input::{Input, InputState};
use gpui_component::text::TextView;
use otherwise_core::agent_cli::{self, AgentError, BACKENDS, Backend, McpServer, StreamEvent, ToolRef};
use otherwise_core::conversation::{self, Conversation, Message, ToolUse};
use otherwise_link::apps;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(120);

/// What the background run hands back while it streams, and at the end.
#[derive(Default)]
struct Shared {
    events: Vec<StreamEvent>,
    done: Option<Result<String, AgentError>>,
}

pub struct AssistantState {
    pub input: Entity<InputState>,
    /// `None` until the first lookup finished.
    pub found: Option<Vec<(Backend, PathBuf)>>,
    pub looking: bool,
    pub conv: Conversation,
    pub past: Vec<Conversation>,
    pub show_past: bool,
    pub busy: bool,
    pub live: String,
    pub live_tools: Vec<ToolUse>,
    pub scroll: ScrollHandle,
    /// Asked before the lookup finished (a Today shortcut): sent once it has.
    pending: Option<String>,
    shared: Arc<Mutex<Shared>>,
}

impl AssistantState {
    pub fn new(window: &mut Window, cx: &mut Context<Launcher>) -> AssistantState {
        AssistantState {
            input: cx.new(|cx| InputState::new(window, cx)),
            found: None,
            looking: false,
            conv: Conversation::new(),
            past: Vec::new(),
            show_past: false,
            busy: false,
            live: String::new(),
            live_tools: Vec::new(),
            scroll: ScrollHandle::new(),
            pending: None,
            shared: Arc::new(Mutex::new(Shared::default())),
        }
    }
}

fn suggestions() -> [String; 4] {
    [i18n::t!("app.assistant.suggest_due"), i18n::t!("app.assistant.suggest_today"), i18n::t!("app.assistant.suggest_down"), i18n::t!("app.assistant.suggest_spend")]
}

fn error_text(e: &AgentError) -> String {
    match e {
        AgentError::NotInstalled(b) => i18n::t!("app.assistant.err_missing", agent = b.label()),
        AgentError::NotLoggedIn(b) => i18n::t!("app.assistant.err_login", agent = b.label(), command = b.login_command()),
        AgentError::Failed(d) => i18n::t!("app.assistant.err_failed", detail = d),
    }
}

fn tool_label(t: &ToolRef) -> String {
    match &t.app {
        Some(a) => format!("{} · {}", apps::display_name(a), t.tool),
        None => format!("{} · {}", i18n::t!("app.assistant.hub"), t.tool),
    }
}

fn date_of(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default() // i18n-ok: date format
}

impl Launcher {
    /// The chosen agent: the saved one if it was found, else the first found.
    pub fn assistant_backend(&self) -> Option<(Backend, PathBuf)> {
        let found = self.assistant.found.as_ref()?;
        let want = self.settings.assistant_agent.as_deref().and_then(Backend::from_binary);
        want.and_then(|w| found.iter().find(|(b, _)| *b == w)).or_else(|| found.first()).cloned()
    }

    pub fn detect_agents(&mut self, cx: &mut Context<Self>) {
        if self.assistant.looking {
            return;
        }
        self.assistant.looking = true;
        let task = cx.background_executor().spawn(async move {
            let path = agent_cli::login_shell_path();
            let home = otherwise_link::paths::home();
            let found: Vec<(Backend, PathBuf)> = BACKENDS.iter().filter_map(|b| agent_cli::find(*b, path.as_deref(), &home).map(|p| (*b, p))).collect();
            (found, conversation::list(&conversation::dir()))
        });
        cx.spawn(async move |this, cx| {
            let (found, past) = task.await;
            this.update(cx, |this, cx| {
                this.assistant.found = Some(found);
                this.assistant.past = past;
                this.assistant.looking = false;
                if let Some(q) = this.assistant.pending.take() {
                    this.ask(q, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_agent(&mut self, b: Backend, cx: &mut Context<Self>) {
        self.settings = crate::settings::Settings::update(|s| s.assistant_agent = Some(b.binary().to_string()));
        cx.notify();
    }

    fn new_conversation(&mut self, cx: &mut Context<Self>) {
        if self.assistant.busy {
            return;
        }
        self.assistant.conv = Conversation::new();
        self.assistant.show_past = false;
        cx.notify();
    }

    fn open_conversation(&mut self, c: Conversation, cx: &mut Context<Self>) {
        if self.assistant.busy {
            return;
        }
        self.assistant.conv = c;
        self.assistant.show_past = false;
        cx.notify();
    }

    /// From a Today item: switch to the Assistant and ask about it.
    pub fn ask_about(&mut self, question: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant.busy {
            self.assistant.input.update(cx, |s, cx| s.set_value(question, window, cx));
        } else {
            self.assistant.conv = Conversation::new();
            self.assistant.show_past = false;
            self.ask(question, cx);
        }
        self.set_page(Page::Assistant, cx);
    }

    pub fn send_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.assistant.input.read(cx).value().trim().to_string();
        if !text.is_empty() && !self.assistant.busy && self.assistant.found.is_some() {
            self.assistant.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.ask(text, cx);
        }
    }

    fn save_conversation(&mut self) {
        let dir = conversation::dir();
        if self.assistant.conv.save(&dir).is_ok() {
            let c = self.assistant.conv.clone();
            self.assistant.past.retain(|p| p.id != c.id);
            self.assistant.past.insert(0, c);
        }
    }

    pub fn ask(&mut self, question: String, cx: &mut Context<Self>) {
        let question = question.trim().to_string();
        if question.is_empty() || self.assistant.busy {
            return;
        }
        // the lookup hasn't finished yet (a Today shortcut before visiting the page)
        if self.assistant.found.is_none() {
            self.assistant.pending = Some(question);
            self.detect_agents(cx);
            return;
        }
        let before = self.assistant.conv.pairs();
        self.assistant.conv.push(Message { user: true, text: question.clone(), tools: vec![], error: None });
        let Some((backend, program)) = self.assistant_backend() else {
            self.assistant.conv.push(Message { user: false, text: String::new(), tools: vec![], error: Some(i18n::t!("app.assistant.none")) });
            cx.notify();
            return;
        };
        let Ok(exe) = std::env::current_exe() else { return };
        self.assistant.conv.backend = Some(backend);
        self.save_conversation();
        self.assistant.busy = true;
        self.assistant.live.clear();
        self.assistant.live_tools.clear();
        *self.assistant.shared.lock().unwrap_or_else(|e| e.into_inner()) = Shared::default();
        let shared = self.assistant.shared.clone();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string(); // i18n-ok: date format
        let prompt = agent_cli::prompt(&agent_cli::system_prompt(i18n::locale(), &today), &before, &question);
        std::thread::spawn(move || {
            let sink = shared.clone();
            let res = agent_cli::ask(backend, &program, &McpServer::hub(&exe), &prompt, None, &mut |e| sink.lock().unwrap_or_else(|p| p.into_inner()).events.push(e));
            shared.lock().unwrap_or_else(|p| p.into_inner()).done = Some(res);
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let finished = this.update(cx, |this, cx| this.drain_assistant(cx)).unwrap_or(true);
                if finished {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Moves streamed events into the page; true when the run is over.
    fn drain_assistant(&mut self, cx: &mut Context<Self>) -> bool {
        let (events, done) = {
            let mut s = self.assistant.shared.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut s.events), s.done.take())
        };
        let any = !events.is_empty();
        for e in events {
            match e {
                StreamEvent::Delta(d) => self.assistant.live.push_str(&d),
                StreamEvent::ToolCall(t) => self.assistant.live_tools.push(ToolUse { tool: t, failed: false }),
                StreamEvent::ToolResult { tool, is_error: true } => {
                    if let Some(u) = self.assistant.live_tools.iter_mut().rev().find(|u| u.tool == tool) {
                        u.failed = true;
                    }
                }
                _ => {}
            }
        }
        let Some(res) = done else {
            if any {
                self.assistant.scroll.scroll_to_bottom();
                cx.notify();
            }
            return false;
        };
        let tools = std::mem::take(&mut self.assistant.live_tools);
        let live = std::mem::take(&mut self.assistant.live);
        let msg = match res {
            Ok(text) => Message { user: false, text: if text.trim().is_empty() { live } else { text }, tools, error: None },
            Err(e) => Message { user: false, text: live, tools, error: Some(error_text(&e)) },
        };
        self.assistant.conv.push(msg);
        self.assistant.busy = false;
        self.save_conversation();
        self.assistant.scroll.scroll_to_bottom();
        cx.notify();
        true
    }

    // ── rendering ───────────────────────────────────────────────────────────────────────────

    fn tool_chips(t: &Tokens, tools: &[ToolUse]) -> Div {
        div().flex().flex_wrap().gap(px(4.)).children(tools.iter().map(|u| if u.failed { ui::chip(tool_label(&u.tool), t.danger, t.danger_soft) } else { ui::chip(tool_label(&u.tool), t.accent, t.accent_soft) }))
    }

    fn render_message(&self, t: &Tokens, i: usize, m: &Message) -> AnyElement {
        if m.user {
            return div().flex().justify_end().child(div().max_w(px(560.)).px(px(12.)).py(px(8.)).rounded(px(12.)).bg(t.accent_soft).text_size(px(13.)).line_height(px(19.)).child(m.text.clone())).into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .max_w(px(720.))
            .when(!m.tools.is_empty(), |d| d.child(Self::tool_chips(t, &m.tools)))
            .when(!m.text.trim().is_empty(), |d| {
                d.child(div().px(px(12.)).py(px(8.)).rounded(px(12.)).border_1().border_color(t.border).bg(t.surface).text_size(px(13.)).child(TextView::markdown(SharedString::from(format!("msg-{i}")), m.text.clone()).selectable(true))) // i18n-ok: element id
            })
            .children(m.error.clone().map(|e| div().flex().items_start().gap(px(6.)).text_size(px(12.)).text_color(t.danger).child(ui::icon(IconName::TriangleAlert, 13., t.danger)).child(div().flex_1().child(e))))
            .into_any_element()
    }

    fn render_empty(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let mut chips = div().flex().flex_wrap().justify_center().gap(px(8.)).max_w(px(620.));
        for (i, s) in suggestions().into_iter().enumerate() {
            let q = s.clone();
            chips = chips.child(ui::outline_button(t, ("suggest", i)).child(s).on_click(cx.listener(move |this, _, _, cx| this.ask(q.clone(), cx))));
        }
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .py(px(40.))
            .child(ui::icon(IconName::Bot, 30., t.accent))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.assistant.intro_title")))
            .child(div().text_size(px(13.)).text_color(t.ink_muted).text_center().max_w(px(520.)).child(i18n::t!("app.assistant.intro_body")))
            .child(chips)
    }

    fn render_no_agent(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .py(px(40.))
            .child(ui::icon(IconName::Bot, 30., t.ink_muted))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.assistant.none_title")))
            .child(div().text_size(px(13.)).text_color(t.ink_muted).text_center().max_w(px(520.)).child(i18n::t!("app.assistant.none")))
            .child(ui::outline_button(t, "assistant-detect").child(ui::icon(IconName::Search, 14., t.ink)).child(i18n::t!("app.assistant.detect")).on_click(cx.listener(|this, _, _, cx| this.detect_agents(cx))))
    }

    fn render_past(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let mut list = div().flex().flex_col().gap(px(4.));
        if self.assistant.past.is_empty() {
            list = list.child(div().text_size(px(12.)).text_color(t.ink_muted).child(i18n::t!("app.assistant.history_empty")));
        }
        let hover = t.raised;
        for (i, c) in self.assistant.past.iter().enumerate() {
            let conv = c.clone();
            let current = c.id == self.assistant.conv.id;
            list = list.child(
                div()
                    .id(("past", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(current, |d| d.bg(t.accent_soft))
                    .hover(move |s| s.bg(hover))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).child(if c.title.is_empty() { i18n::t!("app.assistant.untitled") } else { c.title.clone() }))
                    .child(div().flex_none().text_size(px(11.)).text_color(t.ink_muted).child(date_of(c.updated)))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_conversation(conv.clone(), cx))),
            );
        }
        div().flex().flex_col().gap(px(8.)).p(px(12.)).rounded(px(12.)).border_1().border_color(t.border).bg(t.surface).child(ui::section_title(t, i18n::t!("app.assistant.history"))).child(list)
    }

    pub fn render_assistant(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let a = &self.assistant;
        let chosen = self.assistant_backend();
        let mut header = div().flex().items_center().gap(px(10.)).child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(i18n::t!("app.assistant.title")));
        if let Some(found) = a.found.as_ref().filter(|f| f.len() > 1) {
            let mut picker = ui::segmented(t);
            for (b, _) in found {
                let b = *b;
                let on = chosen.as_ref().is_some_and(|(c, _)| *c == b);
                picker = picker.child(ui::segment(t, SharedString::from(format!("agent-{}", b.binary())), on).child(b.label()).on_click(cx.listener(move |this, _, _, cx| this.set_agent(b, cx)))); // i18n-ok: element id
            }
            header = header.child(picker);
        } else if let Some((b, _)) = &chosen {
            header = header.child(ui::chip(b.label(), t.ink_muted, t.border.opacity(0.5)));
        }
        header = header
            .child(ui::outline_button(t, "assistant-history").child(i18n::t!("app.assistant.history")).on_click(cx.listener(|this, _, _, cx| {
                this.assistant.show_past = !this.assistant.show_past;
                cx.notify();
            })))
            .child(ui::outline_button(t, "assistant-new").child(ui::icon(IconName::Plus, 14., t.ink)).child(i18n::t!("app.assistant.new")).on_click(cx.listener(|this, _, _, cx| this.new_conversation(cx))));

        let writes = self.hub.settings.mcp_writes;
        let mode = if writes { i18n::t!("app.assistant.mode_writes") } else { i18n::t!("app.assistant.mode_read") };
        let privacy = match &chosen {
            Some((b, _)) => i18n::t!("app.assistant.privacy", agent = b.label(), provider = b.provider()),
            None => i18n::t!("app.assistant.privacy_none"),
        };
        let note = div().flex().items_start().gap(px(8.)).px(px(12.)).py(px(8.)).rounded(px(10.)).bg(t.update_soft).text_size(px(12.)).line_height(px(17.)).child(ui::icon(IconName::Info, 14., t.update)).child(div().flex_1().child(format!("{privacy} {mode}")));

        let mut body = div().flex().flex_col().gap(px(14.));
        if a.show_past {
            body = body.child(self.render_past(t, cx));
        }
        let no_agent = a.found.as_ref().is_some_and(|f| f.is_empty());
        if a.found.is_none() {
            body = body.child(div().py(px(40.)).text_center().text_size(px(13.)).text_color(t.ink_muted).child(i18n::t!("app.assistant.looking")));
        } else if no_agent && a.conv.is_empty() {
            body = body.child(self.render_no_agent(t, cx));
        } else if a.conv.is_empty() && !a.busy {
            body = body.child(self.render_empty(t, cx));
        }
        for (i, m) in a.conv.messages.iter().enumerate() {
            body = body.child(self.render_message(t, i, m));
        }
        if a.busy {
            let live = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .max_w(px(720.))
                .when(!a.live_tools.is_empty(), |d| d.child(Self::tool_chips(t, &a.live_tools)))
                .child(div().px(px(12.)).py(px(8.)).rounded(px(12.)).border_1().border_color(t.border).bg(t.surface).text_size(px(13.)).line_height(px(19.)).when(a.live.is_empty(), |d| d.text_color(t.ink_muted)).child(if a.live.is_empty() { i18n::t!("app.assistant.thinking") } else { a.live.clone() }));
            body = body.child(live);
        }
        let can_send = !a.busy && chosen.is_some();
        let send = ui::filled_button(t, "assistant-send", if can_send { t.accent } else { t.border_strong }).child(i18n::t!("app.assistant.send")).when(can_send, |d| d.on_click(cx.listener(|this, _, w, cx| this.send_input(w, cx))));
        let composer = div().flex().flex_none().items_center().gap(px(8.)).px(px(24.)).py(px(12.)).border_t_1().border_color(t.border).bg(t.surface).child(div().flex_1().min_w_0().child(Input::new(&a.input))).child(send);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .id("assistant-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&a.scroll)
                    .child(div().flex().flex_col().gap(px(14.)).p(px(24.)).max_w(px(900.)).child(header).child(note).child(body)),
            )
            .child(composer)
            .into_any_element()
    }
}

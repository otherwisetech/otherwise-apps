//! Settings → Otherwise data folder: where every app keeps the shared files (hub.json, the entity
//! registry, Today feeds) and, by default, its own data (`<root>/<app>/`). Moving it goes through
//! otherwise-link's `relocate` (plan → confirmation listing what moves → verified copy → pointer →
//! originals removed); this file is only the UI.
use crate::app::{Launcher, Modal};
use crate::theme::Tokens;
use crate::ui;
use gpui::*;
use otherwise_core::process;
use otherwise_link::apps::KNOWN;
use otherwise_link::lock::Holder;
use otherwise_link::relocate::{self, Blocker, ItemKind, Plan, Transfer};
use otherwise_link::{Root, paths, storage};
use std::path::PathBuf;

/// What the confirmation offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// Nothing in the way: copy or move.
    MoveOrCopy,
    /// The destination already has Otherwise data: use it (ours stays where it is).
    UseThere,
    /// Something is open: close it first.
    Busy,
}

pub fn item_label(name: &str) -> String {
    match name {
        "hub.json" => i18n::t!("app.data.item_settings"),
        "entities.db" => i18n::t!("app.data.item_entities"),
        "today" => i18n::t!("app.data.item_today"),
        n => match KNOWN.iter().find(|k| k.id == n) {
            Some(k) => i18n::t!("app.data.item_app", app = k.name, folder = n),
            None => n.to_string(),
        },
    }
}

/// The bullet list and the offer for a plan.
pub fn describe(plan: &Plan) -> (String, Offer) {
    let mut lines = Vec::new();
    for i in &plan.items {
        let size = crate::util::bytes(i.bytes);
        let mut line = match i.kind {
            ItemKind::Folder => i18n::tn!("app.data.item_files", i.files as i64, item = item_label(&i.name), size = size),
            _ => i18n::t!("app.data.item_file", item = item_label(&i.name), size = size),
        };
        match &i.blocker {
            Some(Blocker::Exists) => line.push_str(&i18n::t!("app.data.exists")),
            Some(Blocker::InUse(l)) => line.push_str(&i18n::t!("app.data.busy", app = l.app.clone(), host = l.host.clone())),
            None => {}
        }
        lines.push(format!("• {line}")); // i18n-ok: bullet
    }
    let busy = plan.items.iter().any(|i| matches!(i.blocker, Some(Blocker::InUse(_))));
    let exists = plan.items.iter().any(|i| i.blocker == Some(Blocker::Exists));
    let offer = if busy {
        Offer::Busy
    } else if exists || plan.items.is_empty() {
        Offer::UseThere
    } else {
        Offer::MoveOrCopy
    };
    (lines.join("\n"), offer)
}

impl Launcher {
    pub fn choose_data_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some(i18n::t!("app.data.pick").into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.ask_data_root(p, cx)).ok();
            }
        })
        .detach();
    }

    pub fn ask_data_root(&mut self, to: PathBuf, cx: &mut Context<Self>) {
        let from = paths::data_root();
        if to == from {
            self.show_toast(i18n::t!("app.data.same"), cx);
            return;
        }
        let plan = relocate::plan(&from, &to);
        let (list, offer) = describe(&plan);
        let cloud = otherwise_link::cloud::detect(&to).map(|c| c.provider.name().to_string());
        self.modal = Some(Modal::DataRoot { from, to, list, offer, cloud });
        cx.notify();
    }

    pub fn relocate_data(&mut self, transfer: Transfer, cx: &mut Context<Self>) {
        let Some(Modal::DataRoot { to, .. }) = self.modal.take() else { return };
        self.show_toast(i18n::t!("app.data.moving"), cx);
        let task = cx.background_executor().spawn(async move { relocate::relocate(&paths::Home::system(), &to, transfer) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            this.update(cx, |this, cx| {
                this.hub.root = Root::system();
                this.hub.settings = otherwise_link::settings::HubSettings::load(&this.hub.root);
                this.reload_today(cx);
                this.reload_entities(cx);
                match res {
                    Ok(r) if !r.left_behind.is_empty() => {
                        let paths = r.left_behind.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
                        this.modal = Some(Modal::Info { title: i18n::t!("app.data.left_title"), body: i18n::t!("app.data.left_body", to = r.to.display(), paths = paths) });
                    }
                    Ok(r) => this.show_toast(i18n::t!("app.data.moved", path = r.to.display()), cx),
                    Err(relocate::Error::Blocked(p)) => {
                        // something opened in between: show the fresh plan again
                        let (list, offer) = describe(&p);
                        this.modal = Some(Modal::DataRoot { from: p.from, to: p.to, list, offer, cloud: None });
                    }
                    Err(e) => this.modal = Some(Modal::Info { title: i18n::t!("app.data.error_title"), body: i18n::t!("app.data.error", detail = e.to_string()) }),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn render_data_root_modal(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Modal::DataRoot { from, to, list, offer, cloud }) = self.modal.as_ref() else { return None };
        let (title, intro) = match offer {
            Offer::MoveOrCopy => (i18n::t!("app.data.confirm_title"), i18n::t!("app.data.confirm_body", path = to.display())),
            Offer::UseThere => (i18n::t!("app.data.exists_title"), i18n::t!("app.data.exists_body", path = to.display(), from = from.display())),
            Offer::Busy => (i18n::t!("app.data.busy_title"), i18n::t!("app.data.busy_body")),
        };
        let text = |s: String| div().text_size(px(13.)).line_height(px(19.)).text_color(t.ink_muted).child(s);
        let mut buttons = div().flex().justify_end().gap(px(8.)).mt(px(6.)).child(ui::outline_button(t, "data-cancel").child(i18n::t!("app.action.cancel")).on_click(cx.listener(|this, _, _, cx| this.close_modal(cx))));
        match offer {
            Offer::MoveOrCopy => {
                buttons = buttons
                    .child(ui::outline_button(t, "data-copy").child(i18n::t!("app.data.copy")).on_click(cx.listener(|this, _, _, cx| this.relocate_data(Transfer::Copy, cx))))
                    .child(ui::filled_button(t, "data-move", t.accent).child(i18n::t!("app.data.move")).on_click(cx.listener(|this, _, _, cx| this.relocate_data(Transfer::Move, cx))));
            }
            Offer::UseThere => {
                buttons = buttons.child(ui::filled_button(t, "data-use", t.accent).child(i18n::t!("app.data.use_there")).on_click(cx.listener(|this, _, _, cx| this.relocate_data(Transfer::Leave, cx))));
            }
            Offer::Busy => {}
        }
        Some(
            self.panel(t, 520., cx)
                .child(div().pr(px(28.)).text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(text(intro))
                .children((!list.is_empty()).then(|| div().p(px(10.)).rounded(px(8.)).bg(t.canvas).border_1().border_color(t.border).text_size(px(12.)).line_height(px(18.)).child(list.clone())))
                .children(cloud.as_ref().map(|c| text(i18n::t!("app.data.cloud_note", provider = c.clone()))))
                .child(buttons)
                .into_any_element(),
        )
    }

    pub fn render_data_root_settings(&self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let root = self.hub.root.dir().to_path_buf();
        let reg = storage::status(&self.hub.root.entities_db());
        let where_ = match &reg.cloud {
            Some(c) => i18n::t!("app.data.in_cloud", provider = c.provider.name()),
            None => i18n::t!("app.data.local"),
        };
        let mut warnings: Vec<String> = Vec::new();
        if let Holder::Elsewhere(i) = &reg.lock {
            warnings.push(i18n::t!("app.data.registry_elsewhere", host = i.host.clone()));
        }
        if !reg.conflicts.is_empty() {
            warnings.push(i18n::tn!("app.data.conflicts", reg.conflicts.len() as i64));
        }
        let os = self.target.os;
        let reveal_root = root.clone();
        let path_box = div().flex_1().min_w_0().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(t.canvas).border_1().border_color(t.border).text_size(px(11.)).font_family("Menlo").child(root.display().to_string()); // i18n-ok: font name
        let controls = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().flex().items_center().gap(px(8.)).child(path_box))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ui::outline_button(t, "data-change").child(i18n::t!("app.data.change")).on_click(cx.listener(|this, _, window, cx| this.choose_data_root(window, cx))))
                    .child(ui::outline_button(t, "data-reveal").child(crate::cards::reveal_label(os)).on_click(move |_, _, _| {
                        let _ = std::fs::create_dir_all(&reveal_root);
                        let _ = process::reveal(os, &reveal_root);
                    }))
                    .child(div().text_size(px(12.)).text_color(t.ink_muted).child(where_)),
            )
            .children(warnings.into_iter().map(|w| div().text_size(px(12.)).line_height(px(17.)).text_color(t.danger).child(w)));
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .py(px(10.))
            .border_t_1()
            .border_color(t.border)
            .child(ui::section_title(t, i18n::t!("app.data.title")))
            .child(controls)
            .child(div().text_size(px(11.)).line_height(px(16.)).text_color(t.ink_muted).child(i18n::t!("app.data.hint")))
    }
}

#[cfg(test)]
mod tests {
    use otherwise_link::relocate::{Blocker, ItemKind, Plan, PlanItem};
    use std::path::PathBuf;

    fn item(name: &str, blocker: Option<Blocker>) -> PlanItem {
        PlanItem { name: name.into(), kind: if name.contains('.') { ItemKind::File } else { ItemKind::Folder }, bytes: 2048, files: 3, blocker }
    }

    #[test]
    fn offers_follow_the_plan() {
        let p = |items| Plan { from: PathBuf::from("/a"), to: PathBuf::from("/b"), items };
        let (list, offer) = super::describe(&p(vec![item("hub.json", None), item("vigia", None)]));
        assert_eq!(offer, super::Offer::MoveOrCopy);
        assert_eq!(list.lines().count(), 2);
        assert!(list.contains("vigia"));
        assert_eq!(super::describe(&p(vec![item("hub.json", Some(Blocker::Exists))])).1, super::Offer::UseThere);
        assert_eq!(super::describe(&p(vec![])).1, super::Offer::UseThere);
        let busy = Blocker::InUse(otherwise_link::lock::LockInfo { app: "vigia".into(), host: "mac".into(), ..Default::default() });
        assert_eq!(super::describe(&p(vec![item("hub.json", Some(Blocker::Exists)), item("vigia", Some(busy))])).1, super::Offer::Busy);
    }
}

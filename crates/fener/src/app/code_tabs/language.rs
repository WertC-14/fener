//! Language servers for the code tabs (fm-research ADR 0016): one per (command, project root),
//! told about each open file and its changes; their answers go to the tabs they are for.

use super::{App, CodeTab};
use crate::event::AppEvent;

/// A language server and what it is for.
pub(super) struct Server {
    pub(super) id: u64,
    pub(super) command: Vec<String>,
    pub(super) root: std::path::PathBuf,
    pub(super) client: fener_core::lsp::Client,
    /// Columns in its units once it answered `initialize`.
    pub(super) encoding: Option<fener_core::lsp::Encoding>,
}

impl App {
    /// Starts (or reuses) the language server for code tab `i` and opens its document there.
    pub(super) fn lsp_attach(&mut self, i: usize) {
        use fener_core::lsp;
        let Some(path) = self.code.tabs[i].editor.doc.path() else {
            return;
        };
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        // Remembered even when no server fits, so it is not tried again on every key.
        self.code.tabs[i].lsp_path = Some(path.clone());
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let command = match self.code.config.lsp.get(&ext) {
            Some(c) if c.trim().is_empty() => return,
            Some(c) => c.split_whitespace().map(String::from).collect(),
            None => match lsp::server_for(&path) {
                Some(c) => c,
                None => return,
            },
        };
        let root = lsp::root_for(&path);
        let id = match self
            .code
            .servers
            .iter()
            .find(|s| s.command == command && s.root == root)
        {
            Some(s) => s.id,
            None => {
                if self.code.missing.contains(&command[0]) {
                    return;
                }
                self.code.next_id += 1;
                let id = self.code.next_id;
                let tx = self.tx.clone();
                let started = lsp::Client::start(&command, &root, move |event| {
                    let _ = tx.send(AppEvent::Lsp {
                        server: id,
                        event: Box::new(event),
                    });
                });
                match started {
                    Ok(client) => self.code.servers.push(Server {
                        id,
                        command: command.clone(),
                        root,
                        client,
                        encoding: None,
                    }),
                    Err(e) => {
                        self.code.tabs[i].editor.message = Some(format!(
                            "No language server: {} ({e}); see README, Language servers",
                            command[0]
                        ));
                        self.code.missing.insert(command[0].clone());
                        return;
                    }
                }
                id
            }
        };
        let tab = &mut self.code.tabs[i];
        let server = self
            .code
            .servers
            .iter()
            .find(|s| s.id == id)
            .expect("just found");
        let version = tab.editor.doc.version();
        server
            .client
            .open(&path, version, &tab.editor.doc.rope.to_string());
        tab.server = Some(id);
        tab.sent_version = version;
        tab.sent_modified = tab.editor.doc.is_modified();
        if let Some(encoding) = server.encoding {
            tab.editor.lsp.active = true;
            tab.editor.lsp.encoding = encoding;
        }
    }

    /// Tells code tab `i`'s server about a change (the whole text) or a save; a different file
    /// in the window (opened from the tree or a picker) is attached anew.
    pub(super) fn lsp_sync(&mut self, i: usize) {
        let current = self.code.tabs[i]
            .editor
            .doc
            .path()
            .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
        if current.is_some() && current != self.code.tabs[i].lsp_path {
            let tab = &mut self.code.tabs[i];
            if let (Some(id), Some(old)) = (tab.server.take(), tab.lsp_path.take())
                && let Some(server) = self.code.servers.iter().find(|s| s.id == id)
            {
                server.client.close(&old);
            }
            self.code.tabs[i].editor.lsp = Default::default();
            self.lsp_attach(i);
            return;
        }
        let tab = &mut self.code.tabs[i];
        let (Some(id), Some(path)) = (tab.server, tab.editor.doc.path()) else {
            return;
        };
        let Some(server) = self.code.servers.iter().find(|s| s.id == id) else {
            return;
        };
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let version = tab.editor.doc.version();
        if version != tab.sent_version {
            server
                .client
                .change(&path, version, &tab.editor.doc.rope.to_string());
            tab.sent_version = version;
        }
        let modified = tab.editor.doc.is_modified();
        if tab.sent_modified && !modified {
            server.client.save(&path);
        }
        tab.sent_modified = modified;
    }

    /// A language server's answer, to the tabs it is for.
    pub(in crate::app) fn on_lsp(&mut self, server: u64, event: fener_core::lsp::Event) {
        use fener_core::lsp::Event;
        self.dirty = true;
        let same = |tab: &CodeTab, path: &std::path::Path| {
            tab.editor
                .doc
                .path()
                .and_then(|p| std::path::absolute(p).ok())
                .is_some_and(|p| p == path)
        };
        match event {
            Event::Ready(encoding) => {
                if let Some(s) = self.code.servers.iter_mut().find(|s| s.id == server) {
                    s.encoding = Some(encoding);
                }
                for tab in self
                    .code
                    .tabs
                    .iter_mut()
                    .filter(|t| t.server == Some(server))
                {
                    tab.editor.lsp.active = true;
                    tab.editor.lsp.encoding = encoding;
                }
            }
            Event::Diagnostics { path, items } => {
                for tab in self.code.tabs.iter_mut() {
                    if same(tab, &path) {
                        tab.editor.lsp.diagnostics = items.clone();
                        if tab.shared
                            && let Some(other) = tab.other.as_deref_mut()
                        {
                            other.editor.lsp.diagnostics = items.clone();
                        }
                    }
                    // The window of a split without the focus, too.
                    if let Some(other) = tab.other.as_deref_mut()
                        && other.lsp_path.as_deref() == Some(path.as_path())
                    {
                        other.editor.lsp.diagnostics = items.clone();
                    }
                }
            }
            Event::Hover { path, text } => {
                if let Some(tab) = self.code.tabs.iter_mut().find(|t| same(t, &path)) {
                    tab.editor.show_hover(text);
                }
            }
            Event::Completion { path, items, .. } => {
                if let Some(tab) = self.code.tabs.iter_mut().find(|t| same(t, &path)) {
                    tab.editor.show_completion(items);
                }
            }
            Event::Definition { path, items } => {
                let Some(target) = items.first().cloned() else {
                    if let Some(tab) = self.code.tabs.iter_mut().find(|t| same(t, &path)) {
                        tab.editor.message = Some("No definition found".into());
                    }
                    return;
                };
                // Another file opens in its own tab (as a buffer would in LazyVim).
                if target.path != path {
                    self.open_code_tab(&target.path);
                }
                if let Some(tab) = self.code.tabs.iter_mut().find(|t| same(t, &target.path)) {
                    tab.editor.jump_to(target.pos);
                }
            }
            Event::References { path, items } => {
                let places = items
                    .into_iter()
                    .map(|loc| {
                        // The line's text from an open tab, else from the file.
                        let text = self
                            .code
                            .tabs
                            .iter()
                            .find(|t| same(t, &loc.path))
                            .map(|t| fener_core::text::line_text(&t.editor.doc.rope, loc.pos.line))
                            .or_else(|| {
                                std::fs::read_to_string(&loc.path)
                                    .ok()?
                                    .lines()
                                    .nth(loc.pos.line)
                                    .map(String::from)
                            })
                            .unwrap_or_default();
                        (loc.path, loc.pos.line, text)
                    })
                    .collect();
                if let Some(tab) = self.code.tabs.iter_mut().find(|t| same(t, &path)) {
                    tab.editor.show_references(places);
                }
            }
            Event::Notice { path, text } => {
                if let Some(tab) = self.code.tabs.iter_mut().find(|t| same(t, &path)) {
                    tab.editor.message = Some(text);
                }
            }
            Event::Failed(why) => {
                self.code.servers.retain(|s| s.id != server);
                for tab in self
                    .code
                    .tabs
                    .iter_mut()
                    .filter(|t| t.server == Some(server))
                {
                    tab.server = None;
                    tab.editor.lsp = Default::default();
                    tab.editor.message = Some(format!("Language server: {why}"));
                }
            }
        }
    }
}

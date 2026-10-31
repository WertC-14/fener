//! Split windows in a code tab (`Space |`, `Space -`, `Space v`, `Space a`): two windows, each
//! with a file, or one document shared by both when they show the same file.

use std::path::Path;

use fener_core::{Document, Editor, State};

use super::{App, CodeTab};

/// The window of a split that does not have the focus.
pub(super) struct Pane {
    pub(super) editor: Editor,
    pub(super) view: fener_widgets::View,
    pub(super) server: Option<u64>,
    pub(super) sent_version: u64,
    pub(super) sent_modified: bool,
    pub(super) lsp_path: Option<std::path::PathBuf>,
}

impl CodeTab {
    /// The other window gets the focus (the two trade places in the fields).
    pub(super) fn swap_panes(&mut self) {
        self.follow_changes();
        let Some(other) = self.other.as_deref_mut() else {
            return;
        };
        std::mem::swap(&mut self.editor, &mut other.editor);
        std::mem::swap(&mut self.view, &mut other.view);
        std::mem::swap(&mut self.server, &mut other.server);
        std::mem::swap(&mut self.sent_version, &mut other.sent_version);
        std::mem::swap(&mut self.sent_modified, &mut other.sent_modified);
        std::mem::swap(&mut self.lsp_path, &mut other.lsp_path);
        self.other_first = !self.other_first;
        if self.shared {
            // One document, held by the focused window; the language server's view of it and
            // the answers about it go with it.
            let other = self.other.as_deref_mut().expect("split");
            std::mem::swap(&mut self.editor.doc, &mut other.editor.doc);
            std::mem::swap(&mut self.editor.lsp, &mut other.editor.lsp);
            std::mem::swap(&mut self.server, &mut other.server);
            std::mem::swap(&mut self.sent_version, &mut other.sent_version);
            std::mem::swap(&mut self.sent_modified, &mut other.sent_modified);
            std::mem::swap(&mut self.lsp_path, &mut other.lsp_path);
            other.editor.lsp.diagnostics = self.editor.lsp.diagnostics.clone();
            self.editor.shared = true;
            other.editor.shared = false;
        }
    }

    /// The other window's cursor follows the changes made here to the document they share.
    pub(super) fn follow_changes(&mut self) {
        let Some(other) = self.other.as_deref_mut() else {
            return;
        };
        if !self.shared {
            return;
        }
        let changes = self.editor.doc.take_changes();
        // A line put in right at the cursor (`O` above it) pushes the cursor down with its line;
        // other text typed there leaves it before, as in Vim.
        let follow = |change: &fener_core::document::Edit, pos: usize| {
            if change.from == change.to && pos == change.from && change.text.ends_with('\n') {
                pos + change.text.chars().count()
            } else {
                change.map(pos)
            }
        };
        let o = &mut other.editor;
        for change in &changes {
            o.cursor = follow(change, o.cursor);
            o.anchor = follow(change, o.anchor);
        }
        let len = self.editor.doc.rope.len_chars();
        o.cursor = o.cursor.min(len);
        o.anchor = o.anchor.min(len);
    }

    /// Both windows show the other window's document from now on (the focused one opened the
    /// same file, or the window was just split): it may have unsaved changes, so it wins.
    pub(super) fn share_other_doc(&mut self) {
        let Some(other) = self.other.as_deref_mut() else {
            return;
        };
        let doc = std::mem::replace(&mut other.editor.doc, Document::new(""));
        let len = doc.rope.len_chars();
        let e = &mut self.editor;
        e.doc = doc;
        e.doc.track_changes(true);
        e.cursor = e.cursor.min(len);
        e.anchor = e.anchor.min(len);
        e.reader = other.editor.reader;
        e.reader_top = other.editor.reader_top;
        e.shared = true;
        e.lsp = std::mem::take(&mut other.editor.lsp);
        other.editor.lsp.diagnostics = e.lsp.diagnostics.clone();
        self.server = other.server.take();
        self.sent_version = other.sent_version;
        self.sent_modified = other.sent_modified;
        self.lsp_path = other.lsp_path.take();
        // Colors cached for the old text would be wrong for this one.
        let on_ready = self.view.on_ready.clone();
        self.view = fener_widgets::View::default();
        self.view.on_ready = on_ready;
        self.shared = true;
    }

    /// The focused window opened another file: the document they shared goes back to the other
    /// window, with the language server's view of it.
    pub(super) fn unshare(&mut self, doc: Document) {
        let other = self.other.as_deref_mut().expect("split");
        other.editor.doc = doc;
        other.editor.doc.track_changes(false);
        other.editor.lsp = std::mem::take(&mut self.editor.lsp);
        other.server = self.server.take();
        other.sent_version = self.sent_version;
        other.sent_modified = self.sent_modified;
        other.lsp_path = self.lsp_path.take();
        self.editor.shared = false;
        self.shared = false;
    }

    /// Unsaved changes that closing the whole tab would lose (either window).
    pub(super) fn is_modified(&self) -> bool {
        self.editor.doc.is_modified()
            || self
                .other
                .as_deref()
                .is_some_and(|o| !self.shared && o.editor.doc.is_modified())
    }
}

impl App {
    /// After a key in tab `i`'s focused window: the other window's cursor follows the shared
    /// text; the window that opened the other window's file shares its document; the one that
    /// opened another file gives the shared document back.
    pub(super) fn split_sync(&mut self, i: usize) {
        let tab = &mut self.code.tabs[i];
        let Some(other) = tab.other.as_deref() else {
            return;
        };
        if tab.shared {
            match tab.editor.left.take() {
                // `:e!` read the same file again: both windows show the new copy.
                Some(old) if old.path() == tab.editor.doc.path() => {
                    tab.editor.shared = true;
                    tab.editor.doc.track_changes(true);
                    let len = tab.editor.doc.rope.len_chars();
                    if let Some(other) = tab.other.as_deref_mut() {
                        other.editor.cursor = other.editor.cursor.min(len);
                        other.editor.anchor = other.editor.anchor.min(len);
                    }
                }
                Some(doc) => tab.unshare(doc),
                None => tab.follow_changes(),
            }
            return;
        }
        let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let mine = tab.editor.doc.path().map(absolute);
        if mine.is_some() && mine == other.editor.doc.path().map(absolute) {
            // This window's fresh copy from the disk goes; its server attachment too.
            if let (Some(id), Some(path)) = (tab.server.take(), tab.lsp_path.take())
                && let Some(server) = self.code.servers.iter().find(|s| s.id == id)
            {
                server.client.close(&path);
            }
            self.code.tabs[i].share_other_doc();
        }
    }

    /// `Space |` / `Space -`: a second window in tab `i` on the same file (as Vim's `:vsplit`),
    /// with Find Files open to show another one there (Esc keeps the same file).
    pub(super) fn split(&mut self, i: usize, vertical: bool) {
        let tab = &mut self.code.tabs[i];
        if tab.other.is_some() {
            tab.editor.message =
                Some("This tab is split already (Space q w closes a window)".into());
            return;
        }
        let mut editor = Editor::new(Document::new(""));
        editor.root = tab.editor.root.clone();
        editor.state = State::load();
        editor.apply_config(&self.code.config);
        editor.cursor = tab.editor.cursor;
        editor.top = tab.editor.top;
        let mut view = fener_widgets::View::default();
        view.on_ready = tab.view.on_ready.clone();
        tab.other = Some(Box::new(Pane {
            editor,
            view,
            server: None,
            sent_version: 0,
            sent_modified: false,
            lsp_path: None,
        }));
        tab.vertical = vertical;
        // The new window gets the focus; the old one is drawn first (left / top).
        tab.other_first = false;
        tab.swap_panes();
        if tab
            .other
            .as_deref()
            .is_some_and(|o| o.editor.doc.path().is_some())
        {
            tab.share_other_doc();
        }
        tab.editor.run_command(fener_core::Command::FindFiles);
        self.code_requests(i);
    }

    /// `Space q w` / `:q` in a split: this window closes, the other one stays.
    pub(super) fn close_pane(&mut self, i: usize, force: bool) {
        let tab = &mut self.code.tabs[i];
        if tab.other.is_none() {
            tab.editor.message = Some("Not split (Space | or Space - splits)".into());
            return;
        }
        if tab.editor.doc.is_modified() && !force && !tab.shared {
            tab.editor.message =
                Some("No write since last change (:w saves, :q! drops the changes)".into());
            return;
        }
        // A shared document stays open in the other window (swap_panes hands it over).
        if !tab.shared
            && let (Some(id), Some(path)) = (tab.server, tab.lsp_path.clone())
            && let Some(server) = self.code.servers.iter().find(|s| s.id == id)
        {
            server.client.close(&path);
        }
        let tab = &mut self.code.tabs[i];
        tab.swap_panes();
        tab.other = None;
        tab.other_first = false;
        tab.shared = false;
        tab.editor.shared = false;
        tab.editor.doc.track_changes(false);
    }
}

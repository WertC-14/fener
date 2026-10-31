//! The language server's side of the editor (ADR 0016): asking about the cursor's place, the
//! completion menu, hover text, definitions, references and walking the diagnostics.

use std::path::PathBuf;

use super::{CompletionMenu, Editor, Key, LspAsk, Mode, Request};
use crate::lsp;
use crate::picker::{Item, Kind, Pick, Picker};

impl Editor {
    /// `K`, `gd`, `gr`: asks the language server about the cursor's place.
    pub(super) fn ask_lsp(&mut self, make: fn(lsp::Pos) -> LspAsk) {
        if !self.lsp.active {
            self.message = Some("No language server for this file (see Space s k: LSP)".into());
            return;
        }
        let pos = lsp::from_char(&self.doc.rope, self.cursor, self.lsp.encoding);
        self.requests.push(Request::Lsp(make(pos)));
    }

    /// While typing a word (or after `.` / `::` / `->`): asks for completions.
    pub(super) fn ask_completion(&mut self) {
        let pos = lsp::from_char(&self.doc.rope, self.cursor, self.lsp.encoding);
        self.requests
            .push(Request::Lsp(LspAsk::Completion(pos, self.doc.version())));
        if let Some(menu) = &mut self.lsp.completion {
            Self::filter_menu(menu, &self.doc.rope, self.cursor);
        }
    }

    /// The server's completions; shown filtered by the word typed so far.
    pub fn show_completion(&mut self, items: Vec<lsp::Completion>) {
        if self.mode != Mode::Insert || items.is_empty() {
            self.lsp.completion = None;
            return;
        }
        let start = word_start(&self.doc.rope, self.cursor);
        let mut menu = CompletionMenu {
            items,
            shown: Vec::new(),
            selected: 0,
            start,
        };
        Self::filter_menu(&mut menu, &self.doc.rope, self.cursor);
        self.lsp.completion = (!menu.shown.is_empty()).then_some(menu);
    }

    fn filter_menu(menu: &mut CompletionMenu, rope: &ropey::Rope, cursor: usize) {
        menu.start = word_start(rope, cursor);
        let typed: String = rope.slice(menu.start..cursor).chars().collect();
        let mut scored: Vec<(i64, usize)> = menu
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((crate::fuzzy::score(&typed, &item.label)?.0, i)))
            .collect();
        scored.sort_by_key(|(s, _)| -s);
        menu.shown = scored.into_iter().take(50).map(|(_, i)| i).collect();
        menu.selected = menu.selected.min(menu.shown.len().saturating_sub(1));
    }

    /// A key while the completion menu is open; `false` lets Insert mode have it.
    pub(super) fn completion_key(&mut self, key: Key) -> bool {
        let Some(menu) = &mut self.lsp.completion else {
            return false;
        };
        let len = menu.shown.len().max(1);
        match key {
            Key::Down | Key::Ctrl('n' | 'j') => menu.selected = (menu.selected + 1) % len,
            Key::Up | Key::Ctrl('p' | 'k') => menu.selected = (menu.selected + len - 1) % len,
            Key::Tab | Key::Enter => {
                let menu = self.lsp.completion.take().expect("open");
                if let Some(&i) = menu.shown.get(menu.selected) {
                    let insert = menu.items[i].insert.clone();
                    self.doc.edit(menu.start, self.cursor, &insert);
                    self.cursor = menu.start + insert.chars().count();
                }
            }
            Key::Esc => {
                self.lsp.completion = None;
                return false;
            }
            Key::Backspace => {
                self.insert_key(key);
                if self.cursor <= menu_start(&self.lsp.completion) {
                    self.lsp.completion = None;
                } else {
                    self.ask_completion();
                }
            }
            _ => return false,
        }
        true
    }

    /// `K`'s answer.
    pub fn show_hover(&mut self, text: String) {
        if text.trim().is_empty() {
            self.message = Some("No information here".into());
        } else {
            self.lsp.hover = Some(text);
        }
    }

    /// Moves to a place the server named in this document (`gd` in the same file).
    pub fn jump_to(&mut self, pos: lsp::Pos) {
        self.cursor = lsp::to_char(&self.doc.rope, pos, self.lsp.encoding);
        if self.mode != Mode::Normal {
            self.mode = Mode::Normal;
        }
        self.clamp();
        let line = self.line();
        if line < self.top || line >= self.top + self.view_height {
            self.top = line.saturating_sub(self.view_height / 2);
        }
    }

    /// `gr`'s answer: a picker of the places (file, line, text of that line).
    pub fn show_references(&mut self, places: Vec<(PathBuf, usize, String)>) {
        if places.is_empty() {
            self.message = Some("No references found".into());
            return;
        }
        let items = places
            .into_iter()
            .map(|(path, line, text)| Item {
                text: text.trim().to_string(),
                detail: format!("{}:{}", self.display_path(&path), line + 1),
                pick: Pick::File {
                    path,
                    line: Some(line),
                },
            })
            .collect();
        self.picker = Some(Picker::new(Kind::Locations, "References", items));
    }

    /// `]d` / `[d`: the next / previous diagnostic (wrapping), its message shown.
    pub(super) fn next_diagnostic(&mut self, forward: bool) {
        let rope = &self.doc.rope;
        let mut starts: Vec<(usize, &str)> = self
            .lsp
            .diagnostics
            .iter()
            .map(|d| {
                (
                    lsp::to_char(rope, d.start, self.lsp.encoding),
                    d.message.as_str(),
                )
            })
            .collect();
        starts.sort_by_key(|(at, _)| *at);
        let next = if forward {
            starts
                .iter()
                .find(|(at, _)| *at > self.cursor)
                .or(starts.first())
        } else {
            starts
                .iter()
                .rev()
                .find(|(at, _)| *at < self.cursor)
                .or(starts.last())
        };
        match next {
            Some(&(at, message)) => {
                let message = message.lines().next().unwrap_or("").to_string();
                self.cursor = at;
                self.clamp();
                self.message = Some(message);
            }
            None => self.message = Some("No diagnostics".into()),
        }
    }
}

/// Where the word before `cursor` starts.
fn word_start(rope: &ropey::Rope, cursor: usize) -> usize {
    let mut start = cursor;
    while start > 0
        && rope
            .get_char(start - 1)
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        start -= 1;
    }
    start
}

fn menu_start(menu: &Option<CompletionMenu>) -> usize {
    menu.as_ref().map_or(0, |m| m.start)
}

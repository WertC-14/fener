//! What sits beside or over the text: the project tree, the Markdown reader, the start screen
//! and the pickers.

use super::{Editor, Key, Request};
use crate::command::DASHBOARD;
use crate::picker::{Item, Kind, Pick};
use crate::text;
use crate::tree::{self, Tree};

impl Editor {
    /// Opens the folder tree at the open file's project (or `root`), with the file revealed.
    pub fn show_tree(&mut self, focus: bool) {
        let file = self
            .doc
            .path()
            .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
        let root = match &file {
            Some(f) => tree::project_root(f),
            None => std::path::absolute(&self.root).unwrap_or_else(|_| self.root.clone()),
        };
        let mut t = Tree::new(root);
        if let Some(f) = &file {
            t.reveal(f);
        }
        self.tree = Some(t);
        self.tree_focus = focus;
        self.dashboard = None;
    }

    /// A key while the tree has the focus.
    pub(super) fn tree_key(&mut self, key: Key) {
        let Some(t) = &mut self.tree else {
            return;
        };
        match key {
            Key::Char('j') | Key::Down => t.move_by(1),
            Key::Char('k') | Key::Up => t.move_by(-1),
            Key::Char('g') | Key::Home => t.selected = 0,
            Key::Char('G') | Key::End => t.move_by(isize::MAX / 2),
            Key::Ctrl('d') => t.move_by(self.view_height as isize / 2),
            Key::Ctrl('u') => t.move_by(-(self.view_height as isize / 2)),
            Key::Enter | Key::Char('l' | 'o') | Key::Right => {
                if let Some(path) = t.activate() {
                    self.open(&path, None, false);
                    if self.doc.path() == Some(path.as_path()) {
                        self.tree_focus = false;
                    }
                }
            }
            Key::Char('h') | Key::Left => t.collapse(),
            Key::Backspace | Key::Char('-') => t.root_up(),
            Key::Char('R') => t.rebuild(),
            Key::Char('q') => {
                self.tree = None;
                self.tree_focus = false;
            }
            Key::Esc => self.tree_focus = false,
            _ => {}
        }
    }

    /// A key in the Markdown reader: scrolling, or `i` / `Esc` to edit the source.
    pub(super) fn reader_key(&mut self, key: Key) {
        let last = text::line_count(&self.doc.rope).saturating_sub(1);
        let half = (self.view_height / 2).max(1);
        match key {
            Key::Char('j') | Key::Down | Key::Ctrl('e') | Key::Enter => self.reader_top += 1,
            Key::Char('k') | Key::Up | Key::Ctrl('y') => {
                self.reader_top = self.reader_top.saturating_sub(1);
            }
            Key::Ctrl('d') => self.reader_top += half,
            Key::Ctrl('u') => self.reader_top = self.reader_top.saturating_sub(half),
            Key::Char('g') | Key::Home => self.reader_top = 0,
            Key::Char('G') | Key::End => self.reader_top = last,
            Key::Char('i' | 'e') | Key::Esc => {
                self.reader = false;
                self.leave_reader();
            }
            _ => {
                self.message =
                    Some("Reader: j k scroll, i / Esc / Ctrl+E to edit, Space r back".into())
            }
        }
        self.reader_top = self.reader_top.min(last);
    }

    /// Back to the source at the place the reader was showing.
    pub(super) fn leave_reader(&mut self) {
        let line = self
            .reader_top
            .min(text::line_count(&self.doc.rope).saturating_sub(1));
        self.top = line;
        self.cursor = text::first_non_blank(&self.doc.rope, line);
    }

    /// The list the worker made for the picker of `kind`, for the query `query` (grep).
    /// Ignored when that picker is no longer open or the query changed meanwhile.
    pub fn receive(&mut self, kind: Kind, query: &str, items: Vec<Item>) {
        if let Some(p) = &mut self.picker
            && p.kind == kind
            && (kind != Kind::Grep || p.query == query)
        {
            p.set_items(items);
        }
    }

    /// A key on the start screen; `false` when it is not the menu's (Space, `:` go on as usual).
    pub(super) fn dashboard_key(&mut self, key: Key) -> bool {
        let selected = self.dashboard.unwrap_or(0);
        match key {
            Key::Char('j') | Key::Down | Key::Tab => {
                self.dashboard = Some((selected + 1) % DASHBOARD.len());
            }
            Key::Char('k') | Key::Up => {
                self.dashboard = Some((selected + DASHBOARD.len() - 1) % DASHBOARD.len());
            }
            Key::Enter => self.run_command(DASHBOARD[selected].2),
            Key::Esc => self.dashboard = None,
            Key::Char(c) => match DASHBOARD.iter().position(|(k, _, _)| *k == c) {
                Some(i) => {
                    self.dashboard = Some(i);
                    self.run_command(DASHBOARD[i].2);
                }
                None => return false,
            },
            _ => {}
        }
        true
    }

    pub(super) fn picker_key(&mut self, key: Key) {
        let Some(p) = &mut self.picker else {
            return;
        };
        let mut query_changed = false;
        match key {
            Key::Esc | Key::Ctrl('c') => {
                if p.kind == Kind::Themes {
                    self.theme = p.theme_before;
                }
                self.picker = None;
                return;
            }
            Key::Enter => {
                let query = p.query.clone();
                let lines = p.kind == Kind::Lines;
                let pick = p
                    .current()
                    .map(|item| (item.pick.clone(), item.detail.clone(), item.text.clone()));
                self.picker = None;
                // Find in File: the query stays lit, n / N go on from there.
                if lines && !query.is_empty() {
                    self.last_search = Some(query);
                    self.highlight_search = true;
                    self.search_forward = true;
                }
                match pick {
                    Some((Pick::File { path, line }, ..)) => {
                        let same = self.doc.path() == Some(path.as_path());
                        self.open(&path, line, false);
                        // A jump inside a Markdown file in the reader scrolls the reader.
                        if same && self.reader {
                            self.reader_top = line.unwrap_or(0);
                        }
                    }
                    Some((Pick::Line(line), ..)) => {
                        self.cursor = text::first_non_blank(&self.doc.rope, line);
                        if self.reader {
                            self.reader_top = line;
                        }
                    }
                    Some((Pick::Command(command), ..)) => self.run_command(command),
                    Some((Pick::Theme(i), ..)) => {
                        self.theme = i;
                        self.state.theme = self.themes.get(i).map(|n| (*n).to_string());
                        self.requests.push(Request::SaveState);
                    }
                    Some((Pick::Info, keys, text)) => {
                        self.message = Some(format!("{keys}  {text}"))
                    }
                    None => {}
                }
                return;
            }
            Key::Up | Key::Ctrl('p' | 'k') => p.move_by(-1),
            Key::Down | Key::Tab | Key::Ctrl('n' | 'j') => p.move_by(1),
            Key::Backspace => query_changed = p.query.pop().is_some(),
            Key::Ctrl('u') => {
                query_changed = !p.query.is_empty();
                p.query.clear();
            }
            Key::Ctrl('w') => {
                let keep = p.query.trim_end().rfind(' ').map_or(0, |i| i + 1);
                query_changed = keep < p.query.len();
                p.query.truncate(keep);
            }
            Key::Char(c) => {
                p.query.push(c);
                query_changed = true;
            }
            _ => {}
        }
        if query_changed {
            p.selected = 0;
            if p.kind == Kind::Grep {
                p.loading = !p.query.is_empty();
                if p.query.is_empty() {
                    p.set_items(Vec::new());
                } else {
                    self.requests.push(Request::Grep(p.query.clone()));
                }
            } else {
                p.refilter();
            }
        }
        // Themes are tried on while moving through the list.
        if let Some(Item {
            pick: Pick::Theme(i),
            ..
        }) = p.current()
        {
            self.theme = *i;
        }
    }
}

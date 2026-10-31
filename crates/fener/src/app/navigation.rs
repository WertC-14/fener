//! Moving around: the selection, pages and wheel steps per view, opening entries, back /
//! forward / up, and typing a path (Ctrl+L).

use std::path::PathBuf;

use liman_core::Entry;
use liman_core::i18n::trf;
use liman_widgets::breadcrumb::{self, Segment};
use liman_widgets::grid::{self, GridView};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::open::{self, OpenPlan};

use super::{App, Listing, View, WHEEL_STEP, code_tabs, is_typing};

impl App {
    pub fn path_segments(&self) -> Vec<Segment> {
        breadcrumb::segments(&self.tab.cwd, &self.places.home)
    }

    /// Entries that fit on one screen; at least one.
    pub(super) fn page(&self) -> isize {
        let entries = match self.drawn_view.list_mode() {
            Some(mode) => usize::from(
                self.list_area
                    .height
                    .saturating_sub(liman_widgets::file_list::HEADER_HEIGHT)
                    / mode.row_height(),
            ),
            None => {
                let (_, tile_h) = grid::tile_size(self.drawn_grid_level);
                usize::from(self.list_area.height / tile_h) * self.grid_columns()
            }
        };
        isize::try_from(entries).unwrap_or(1).max(1)
    }

    /// Tiles per row in the grid as drawn last frame.
    fn grid_columns(&self) -> usize {
        GridView::columns(self.list_area.width, self.drawn_grid_level)
    }

    /// One wheel notch: three lines in the lists, one tile row in the grid.
    pub(super) fn wheel_step(&self) -> isize {
        match self.drawn_view {
            View::Grid => self.vertical_step(),
            _ => WHEEL_STEP,
        }
    }

    /// Rows move by one entry in the lists, by a whole tile row in the grid.
    pub(super) fn vertical_step(&self) -> isize {
        match self.drawn_view {
            View::Grid => isize::try_from(self.grid_columns()).unwrap_or(1),
            _ => 1,
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        if self.tab.visible.is_empty() {
            return;
        }
        let current = self.tab.table.selected().unwrap_or(0);
        self.select_row(current.saturating_add_signed(delta));
    }

    /// Shift+movement: the anchor (where the range started: the last plain click or the selection
    /// when Shift was first pressed) stays, the cursor moves, and exactly the range is marked.
    pub(super) fn extend_selection(&mut self, code: KeyCode) {
        let Some(current) = self.tab.table.selected() else {
            return;
        };
        let anchor = *self.click_anchor.get_or_insert(current);
        let grid = self.drawn_view == View::Grid;
        match code {
            KeyCode::Up => self.move_selection(-self.vertical_step()),
            KeyCode::Down => self.move_selection(self.vertical_step()),
            KeyCode::Left if grid => self.move_selection(-1),
            KeyCode::Right if grid => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-self.page()),
            KeyCode::PageDown => self.move_selection(self.page()),
            KeyCode::Home => self.select_row(0),
            KeyCode::End => self.select_row(usize::MAX),
            _ => return,
        }
        let to = self.tab.table.selected().unwrap_or(anchor);
        let (from, to) = (anchor.min(to), anchor.max(to));
        self.tab.marked = self
            .visible_entries()
            .skip(from)
            .take(to - from + 1)
            .map(|e| e.path.clone())
            .collect();
    }

    /// Selects `row`, clamped to the last row.
    pub(super) fn select_row(&mut self, row: usize) {
        if self.tab.visible.is_empty() {
            self.tab.table.select(None);
        } else {
            self.tab
                .table
                .select(Some(row.min(self.tab.visible.len() - 1)));
        }
    }

    /// Enter / double click: open a folder here, a file in its app (or editor over SSH).
    pub(super) fn activate_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if entry.is_dir {
            let path = entry.path.clone();
            self.load(path);
            return;
        }
        let entry = entry.clone();
        self.open_file(&entry);
    }

    /// Opens a file: text in a code tab (ADR 0012), the rest in its app (or `$EDITOR` over SSH).
    pub(super) fn open_file(&mut self, entry: &Entry) {
        if code_tabs::opens_in_code_tab(entry) {
            return self.open_code_tab(&entry.path);
        }
        match open::plan(entry, &open::Env::current()) {
            OpenPlan::Desktop(path) => {
                let name = entry.name.clone();
                self.message = Some(match open::open_desktop(&path) {
                    Ok(()) => trf("Opening “{}”…", &[&name]),
                    Err(e) => trf("Cannot open “{}”: {}", &[&name, &e]),
                });
            }
            OpenPlan::Editor { program, path } => self.external = Some((program, path)),
            OpenPlan::Unavailable(why) => self.message = Some(why),
        }
    }

    pub(super) fn go_back(&mut self) {
        if let Some(path) = self.tab.back_stack.pop() {
            self.tab.forward_stack.push(self.tab.cwd.clone());
            self.moving_in_history = true;
            self.load(path);
        }
    }

    pub(super) fn go_forward(&mut self) {
        if let Some(path) = self.tab.forward_stack.pop() {
            self.tab.back_stack.push(self.tab.cwd.clone());
            self.moving_in_history = true;
            self.load(path);
        }
    }

    pub(super) fn go_up(&mut self) {
        if self.tab.results.is_some() {
            return self.load(self.tab.cwd.clone()); // back from results to the folder
        }
        let Some(parent) = self.tab.cwd.parent().map(PathBuf::from) else {
            return;
        };
        self.tab.select_after_load = self
            .tab
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.load(parent);
    }

    /// The display order: every entry of the listing, as sorted.
    pub(super) fn refresh_visible(&mut self) {
        let len = match &self.tab.listing {
            Listing::Ready(entries) => entries.len(),
            _ => 0,
        };
        self.tab.visible = (0..len).collect();
    }

    /// Ctrl+L: the path bar becomes a text field with the current folder.
    pub(super) fn begin_path_input(&mut self) {
        // Under the home folder the path starts with `~/`, like in a shell.
        let mut text = match self.tab.cwd.strip_prefix(&self.places.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => self.tab.cwd.display().to_string(),
        };
        if !text.ends_with('/') {
            text.push('/');
        }
        self.path_input = Some(text);
    }

    pub(super) fn on_path_key(&mut self, key: KeyEvent) {
        use liman_core::path_input;
        let Some(text) = &mut self.path_input else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.path_input = None,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Tab => {
                if let Some(done) = path_input::complete(text, &self.tab.cwd, &self.places.home) {
                    *text = done;
                }
            }
            KeyCode::Char(c) if is_typing(key) => text.push(c),
            KeyCode::Enter => {
                let typed = self.path_input.take().unwrap_or_default();
                let path = path_input::resolve(&typed, &self.tab.cwd, &self.places.home);
                // A file opens its folder with the file selected.
                match (path.is_dir(), path.parent(), path.file_name()) {
                    (true, _, _) => self.load(path),
                    (false, Some(dir), Some(name)) if path.exists() => {
                        self.tab.select_after_load = Some(name.to_string_lossy().into_owned());
                        self.load(dir.to_path_buf());
                    }
                    _ => self.message = Some(trf("No such file or folder: {}", &[&typed])),
                }
            }
            _ => {}
        }
    }
}

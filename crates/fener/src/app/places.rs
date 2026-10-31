//! The Places panel on the left: quick access, bookmarks and the folder tree.

use std::path::PathBuf;

use liman_core::i18n::{tr, trf};
use liman_widgets::Sidebar;
use liman_widgets::sidebar::Item as SideItem;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::worker;

use super::{App, Focus, Results};

impl App {
    /// Keys while Places has focus: ↑↓ choose (titles are skipped), Enter opens and moves to the
    /// files. In the folder tree → opens a branch (or steps into it), ← closes it (or goes to the
    /// parent). Returns false for keys Places does not use (they work as usual).
    pub(super) fn on_places_key(&mut self, key: KeyEvent) -> bool {
        // The tree may have shrunk since the cursor was placed (a move closes branches).
        let row = self.sidebar_selected.min(self.sidebar_len() - 1);
        self.sidebar_selected = row;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.sidebar_selected = self.sidebar_step(row, -1),
            KeyCode::Down | KeyCode::Char('j') => self.sidebar_selected = self.sidebar_step(row, 1),
            KeyCode::Home | KeyCode::Char('g') => self.sidebar_selected = self.sidebar_step(0, 1),
            KeyCode::End | KeyCode::Char('G') => {
                self.sidebar_selected = self.sidebar_step(self.sidebar_len(), -1);
            }
            KeyCode::Delete => {
                if let SideItem::Place(i) = self.sidebar_item(row)
                    && self.sidebar[i].kind == liman_core::SpecialDir::Bookmark
                {
                    let path = self.sidebar[i].path.clone();
                    self.toggle_bookmark_for(path);
                }
            }
            KeyCode::Right | KeyCode::Char('l')
                if matches!(self.sidebar_item(row), SideItem::Dir(_)) =>
            {
                let SideItem::Dir(d) = self.sidebar_item(row) else {
                    return true;
                };
                let dir = self.tree.rows()[d].clone();
                if dir.expanded {
                    self.sidebar_selected = self.sidebar_step(row, 1);
                } else {
                    self.expand_tree(&dir.path);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let SideItem::Dir(d) = self.sidebar_item(row) {
                    let dir = self.tree.rows()[d].clone();
                    if dir.expanded {
                        self.tree.collapse(&dir.path);
                    } else if let Some(parent) = self.tree.rows()[..d]
                        .iter()
                        .rposition(|r| r.depth + 1 == dir.depth)
                    {
                        self.sidebar_selected = Sidebar::dir_row(self.sidebar.len(), parent);
                    }
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                match self.sidebar_item(row) {
                    SideItem::Place(i) => self.open_place(i),
                    SideItem::Dir(d) => self.load(self.tree.rows()[d].path.clone()),
                    SideItem::Label => return true,
                }
                self.tab.focus = Focus::Files;
            }
            _ => return false,
        }
        true
    }

    pub(super) fn sidebar_len(&self) -> usize {
        Sidebar::len(self.sidebar.len(), self.tree.rows().len())
    }

    pub(super) fn sidebar_item(&self, row: usize) -> SideItem {
        Sidebar::item(self.sidebar.len(), self.tree.rows().len(), row).unwrap_or(SideItem::Label)
    }

    pub(super) fn sidebar_row_at(&self, column: u16, row: u16) -> Option<usize> {
        Sidebar::row_at(
            self.sidebar_area,
            self.sidebar_offset,
            self.sidebar_len(),
            column,
            row,
        )
    }

    /// The next selectable row from `from` in direction `step` (titles and the rule are skipped);
    /// `from` itself when there is none.
    fn sidebar_step(&self, from: usize, step: isize) -> usize {
        let mut row = from;
        loop {
            let Some(next) = row
                .checked_add_signed(step)
                .filter(|&r| r < self.sidebar_len())
            else {
                return from;
            };
            row = next;
            if self.sidebar_item(row) != SideItem::Label {
                return row;
            }
        }
    }

    /// A click in Places: a place opens; in the tree, the ▸/▾ opens or closes the branch and the
    /// name opens the folder.
    pub(super) fn on_sidebar_click(&mut self, row: usize, column: u16) {
        match self.sidebar_item(row) {
            SideItem::Place(i) => self.open_place(i),
            SideItem::Dir(d) => {
                self.sidebar_selected = row;

                let dir = self.tree.rows()[d].clone();
                if column == Sidebar::arrow_column(self.sidebar_area, dir.depth) {
                    if dir.expanded {
                        self.tree.collapse(&dir.path);
                    } else {
                        self.expand_tree(&dir.path);
                    }
                } else {
                    self.load(dir.path);
                }
            }
            SideItem::Label => {}
        }
    }

    /// Opens a branch of the tree; its subfolders are read on a worker the first time.
    fn expand_tree(&mut self, dir: &std::path::Path) {
        if self.tree.expand(dir) {
            worker::spawn_tree_children(
                self.tx.clone(),
                dir.to_path_buf(),
                self.options.show_hidden,
            );
        }
    }

    /// After moving to `cwd`: its subfolders are known from the listing, and the tree opens the way
    /// down to it.
    pub(super) fn update_tree(&mut self, mut dirs: Vec<(String, PathBuf)>) {
        dirs.sort_by(|a, b| liman_core::sort::natural_cmp(&a.0, &b.0));
        let dirs = dirs.into_iter().map(|(_, path)| path).collect();
        self.tree.set_children(self.tab.cwd.clone(), dirs);
        for dir in self.tree.focus(&self.tab.cwd) {
            worker::spawn_tree_children(self.tx.clone(), dir, self.options.show_hidden);
        }
    }

    /// Opens sidebar place `i`: a folder, the Recent list, or a pinned file (in its app).
    fn open_place(&mut self, i: usize) {
        self.sidebar_selected = i + 1; // the row under the QUICK ACCESS title
        let place = self.sidebar[i].clone();
        if place.kind == liman_core::SpecialDir::Recent {
            self.show_recent();
        } else if place.kind == liman_core::SpecialDir::Bookmark && place.path.is_file() {
            let entry = liman_core::listing::entry_for(place.path, place.name);
            self.open_file(&entry);
        } else {
            self.load(place.path);
        }
    }

    /// Recently used files (GTK's list) as a results view.
    pub(super) fn show_recent(&mut self) {
        let home = self.places.home.clone();
        let paths = liman_core::recent::recent_files(&home);
        if paths.is_empty() {
            self.message = Some(tr("No recent files").into());
            return;
        }
        let results = Results {
            command: tr("Recent files").into(),
            count: paths.len(),
            running: false,
        };
        self.load_results(results, home, paths);
    }

    /// Ctrl+D: bookmark the selected folder (or this folder), or remove it if already there.
    /// Ctrl+D: pins the selected file or folder to Quick Access (this folder when nothing is
    /// selected), or unpins it.
    pub(super) fn toggle_bookmark(&mut self) {
        let path = self
            .selected_entry()
            .map_or_else(|| self.tab.cwd.clone(), |e| e.path.clone());
        self.toggle_bookmark_for(path);
    }

    pub(super) fn toggle_bookmark_for(&mut self, folder: PathBuf) {
        let name = folder.file_name().map_or_else(
            || folder.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        if let Some(i) = self.bookmarks.iter().position(|b| *b == folder) {
            self.bookmarks.remove(i);
            self.message = Some(trf("Removed bookmark “{}”", &[&name]));
        } else {
            self.bookmarks.push(folder);
            self.message = Some(trf("Bookmarked “{}”", &[&name]));
        }
        self.sidebar = self.places.sidebar_with(&self.bookmarks);
        self.sidebar_selected = self.sidebar_selected.min(self.sidebar_len() - 1);
        if !cfg!(test) {
            let file = liman_core::config::bookmarks_path(&self.places.home);
            if let Err(e) = liman_core::config::save_bookmarks(&file, &self.bookmarks) {
                self.message = Some(trf("Bookmark not saved: {}", &[&e]));
            }
        }
    }
}

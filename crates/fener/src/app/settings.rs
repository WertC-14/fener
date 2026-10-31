//! Preferences changed from the app: theme, hidden files, sort order, view size and the
//! terminal panel's height. Saved to liman's settings file, which fener shares.

use liman_core::i18n::{tr, trf};
use liman_widgets::grid;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Listing, View};

impl App {
    /// `t`: the theme list. Moving through it applies each theme at once (live preview).
    pub(super) fn open_theme_picker(&mut self) {
        let current = liman_widgets::theme::current_index();
        self.theme_picker = Some((current, current));
    }

    pub(super) fn on_picker_key(&mut self, key: KeyEvent) {
        let Some((selected, original)) = self.theme_picker else {
            return;
        };
        let last = liman_widgets::theme::THEMES.len() - 1;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.pick_theme(selected.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.pick_theme((selected + 1).min(last)),
            KeyCode::Enter => self.save_theme(),
            KeyCode::Esc | KeyCode::Char('q') => {
                liman_widgets::theme::set_index(original);
                self.theme_picker = None;
            }
            _ => {}
        }
    }

    pub(super) fn pick_theme(&mut self, index: usize) {
        if let Some((selected, _)) = &mut self.theme_picker {
            *selected = index;
            liman_widgets::theme::set_index(index);
        }
    }

    /// Keeps the previewed theme and writes it to the config file.
    pub(super) fn save_theme(&mut self) {
        self.theme_picker = None;
        let name = liman_widgets::theme::palette().name;
        self.save_setting("theme", name);
        self.message = Some(trf("Theme: {}", &[&name]));
    }

    /// Settings from the config file (theme is applied earlier, in `main`).
    pub fn apply_settings(&mut self, settings: &std::collections::BTreeMap<String, String>) {
        if !cfg!(test) {
            self.bookmarks = liman_core::config::load_bookmarks(
                &liman_core::config::bookmarks_path(&self.places.home),
            );
            self.sidebar = self.places.sidebar_with(&self.bookmarks);
        }
        if let Some(order) = settings
            .get("sort")
            .and_then(|s| liman_core::sort::SortOrder::from_config(s))
        {
            self.sort = order;
        }
        // `images = halfblocks` never asks the terminal for a graphics protocol (ADR 0009).
        self.preview.images_auto = settings.get("images").is_none_or(|v| v != "halfblocks");
        if settings.get("preview").is_some_and(|v| v == "true") {
            self.preview.shown = true;
        }
        if settings.get("hidden").is_some_and(|v| v == "true") && !self.options.show_hidden {
            self.options.show_hidden = true;
            self.load(self.tab.cwd.clone());
        }
    }

    pub(super) fn save_setting(&mut self, key: &str, value: &str) {
        if cfg!(test) {
            return; // never touch the real config from tests
        }
        let file = liman_core::config::path(&self.places.home);
        if let Err(e) = liman_core::config::set(&file, key, value) {
            self.message = Some(trf("Setting not saved: {}", &[&e]));
        }
    }

    /// Ctrl+H: show or hide dot files (reloads, keeps the selection).
    pub(super) fn toggle_hidden(&mut self) {
        self.options.show_hidden = !self.options.show_hidden;
        let value = if self.options.show_hidden {
            "true"
        } else {
            "false"
        };
        self.save_setting("hidden", value);
        self.message = Some(
            tr(if self.options.show_hidden {
                "Showing hidden files"
            } else {
                "Hiding hidden files"
            })
            .into(),
        );
        self.tab.select_after_load = self.selected_entry().map(|e| e.name.clone());
        if self.tab.results.is_none() {
            self.load(self.tab.cwd.clone());
        }
    }

    /// Re-sorts the listing in place (no reload) and keeps the selected entry selected.
    pub(super) fn set_sort(&mut self, order: liman_core::sort::SortOrder) {
        self.save_setting("sort", &order.to_config());
        self.set_sort_quietly(order);
        let template = if order.descending {
            "Sorted by {}, descending"
        } else {
            "Sorted by {}"
        };
        self.message = Some(trf(template, &[&tr(order.key.name())]));
    }

    /// Sorts again, keeping the selected entry selected (no message, not saved).
    pub(super) fn set_sort_quietly(&mut self, order: liman_core::sort::SortOrder) {
        self.sort = order;
        let keep = self.selected_entry().map(|e| e.path.clone());
        if let Listing::Ready(entries) = &mut self.tab.listing {
            liman_core::sort::sort_with(entries, order);
        }
        self.refresh_visible();
        let row = keep
            .and_then(|p| self.visible_entries().position(|e| e.path == p))
            .unwrap_or(0);
        self.select_row(row);
    }

    /// `v`: one key between the compact list (like cardea) and the large view last used.
    pub(super) fn toggle_compact(&mut self) {
        if self.view == View::Detailed {
            self.view = self.last_large_view;
        } else {
            self.last_large_view = self.view;
            self.view = View::Detailed;
        }
    }

    /// Ctrl+↑ / Ctrl+↓: grow or shrink the terminal panel by `rows`.
    pub(super) fn resize_panel(&mut self, rows: i16) {
        let current = self.term_height.unwrap_or(self.term_area.height);
        self.term_height = Some(current.saturating_add_signed(rows).max(4));
    }

    /// Zooms like a GUI file manager: Detailed → Normal → Grid, then larger and larger boxes.
    pub(super) fn zoom(&mut self, step: i8) {
        let current = self.drawn_grid_level;
        let largest = grid::BOX_SIZES.len() - 1;
        match (self.view, step.signum()) {
            (View::Detailed, 1) => self.view = View::Normal,
            (View::Normal, 1) => {
                self.view = View::Grid;
                self.grid_level = None; // fit to the window first
            }
            (View::Grid, 1) => self.grid_level = Some((current + 1).min(largest)),
            (View::Grid, -1) if current == 0 => {
                self.view = View::Normal;
                self.grid_level = None;
            }
            (View::Grid, -1) => self.grid_level = Some(current - 1),
            (View::Normal, -1) => self.view = View::Detailed,
            _ => {}
        }
    }
}

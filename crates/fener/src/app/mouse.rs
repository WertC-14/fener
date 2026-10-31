//! Mouse: clicks, double clicks, the wheel, dragging the panel border, and dragging files
//! onto folders (move, Ctrl copies).

use std::path::PathBuf;
use std::time::Instant;

use liman_core::i18n::{tr, trf};
use liman_widgets::grid::GridView;
use liman_widgets::sidebar::Item as SideItem;
use liman_widgets::{FileList, ListMode, breadcrumb};
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{App, DOUBLE_CLICK, Dialog, Drag, Focus, TermMode, View};

impl App {
    pub(super) fn on_mouse(&mut self, mouse: MouseEvent) {
        let ctrl = mouse.modifiers.contains(KeyModifiers::CONTROL);
        match mouse.kind {
            MouseEventKind::ScrollUp if ctrl => self.zoom(1),
            MouseEventKind::ScrollDown if ctrl => self.zoom(-1),
            MouseEventKind::ScrollDown
                if self.preview.area.contains((mouse.column, mouse.row).into()) =>
            {
                self.scroll_preview(3)
            }
            MouseEventKind::ScrollUp
                if self.preview.area.contains((mouse.column, mouse.row).into()) =>
            {
                self.scroll_preview(-3)
            }
            // The wheel over the tab row walks through the tabs.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if !self.tabs.chip_areas.is_empty() && self.tabs.chip_areas[0].y == mouse.row =>
            {
                let step = if mouse.kind == MouseEventKind::ScrollDown {
                    1
                } else {
                    -1
                };
                self.cycle_tab(step)
            }
            MouseEventKind::ScrollDown => self.move_selection(self.wheel_step()),
            MouseEventKind::ScrollUp => self.move_selection(-self.wheel_step()),
            MouseEventKind::Down(button) if self.tab_row_click(mouse.column, mouse.row, button) => {
            }
            MouseEventKind::Down(MouseButton::Left)
                if self.tab.term_mode == TermMode::Panel && mouse.row == self.term_area.y =>
            {
                self.dragging_panel = true; // grabbed the panel's top border
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging_panel => {
                self.term_height = Some(self.term_area.bottom().saturating_sub(mouse.row));
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging_panel => {
                self.dragging_panel = false
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.on_click(mouse.column, mouse.row, mouse.modifiers)
            }
            // Middle click on a folder (list or Places): open it in a new background tab.
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some(path) = self.folder_at(mouse.column, mouse.row) {
                    self.open_in_background_tab(path);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.open_context_menu(mouse.column, mouse.row)
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag.is_some() => {
                self.on_drag(mouse.column, mouse.row)
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag.is_some() => {
                let copy = mouse.modifiers.contains(KeyModifiers::CONTROL);
                self.on_drop(mouse.column, mouse.row, copy);
            }
            _ => return, // movement, other buttons: nothing to do
        }
        self.dirty = true;
    }

    fn on_click(&mut self, column: u16, row: u16, modifiers: KeyModifiers) {
        if self.menu.is_some() {
            let row_index = usize::from(row.saturating_sub(self.menu_area.y));
            let chosen = self
                .menu
                .as_ref()
                .and_then(|m| m.items.get(m.offset + row_index))
                .copied();
            self.menu = None;
            if self.menu_area.contains((column, row).into())
                && let Some(action) = chosen
                && self.action_available(action)
            {
                self.run_action(action);
            }
            return;
        }
        if let Some((selected, _)) = self.theme_picker {
            let inside = self.picker_area.contains((column, row).into());
            let index = usize::from(row.saturating_sub(self.picker_area.y));
            if inside && index < liman_widgets::theme::THEMES.len() {
                if index == selected {
                    self.save_theme();
                } else {
                    self.pick_theme(index);
                }
            }
            return;
        }
        if self.help_open {
            self.help_open = false;
            return;
        }
        if self.tab.term_mode == TermMode::Fullscreen {
            return;
        }
        if self.tab.term_mode == TermMode::Panel {
            let in_terminal = self.term_area.contains((column, row).into());
            self.tab.focus = if in_terminal {
                Focus::Terminal
            } else {
                Focus::Files
            };
            if in_terminal {
                return;
            }
        }
        if let Some(i) = self.sidebar_row_at(column, row) {
            // Keys go where you clicked: Del then means the sidebar item, never the selected
            // file in the list (a Del meant for the sidebar trashed ~/Projects).
            self.tab.focus = Focus::Places;
            self.on_sidebar_click(i, column);
            return;
        }
        if row == self.path_bar_area.y {
            let segments = self.path_segments();
            if let Some(i) = breadcrumb::segment_at(&segments, self.path_bar_area.x, column) {
                let path = segments[i].path.clone();
                if path != self.tab.cwd {
                    self.load(path);
                }
            }
            return;
        }
        if self.drawn_view == View::Detailed
            && let Some(key) =
                FileList::sort_key_at(self.list_area, ListMode::Detailed, column, row)
        {
            let descending = key == self.sort.key && !self.sort.descending;
            return self.set_sort(liman_core::sort::SortOrder { key, descending });
        }
        let Some(index) = self.entry_at(column, row) else {
            return;
        };
        // Ctrl+click: mark / unmark one; Shift+click: mark the range from the last click.
        if modifiers.contains(KeyModifiers::CONTROL) {
            let Some(path) = self.visible_entry(index).map(|e| e.path.clone()) else {
                return;
            };
            if !self.tab.marked.remove(&path) {
                self.tab.marked.insert(path);
            }
            self.tab.table.select(Some(index));
            self.click_anchor = Some(index);
            return;
        }
        if modifiers.contains(KeyModifiers::SHIFT) {
            let anchor = self
                .click_anchor
                .or(self.tab.table.selected())
                .unwrap_or(index);
            let (from, to) = (anchor.min(index), anchor.max(index));
            let paths: Vec<_> = self
                .visible_entries()
                .skip(from)
                .take(to - from + 1)
                .map(|e| e.path.clone())
                .collect();
            self.tab.marked.extend(paths);
            self.tab.table.select(Some(index));
            return;
        }
        self.click_anchor = Some(index);
        self.drag = Some(Drag {
            from: index,
            start: (column, row),
            active: false,
        });
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(at, i)| i == index && now.duration_since(at) <= DOUBLE_CLICK);
        self.tab.table.select(Some(index));
        if double {
            self.last_click = None;
            self.activate_selected();
        } else {
            self.last_click = Some((now, index));
        }
    }

    /// The visible entry under terminal cell (`column`, `row`) in the list or grid.
    pub(super) fn entry_at(&self, column: u16, row: u16) -> Option<usize> {
        let hit = match self.drawn_view.list_mode() {
            Some(mode) => {
                FileList::row_at(self.list_area, self.tab.table.offset(), mode, column, row)
            }
            None => GridView::index_at(
                self.list_area,
                self.tab.table.offset(),
                self.drawn_grid_level,
                column,
                row,
            ),
        };
        hit.filter(|&i| i < self.tab.visible.len())
    }

    /// The folder under the mouse: a folder in the list or a place in the sidebar (Recent is a
    /// list, not a folder).
    fn folder_at(&self, column: u16, row: u16) -> Option<PathBuf> {
        let in_list = self
            .entry_at(column, row)
            .and_then(|i| self.visible_entry(i))
            .filter(|e| e.is_dir)
            .map(|e| e.path.clone());
        in_list.or_else(
            || match self.sidebar_item(self.sidebar_row_at(column, row)?) {
                SideItem::Place(i) => Some(&self.sidebar[i])
                    .filter(|p| p.kind != liman_core::SpecialDir::Recent && p.path.is_dir())
                    .map(|p| p.path.clone()),
                SideItem::Dir(d) => Some(self.tree.rows()[d].path.clone()),
                SideItem::Label => None,
            },
        )
    }

    /// A drag becomes real after the mouse moved a couple of cells (a click may wobble).
    fn on_drag(&mut self, column: u16, row: u16) {
        // Out of the window: copied as a file, for Ctrl+V in the app next to this one.
        if self.drag.as_ref().is_some_and(|d| d.active) && self.drag_left_window(column, row) {
            return;
        }
        // Out of the window: copied as a file, for Ctrl+V in the app next to this one.
        if self.drag.as_ref().is_some_and(|d| d.active) && self.drag_left_window(column, row) {
            return;
        }
        if let Some(drag) = &mut self.drag
            && !drag.active
            && (column.abs_diff(drag.start.0) + row.abs_diff(drag.start.1)) >= 2
        {
            drag.active = true;
            self.message = Some(tr("Drop on a folder to move (hold Ctrl to copy)").into());
        }
    }

    /// Mouse released after a drag: move / copy onto the folder or place under the pointer.
    fn on_drop(&mut self, column: u16, row: u16, copy: bool) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !drag.active {
            return;
        }
        let Some(dragged) = self.visible_entry(drag.from).map(|e| e.path.clone()) else {
            return;
        };
        let sources: Vec<PathBuf> = if self.tab.marked.contains(&dragged) {
            self.targets()
        } else {
            vec![dragged]
        };
        // Dropped on the QUICK ACCESS title or the rule under the places: pin them there.
        if let Some(row) = self.sidebar_row_at(column, row)
            && (row == 0 || row == self.sidebar.len() + 1)
        {
            for path in sources {
                if !self.bookmarks.contains(&path) {
                    self.toggle_bookmark_for(path);
                }
            }
            return;
        }
        if !copy && let Some(path) = sources.iter().find(|p| self.is_protected(p)) {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            self.message = Some(trf(
                "“{}” is protected: it cannot be removed or moved",
                &[&name],
            ));
            return;
        }
        match self.folder_at(column, row) {
            // Dropped on Trash: a real (confirmed, undoable) trash, not a plain move into it.
            Some(dest) if dest.starts_with(&self.trash_dir) => {
                self.dialog = Some(Dialog::ConfirmTrash {
                    paths: sources,
                    inside: None,
                });
            }
            Some(dest)
                if !sources.contains(&dest) && !sources.iter().any(|s| dest.starts_with(s)) =>
            {
                self.drop_onto(sources, dest, copy);
            }
            _ => self.message = Some(tr("Dropped outside a folder: nothing done").into()),
        }
    }
}

//! The command palette (Ctrl+P) and the right-click menu: both list actions from `actions`.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{Action, App, Menu, actions, is_typing};

impl App {
    /// Ctrl+P: every action, filtered by what you type.
    pub(super) fn open_palette(&mut self) {
        self.menu = Some(Menu {
            items: actions::matching(""),
            selected: 0,
            query: Some(String::new()),
            at: (0, 0),
            offset: 0,
        });
    }

    /// Right click: actions for the entry under the mouse, or for the folder on empty space.
    pub(super) fn open_context_menu(&mut self, column: u16, row: u16) {
        let items = match self.entry_at(column, row) {
            Some(index) => {
                let path = self.visible_entry(index).map(|e| e.path.clone());
                if path.is_some_and(|p| !self.tab.marked.contains(&p)) {
                    self.tab.marked.clear(); // right-click on an unmarked entry acts on that entry only
                }
                self.tab.table.select(Some(index));
                Action::ON_ENTRY.to_vec()
            }
            None if self.list_area.contains((column, row).into()) => Action::ON_FOLDER.to_vec(),
            None => return,
        };
        self.menu = Some(Menu {
            items,
            selected: 0,
            query: None,
            at: (column, row),
            offset: 0,
        });
    }

    pub(super) fn on_menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        let last = menu.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.menu = None,
            KeyCode::Up => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Down => menu.selected = (menu.selected + 1).min(last),
            KeyCode::Enter => {
                let chosen = menu.items.get(menu.selected).copied();
                self.menu = None;
                if let Some(action) = chosen
                    && self.action_available(action)
                {
                    self.run_action(action);
                }
            }
            KeyCode::Backspace if menu.query.is_some() => {
                let query = menu.query.as_mut().expect("checked");
                query.pop();
                menu.items = actions::matching(query);
                menu.selected = 0;
            }
            KeyCode::Char(c) if menu.query.is_some() && is_typing(key) => {
                let query = menu.query.as_mut().expect("checked");
                query.push(c);
                menu.items = actions::matching(query);
                menu.selected = 0;
            }
            _ => {}
        }
    }
}

//! Results views: Ctrl+F searches the subfolders, and paths a terminal command printed are
//! shown as a list of files.

use std::path::PathBuf;

use liman_core::Entry;
use liman_core::i18n::trf;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::TableState;

use crate::worker;

use super::{App, Listing, Results, is_typing};

impl App {
    /// Shows the files a shell command printed, like a folder listing (Backspace returns to the folder).
    pub(super) fn load_results(&mut self, results: Results, base: PathBuf, paths: Vec<PathBuf>) {
        self.results_pending = Some(results);
        self.load_from_shell = true; // the shell is already in `base`
        self.start_loading(&base);
        worker::spawn_results(self.tx.clone(), self.generation, base, paths);
    }

    /// A command run in the terminal finished: if it printed paths, show them up here.
    pub(super) fn on_command_finished(&mut self, out: crate::terminal::CommandOutput) {
        // A command that moved the shell (`cd /usr/share`) is navigation, not a list of files:
        // the view follows the folder instead.
        let shell_now = self.tab.terminal.as_ref().and_then(|t| t.cwd());
        if shell_now.is_some_and(|now| now != out.cwd) {
            return;
        }
        let text = liman_core::results::strip_ansi(&out.bytes);
        let paths = liman_core::results::paths_from_output(&text, &out.cwd);
        if paths.is_empty() {
            return;
        }
        // Typed fast (or pasted), the line may not have been echoed yet when Enter was pressed:
        // then the command is the first printed line that is not one of the results.
        let command = if out.command.is_empty() {
            text.lines()
                .map(crate::terminal::strip_prompt)
                .find(|l| {
                    !l.is_empty()
                        && !paths
                            .iter()
                            .any(|p| p.ends_with(l.trim_start_matches("./")))
                })
                .unwrap_or_else(|| "command".into())
        } else {
            out.command
        };
        let results = Results {
            command,
            count: paths.len(),
            running: false,
        };
        self.load_results(results, out.cwd, paths);
    }

    /// Ctrl+F: every key searches again (live, like cardea); matches stream into a results view.
    /// ↑↓ move in the results while typing, Enter keeps them, Esc goes back to the folder.
    pub(super) fn on_search_key(&mut self, key: KeyEvent) {
        let Some(text) = &mut self.search_input else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.search_input = None;
                if self.tab.results.is_some() {
                    self.load(self.tab.cwd.clone());
                }
            }
            KeyCode::Enter => self.search_input = None,
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Backspace => {
                text.pop();
                self.search_live();
            }
            KeyCode::Char(c) if is_typing(key) => {
                text.push(c);
                self.search_live();
            }
            _ => {}
        }
    }

    /// Starts a search for the typed text (the older one stops: new generation). The view becomes
    /// an empty results list that fills as matches arrive.
    fn search_live(&mut self) {
        let needle = self.search_input.clone().unwrap_or_default();
        if needle.is_empty() {
            if self.tab.results.is_some() {
                self.load(self.tab.cwd.clone()); // nothing to search: back to the folder
            }
            return;
        }
        self.generation += 1;
        self.current_generation
            .store(self.generation, std::sync::atomic::Ordering::Relaxed);
        self.loading_path = None;
        self.results_pending = None;
        self.tab.results = Some(Results {
            command: trf("search “{}”", &[&needle]),
            count: 0,
            running: true,
        });
        self.tab.listing = Listing::Ready(Vec::new());
        self.tab.visible.clear();
        self.tab.marked.clear();
        self.tab.table = TableState::default();
        // Search the folder itself, not inside a previous result list.
        worker::spawn_search(
            self.tx.clone(),
            self.generation,
            self.current_generation.clone(),
            self.tab.cwd.clone(),
            needle,
            self.options.show_hidden,
        );
    }

    pub(super) fn on_search_found(&mut self, generation: u64, entries: Vec<Entry>, done: bool) {
        if generation != self.generation {
            return; // an older search
        }
        let Listing::Ready(list) = &mut self.tab.listing else {
            return;
        };
        list.extend(entries);
        let count = list.len();
        let dirs: Vec<PathBuf> = if done {
            list.iter()
                .filter(|e| e.is_dir)
                .map(|e| e.path.clone())
                .collect()
        } else {
            Vec::new()
        };
        if let Some(results) = &mut self.tab.results {
            results.count = count;
            results.running = !done;
        }
        self.refresh_visible();
        if self.tab.table.selected().is_none() {
            self.select_row(0);
        }
        if !dirs.is_empty() {
            worker::spawn_counts(
                self.tx.clone(),
                self.generation,
                self.current_generation.clone(),
                dirs,
                self.options,
            );
        }
        self.dirty = true;
    }
}

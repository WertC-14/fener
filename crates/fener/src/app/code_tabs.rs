//! Code tabs (fm-research ADR 0012): a text file opened in the file manager gets its own tab,
//! right of the folder tabs, holding a fener editor (`fener-core` + `fener-widgets`). The whole
//! area under the tab row is the editor; the file manager's panels are not shown. Alt+1 is
//! always back to the files.
//!
//! Keys of an active code tab go to the editor, except Alt+1…9 (tabs). The editor's slow work
//! (Find Files, grep) runs on worker threads and comes back as [`AppEvent::CodeItems`].
//!
//! Each code tab can have a shell under the text (Ctrl+/ or F4, as LazyVim's terminal): F6
//! moves the keys between the text and the shell, Ctrl+/ in the shell hides it. F5 saves the
//! file and types its build-and-run command there (`fener_core::run`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fener_core::picker::{self, Item, Kind};
use fener_core::{Document, Editor, Key, Mode, Request, State, files, tree};
use liman_core::FileType;
use liman_core::i18n::trf;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Widget};

use super::App;
use crate::event::AppEvent;
use crate::terminal::{Terminal, shell_quote};

/// Files bigger than this open the old way (desktop app / `$EDITOR`), not in a code tab.
const MAX_SIZE: u64 = 20 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_HITS: usize = 2_000;

pub struct CodeTab {
    /// Stays the same while tabs before it close (worker results find their tab by it).
    pub id: u64,
    pub editor: Editor,
    pub view: fener_widgets::View,
    /// The shell under the text, started on first use and kept while hidden.
    pub terminal: Option<Terminal>,
    pub term_open: bool,
    /// Keys go to the shell.
    pub term_focus: bool,
}

#[derive(Default)]
pub struct CodeTabs {
    pub tabs: Vec<CodeTab>,
    /// The code tab shown, or `None` when a folder tab is.
    pub active: Option<usize>,
    next_id: u64,
    /// Bumped by each worker job; a job that sees it change stops.
    generation: Arc<AtomicU64>,
    /// Tests set this: recent files are not written to the real home folder.
    pub(super) no_state: bool,
}

impl CodeTabs {
    pub fn active_editor(&self) -> Option<&Editor> {
        self.active.map(|i| &self.tabs[i].editor)
    }
}

/// Whether `entry` opens in a code tab: text, code and config files of a sensible size.
pub fn opens_in_code_tab(entry: &liman_core::Entry) -> bool {
    !entry.is_dir
        && matches!(
            entry.file_type,
            FileType::Code | FileType::Config | FileType::Text
        )
        && entry.size <= MAX_SIZE
}

impl App {
    /// Opens `path` in a code tab (the existing one if it is open already) and shows it.
    pub fn open_code_tab(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self
            .code
            .tabs
            .iter()
            .position(|t| t.editor.doc.path() == Some(path.as_path()))
        {
            self.code.active = Some(i);
            return;
        }
        let doc = match Document::open(&path) {
            Ok(doc) => doc,
            Err(e) => {
                self.message = Some(trf("Cannot open “{}”: {}", &[&path.display(), &e]));
                return;
            }
        };
        let mut editor = Editor::new(doc);
        editor.root = tree::project_root(&path);
        editor.state = State::load();
        // The folder the file is in, beside it (the user asked to see it; fener alone keeps
        // the tree closed, as LazyVim).
        editor.show_tree(false);
        editor.remember(&path);
        self.code.next_id += 1;
        self.code.tabs.push(CodeTab {
            id: self.code.next_id,
            editor,
            view: fener_widgets::View::default(),
            terminal: None,
            term_open: false,
            term_focus: false,
        });
        let i = self.code.tabs.len() - 1;
        self.code.active = Some(i);
        self.code_requests(i);
    }

    /// Alt+N or a click on chip N: folder tabs first, then code tabs.
    pub(super) fn switch_any_tab(&mut self, to: usize) {
        let folders = self.tabs.count();
        if to < folders {
            self.code.active = None;
            self.switch_tab(to);
        } else if to - folders < self.code.tabs.len() {
            self.code.active = Some(to - folders);
        }
        self.dirty = true;
    }

    /// Closes code tab `i`. Unsaved changes keep it open unless `force` (`:q!` already chose
    /// to drop them).
    pub(super) fn close_code_tab(&mut self, i: usize, force: bool) {
        let tab = &self.code.tabs[i];
        if tab.editor.doc.is_modified() && !force {
            self.code.active = Some(i);
            self.code.tabs[i].editor.message =
                Some("No write since last change (:w saves, :q! drops the changes)".into());
            return;
        }
        self.code.tabs.remove(i);
        self.code.active = match self.code.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
    }

    /// Before liman quits: the first code tab with unsaved changes, shown with a warning.
    pub(super) fn unsaved_code_tab(&mut self) -> bool {
        let Some(i) = self
            .code
            .tabs
            .iter()
            .position(|t| t.editor.doc.is_modified())
        else {
            return false;
        };
        self.code.active = Some(i);
        self.code.tabs[i].editor.message =
            Some("Unsaved changes: :w saves, :q! drops them, then quit fener again".into());
        self.dirty = true;
        true
    }

    /// A key while a code tab is shown.
    pub(super) fn on_code_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        if let KeyCode::Char(c @ '1'..='9') = key.code
            && key.modifiers.contains(KeyModifiers::ALT)
        {
            return self.switch_any_tab(c as usize - '1' as usize);
        }
        let Some(i) = self.code.active else {
            return;
        };
        let tab = &mut self.code.tabs[i];
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let toggle = key.code == KeyCode::F(4)
            || (ctrl && matches!(key.code, KeyCode::Char('/' | '_' | '7')));
        if tab.term_focus && key.code != KeyCode::F(5) {
            if toggle {
                tab.term_open = false;
                tab.term_focus = false;
            } else if key.code == KeyCode::F(6) {
                tab.term_focus = false;
            } else if let Some(term) = &mut tab.terminal {
                term.send_key(key);
            }
            return;
        }
        if key.code == KeyCode::F(6) && tab.term_open {
            tab.term_focus = true;
            return;
        }
        let Some(key) = convert(key) else {
            return;
        };
        self.code.tabs[i].editor.handle_key(key);
        self.code_requests(i);
        // fener sets `quit` only when nothing is unsaved or `:q!` said to drop it.
        if self.code.tabs[i].editor.quit {
            self.close_code_tab(i, true);
        }
    }

    pub(super) fn on_code_paste(&mut self, text: &str) {
        let Some(i) = self.code.active else {
            return;
        };
        for c in text.chars() {
            let key = if c == '\n' { Key::Enter } else { Key::Char(c) };
            self.code.tabs[i].editor.handle_key(key);
        }
        self.dirty = true;
    }

    /// A worker's list for a code tab's picker.
    pub(super) fn on_code_items(&mut self, id: u64, kind: Kind, query: &str, items: Vec<Item>) {
        if let Some(tab) = self.code.tabs.iter_mut().find(|t| t.id == id) {
            tab.editor.receive(kind, query, items);
            self.dirty = true;
        }
    }

    /// Does what code tab `i`'s editor asked for (state on the spot, lists on a worker).
    fn code_requests(&mut self, i: usize) {
        let tab = &mut self.code.tabs[i];
        for request in std::mem::take(&mut tab.editor.requests) {
            match request {
                Request::SaveState => {
                    // Remembering is a convenience: a read-only home folder must not stop editing.
                    if !self.code.no_state {
                        let _ = tab.editor.state.save();
                    }
                    continue;
                }
                Request::ToggleTerminal => {
                    let root = tab.editor.root.clone();
                    if open_terminal(tab, &root, &self.tx) {
                        tab.term_focus = true;
                    }
                    continue;
                }
                Request::Run(run) => {
                    let fresh = tab.terminal.is_none();
                    if !open_terminal(tab, &run.dir, &self.tx) {
                        continue;
                    }
                    let Some(term) = &mut tab.terminal else {
                        continue;
                    };
                    if !fresh && !term.is_idle() {
                        tab.editor.message = Some(
                            "Something still runs in the terminal (Ctrl+C there stops it)".into(),
                        );
                        tab.term_focus = true;
                        continue;
                    }
                    // A new shell starts in the right folder; an old one is taken there.
                    let line = if fresh || term.cwd().as_deref() == Some(run.dir.as_path()) {
                        format!("{}\r", run.command)
                    } else {
                        let dir = shell_quote(&run.dir.to_string_lossy());
                        format!("cd {dir} && {}\r", run.command)
                    };
                    term.type_text(&line);
                    tab.term_focus = true;
                    continue;
                }
                Request::ListFiles | Request::Grep(_) => {}
            }
            let (id, root, tx) = (tab.id, tab.editor.root.clone(), self.tx.clone());
            let mine = self.code.generation.fetch_add(1, Ordering::Relaxed) + 1;
            let generation = Arc::clone(&self.code.generation);
            let cancel = move || generation.load(Ordering::Relaxed) != mine;
            std::thread::spawn(move || {
                let (kind, query, items) = match request {
                    Request::ListFiles => {
                        let paths = files::list(&root, MAX_FILES, &cancel);
                        (Kind::Files, String::new(), picker::file_items(&root, paths))
                    }
                    Request::Grep(query) => {
                        let hits = files::grep(&root, &query, MAX_HITS, &cancel);
                        (Kind::Grep, query, picker::grep_items(&root, hits))
                    }
                    _ => return,
                };
                if !cancel() {
                    let _ = tx.send(AppEvent::CodeItems {
                        id,
                        kind,
                        query,
                        items,
                    });
                }
            });
        }
    }

    /// Draws the active code tab into `area`; returns where the terminal cursor goes.
    pub fn render_code_tab(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
    ) -> Option<(u16, u16)> {
        let i = self.code.active?;
        let theme = theme_from_liman();
        let tab = &mut self.code.tabs[i];
        let mut text_area = area;
        let mut term_cursor = None;
        if tab.term_open
            && let Some(term) = &mut tab.terminal
        {
            let height = (area.height * 35 / 100)
                .max(8)
                .min(area.height.saturating_sub(6));
            let [top, bottom] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(height)]).areas(area);
            text_area = top;
            let accent = if tab.term_focus {
                liman_widgets::theme::accent()
            } else {
                liman_widgets::theme::dim()
            };
            let hint = if tab.term_focus {
                " Terminal · F6 to the text · Ctrl+/ hide · F5 run again "
            } else {
                " Terminal · F6 to type here · Ctrl+/ open "
            };
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(hint, Style::new().fg(accent)));
            let inner = block.inner(bottom);
            block.render(bottom, buf);
            term_cursor = crate::ui::draw_term_screen(buf, inner, term, tab.term_focus);
        }
        let text_cursor =
            fener_widgets::render(buf, text_area, &mut tab.editor, &mut tab.view, &theme);
        if tab.term_focus {
            term_cursor
        } else {
            text_cursor
        }
    }

    /// The code tab that owns the shell `id`, if any.
    pub(super) fn code_tab_with_terminal(&mut self, id: u64) -> Option<&mut CodeTab> {
        self.code
            .tabs
            .iter_mut()
            .find(|t| t.terminal.as_ref().is_some_and(|term| term.id == id))
    }

    /// Insert mode and the command line want a bar cursor; `None` when no code tab is shown.
    pub fn code_cursor_bar(&self) -> Option<bool> {
        let editor = self.code.active_editor()?;
        Some(matches!(
            editor.mode,
            Mode::Insert | Mode::Command | Mode::Search
        ))
    }
}

/// Shows the code tab's shell, starting it in `dir` if it is not running. False if it cannot.
fn open_terminal(tab: &mut CodeTab, dir: &Path, tx: &std::sync::mpsc::Sender<AppEvent>) -> bool {
    if tab.terminal.is_none() {
        // The real size comes with the first frame.
        match Terminal::spawn(dir, 10, 80, tx.clone()) {
            Ok(term) => tab.terminal = Some(term),
            Err(e) => {
                tab.editor.message = Some(format!("Cannot start a shell: {e}"));
                return false;
            }
        }
    }
    tab.term_open = true;
    true
}

/// fener's colors made from liman's theme, so a code tab looks like the rest of liman.
fn theme_from_liman() -> fener_widgets::Theme {
    use liman_widgets::theme;
    let p = theme::palette();
    fener_widgets::Theme {
        name: p.name,
        bg: p.bg,
        bg_dark: p.bar_bg,
        bg_line: p.selected_bg,
        bg_visual: p.marked_bg,
        bg_search: p.border,
        fg: p.fg,
        fg_dim: p.dim,
        gutter: p.border,
        orange: theme::type_color(FileType::Presentation),
        blue: theme::accent(),
        green: theme::type_color(FileType::Spreadsheet),
        magenta: theme::type_color(FileType::Code),
        yellow: theme::type_color(FileType::Archive),
        red: theme::type_color(FileType::Pdf),
        cyan: theme::type_color(FileType::Image),
    }
}

/// A terminal key as fener's key (`None` for keys the editor does not use).
fn convert(key: KeyEvent) -> Option<Key> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Esc => Key::Esc,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Tab => Key::Tab,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    })
}

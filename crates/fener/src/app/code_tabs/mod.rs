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
use liman_core::i18n::{tr, trf};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Widget};

use super::App;
use crate::event::AppEvent;
use crate::terminal::{Terminal, shell_quote};

mod language;
mod split;

use language::Server;
use split::Pane;

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
    /// The language server this file talks to (an id in `CodeTabs::servers`).
    server: Option<u64>,
    /// The document version the server has, and whether it was unsaved then.
    sent_version: u64,
    sent_modified: bool,
    /// The file the server was told about (another file in this pane re-attaches).
    lsp_path: Option<std::path::PathBuf>,
    /// The other window of a split (`Space |`, `Space -`); the fields above are the focused one.
    other: Option<Box<Pane>>,
    /// Side by side (`|`) or one above the other (`-`).
    vertical: bool,
    /// Both windows show the same file: one document, held by the focused window (the other
    /// borrows it to be drawn), so typing in one shows in the other.
    shared: bool,
    /// The other window is drawn first (left / top).
    other_first: bool,
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
    /// `~/.config/fener/config.toml` (ADR 0013), applied to every new code tab.
    pub config: fener_core::config::Config,
    /// What was wrong with the config file; shown once, in the first code tab.
    pub config_error: Option<String>,
    /// Running language servers, one per (command, project root).
    servers: Vec<Server>,
    /// Servers that could not start (said once, not tried again).
    missing: std::collections::HashSet<String>,
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
        editor.apply_config(&self.code.config);
        if let Some(error) = self.code.config_error.take() {
            editor.message = Some(format!("Config ignored: {error}"));
        }
        // The project's folder tree beside the file (`[editor] tree-open`).
        if self.code.config.editor.tree_open {
            editor.show_tree(false);
        }
        editor.remember(&path);
        self.code.next_id += 1;
        let mut view = fener_widgets::View::default();
        let tx = self.tx.clone();
        view.on_ready = Some(std::sync::Arc::new(move || {
            let _ = tx.send(AppEvent::Redraw);
        }));
        self.code.tabs.push(CodeTab {
            id: self.code.next_id,
            editor,
            view,
            terminal: None,
            server: None,
            sent_version: 0,
            sent_modified: false,
            lsp_path: None,
            other: None,
            vertical: true,
            other_first: false,
            shared: false,
            term_open: false,
            term_focus: false,
        });
        let i = self.code.tabs.len() - 1;
        self.code.active = Some(i);
        self.lsp_attach(i);
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
        if tab.is_modified() && !force {
            self.code.active = Some(i);
            self.code.tabs[i].editor.message =
                Some("No write since last change (:w saves, :q! drops the changes)".into());
            return;
        }
        let closed = self.code.tabs.remove(i);
        // Both windows of a split: the other one may have its own file open on a server, and
        // a server must hear about the close before that file can be opened there again.
        let windows = std::iter::once((closed.server, closed.lsp_path))
            .chain(closed.other.map(|o| (o.server, o.lsp_path)));
        for (id, path) in windows {
            if let (Some(id), Some(path)) = (id, path)
                && let Some(server) = self.code.servers.iter().find(|s| s.id == id)
            {
                server.client.close(&path);
            }
        }
        self.code.active = match self.code.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
    }

    /// Before liman quits: the first code tab with unsaved changes, shown with a warning.
    pub(super) fn unsaved_code_tab(&mut self) -> bool {
        let Some(i) = self.code.tabs.iter().position(CodeTab::is_modified) else {
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
            } else if key.code == KeyCode::Tab
                && tab
                    .terminal
                    .as_ref()
                    .is_none_or(|t| t.typed_text().trim().is_empty())
            {
                // Tab on an empty command line: up to the text (with text typed, it completes).
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
        // Ctrl+H/J/K/L between the windows of a split (LazyVim's window keys).
        let tab = &mut self.code.tabs[i];
        if ctrl
            && tab.other.is_some()
            && tab.editor.mode == Mode::Normal
            && tab.editor.picker.is_none()
            && !tab.editor.tree_focus
        {
            let first_focused = !tab.other_first;
            let go = match (tab.vertical, key.code) {
                (true, KeyCode::Char('l')) => first_focused,
                (true, KeyCode::Char('h')) => !first_focused,
                (false, KeyCode::Char('j')) => first_focused,
                (false, KeyCode::Char('k')) => !first_focused,
                _ => false,
            };
            if go {
                self.lsp_sync(i);
                self.code.tabs[i].swap_panes();
                return;
            }
        }
        let Some(key) = convert(key) else {
            return;
        };
        self.code.tabs[i].editor.handle_key(key);
        self.split_sync(i);
        self.lsp_sync(i);
        self.code_requests(i);
        // fener sets `quit` only when nothing is unsaved or `:q!` said to drop it. In a split
        // only this window closes.
        if self.code.tabs[i].editor.quit {
            if self.code.tabs[i].other.is_some() {
                self.code.tabs[i].editor.quit = false;
                self.close_pane(i, true);
            } else {
                self.close_code_tab(i, true);
            }
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
        self.split_sync(i);
        self.lsp_sync(i);
        self.dirty = true;
    }

    /// A formatter finished for code tab `id`.
    pub(super) fn on_code_formatted(
        &mut self,
        id: u64,
        version: u64,
        result: Result<String, String>,
    ) {
        if let Some(tab) = self.code.tabs.iter_mut().find(|t| t.id == id) {
            tab.editor.formatted(version, result);
            self.dirty = true;
        }
        if let Some(i) = self.code.tabs.iter().position(|t| t.id == id) {
            self.lsp_sync(i);
        }
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
        let mut switch_to = None;
        let mut restore = false;
        let mut split = None;
        let mut close_window = false;
        let (folders, total) = (self.tabs.count(), self.tabs.count() + self.code.tabs.len());
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
                Request::SwitchTab(n) => {
                    switch_to = Some(n);
                    continue;
                }
                Request::CycleTab(step) => {
                    let here = folders + i;
                    let to = (here as isize + step).rem_euclid(total as isize) as usize;
                    switch_to = Some(to);
                    continue;
                }
                // Space g g: lazygit gets the whole terminal (as $EDITOR does for liman).
                Request::Lazygit => {
                    if in_path("lazygit") {
                        let root = tab.editor.root.clone();
                        self.external = Some(("lazygit -p".into(), root));
                    } else {
                        tab.editor.message =
                            Some("lazygit is not installed: sudo pacman -S lazygit".into());
                    }
                    continue;
                }
                Request::RestoreSession => {
                    restore = true;
                    continue;
                }
                Request::Split { vertical } => {
                    split = Some(vertical);
                    continue;
                }
                Request::CloseWindow => {
                    close_window = true;
                    continue;
                }
                // Space c f: the formatter gets the text on stdin, on a worker thread.
                Request::Format {
                    command,
                    dir,
                    version,
                } => {
                    let text = tab.editor.doc.rope.to_string();
                    let (id, tx) = (tab.id, self.tx.clone());
                    std::thread::spawn(move || {
                        let result = run_formatter(&command, &dir, &text);
                        let _ = tx.send(AppEvent::CodeFormatted {
                            id,
                            version,
                            result,
                        });
                    });
                    continue;
                }
                // Space t, Ctrl+/: open the shell and type there, or close it if it is open.
                // (Inside the shell Ctrl+/ closes it too; Tab or F6 go up to the text.)
                Request::ToggleTerminal => {
                    if tab.term_open {
                        tab.term_open = false;
                        tab.term_focus = false;
                    } else {
                        let root = tab.editor.root.clone();
                        if open_terminal(tab, &root, &self.tx) {
                            tab.term_focus = true;
                        }
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
                Request::Lsp(ask) => {
                    let (Some(server), Some(path)) = (tab.server, tab.editor.doc.path()) else {
                        continue;
                    };
                    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
                    if let Some(s) = self.code.servers.iter().find(|s| s.id == server) {
                        use fener_core::LspAsk;
                        match ask {
                            LspAsk::Hover(pos) => s.client.hover(&path, pos),
                            LspAsk::Definition(pos) => s.client.definition(&path, pos),
                            LspAsk::References(pos) => s.client.references(&path, pos),
                            LspAsk::Completion(pos, version) => {
                                s.client.completion(&path, pos, version)
                            }
                        }
                    }
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
        if let Some(n) = switch_to {
            self.switch_any_tab(n);
        }
        if restore {
            self.restore_session();
        }
        if let Some(vertical) = split {
            self.split(i, vertical);
        }
        if close_window {
            self.close_pane(i, false);
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
        // Changes that came without a key (formatting, a completion) move the other cursor too.
        tab.follow_changes();
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
                " Terminal · Tab (empty line) up to the text · Ctrl+/ close · F5 run again "
            } else {
                " Terminal · F6 to type here · Space t close "
            };
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(hint, Style::new().fg(accent)));
            let inner = block.inner(bottom);
            block.render(bottom, buf);
            term_cursor = crate::ui::draw_term_screen(buf, inner, term, tab.term_focus);
        }
        if let Some(other) = tab.other.as_deref_mut() {
            // Two windows with a line between them; the focused one gets the cursor.
            let [a, line, b] = if tab.vertical {
                Layout::horizontal([
                    Constraint::Fill(1),
                    Constraint::Length(1),
                    Constraint::Fill(1),
                ])
                .areas(text_area)
            } else {
                Layout::vertical([
                    Constraint::Fill(1),
                    Constraint::Length(1),
                    Constraint::Fill(1),
                ])
                .areas(text_area)
            };
            let rule = if tab.vertical { "│" } else { "─" };
            for y in line.top()..line.bottom() {
                for x in line.left()..line.right() {
                    buf[(x, y)]
                        .set_symbol(rule)
                        .set_style(Style::new().fg(liman_widgets::theme::border()));
                }
            }
            let (mine, theirs) = if tab.other_first { (b, a) } else { (a, b) };
            // A shared document is lent to the other window while it is drawn.
            if tab.shared {
                std::mem::swap(&mut tab.editor.doc, &mut other.editor.doc);
            }
            fener_widgets::render(buf, theirs, &mut other.editor, &mut other.view, &theme);
            if tab.shared {
                std::mem::swap(&mut tab.editor.doc, &mut other.editor.doc);
            }
            text_area = mine;
        }
        let text_cursor =
            fener_widgets::render(buf, text_area, &mut tab.editor, &mut tab.view, &theme);
        if tab.term_focus {
            term_cursor
        } else {
            text_cursor
        }
    }

    /// Writes the open code tabs (file and cursor line) for the next start (Space q s, Alt+S).
    /// An empty list keeps the last one: quitting from the files alone forgets nothing.
    pub fn save_session(&self) {
        let files: Vec<(std::path::PathBuf, usize)> = self
            .code
            .tabs
            .iter()
            .filter_map(|t| {
                Some((
                    std::path::absolute(t.editor.doc.path()?).ok()?,
                    t.editor.line(),
                ))
            })
            .collect();
        if !files.is_empty() && !self.code.no_state {
            let _ = fener_core::state::save_session(&files);
        }
    }

    /// Opens the last session's files again (those not open already), each at its line.
    pub(super) fn restore_session(&mut self) {
        let files = fener_core::state::load_session();
        if files.is_empty() {
            self.message = Some(tr("No earlier session to restore").into());
            return;
        }
        for (path, line) in files {
            self.open_code_tab(&path);
            if let Some(i) = self.code.active
                && self.code.tabs[i].editor.doc.path() == Some(path.as_path())
            {
                let editor = &mut self.code.tabs[i].editor;
                let last = fener_core::text::line_count(&editor.doc.rope).saturating_sub(1);
                editor.cursor = fener_core::text::first_non_blank(&editor.doc.rope, line.min(last));
            }
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

/// Runs a formatter: `text` on stdin, the formatted text from stdout, or the first line of
/// what it said on stderr.
fn run_formatter(command: &str, dir: &Path, text: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", command])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let input = text.to_string();
    // Written from its own thread: a big file must not fill the pipe while stdout waits.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        let first = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        Err(if output.status.code() == Some(127) {
            format!(
                "{} is not installed",
                command.split_whitespace().next().unwrap_or(command)
            )
        } else if first.is_empty() {
            format!("exit status {}", output.status)
        } else {
            first.to_string()
        })
    }
}

/// Whether `program` is on `$PATH`.
fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
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
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => Key::Alt(c),
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

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::super::test_util::{ctrl, key, keys, press, select, setup, wait_picker};

    #[test]
    fn text_files_open_in_code_tabs_and_unsaved_changes_block_quitting() {
        let (dir, mut app, _rx) = setup("code");
        app.code.no_state = true;
        select(&mut app, "a.txt");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.code.active, Some(0));
        let labels = app.tab_labels();
        assert_eq!(labels.len(), 2);
        assert!(labels[1].active && labels[1].title.ends_with("a.txt"));
        // x deletes in the editor; Alt+1 goes back to liman, where q must not lose the change.
        keys(&mut app, "x");
        assert!(
            app.tab_labels()[1].has_shell,
            "the dot marks unsaved changes"
        );
        app.handle(key(KeyCode::Char('1'), KeyModifiers::ALT));
        assert_eq!(app.code.active, None);
        app.run_action(crate::app::Action::Quit);
        assert!(app.running);
        assert_eq!(app.code.active, Some(0), "the unsaved tab is shown");
        // :q! drops the change and closes the tab.
        keys(&mut app, ":q!");
        press(&mut app, KeyCode::Enter);
        assert!(app.code.tabs.is_empty() && app.code.active.is_none());
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "a");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_code_tab_splits_into_two_windows_with_a_file_each() {
        let (dir, mut app, rx) = setup("split");
        app.code.no_state = true;
        select(&mut app, "a.txt");
        press(&mut app, KeyCode::Enter);
        // Space | : a new window on the right that asks for its file.
        keys(&mut app, " |");
        wait_picker(&mut app, &rx);
        keys(&mut app, "b.txt");
        press(&mut app, KeyCode::Enter);
        let tab = &app.code.tabs[0];
        let name = |e: &fener_core::Editor| e.doc.path().unwrap().file_name().unwrap().to_owned();
        assert_eq!(name(&tab.editor), "b.txt");
        assert_eq!(name(&tab.other.as_ref().unwrap().editor), "a.txt");
        // Ctrl+H: back to the left window; :q closes it, the right one stays.
        ctrl(&mut app, 'h');
        assert_eq!(name(&app.code.tabs[0].editor), "a.txt");
        keys(&mut app, ":q");
        press(&mut app, KeyCode::Enter);
        let tab = &app.code.tabs[0];
        assert!(tab.other.is_none());
        assert_eq!(name(&tab.editor), "b.txt");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn both_windows_of_a_split_share_one_file() {
        let (dir, mut app, rx) = setup("shared");
        app.code.no_state = true;
        let text = |e: &fener_core::Editor| e.doc.rope.to_string();
        select(&mut app, "a.txt");
        press(&mut app, KeyCode::Enter);
        // Space | and Esc: the new window keeps the same file, as Vim's :vsplit.
        keys(&mut app, " |");
        wait_picker(&mut app, &rx);
        keys(&mut app, "\x1b");
        assert_eq!(text(&app.code.tabs[0].editor), "a");
        // A line typed above on the right, seen on the left.
        keys(&mut app, "OX\x1b");
        ctrl(&mut app, 'h');
        let tab = &app.code.tabs[0];
        assert_eq!(text(&tab.editor), "X\na");
        assert!(tab.editor.doc.is_modified());
        // The left cursor was on "a" and moved down with it.
        assert_eq!(tab.editor.cursor, 2);
        // :q here closes this window only; nothing is lost.
        keys(&mut app, ":q");
        press(&mut app, KeyCode::Enter);
        let tab = &app.code.tabs[0];
        assert!(tab.other.is_none());
        assert_eq!(text(&tab.editor), "X\na");
        assert_eq!(tab.editor.doc.path().unwrap().file_name().unwrap(), "a.txt");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_window_that_opens_another_file_leaves_the_shared_one_to_the_other() {
        let (dir, mut app, rx) = setup("unshare");
        app.code.no_state = true;
        select(&mut app, "a.txt");
        press(&mut app, KeyCode::Enter);
        keys(&mut app, " |");
        wait_picker(&mut app, &rx);
        keys(&mut app, "\x1b");
        keys(&mut app, "iX\x1b");
        // Unsaved, yet this window may show b.txt: the left window still has a.txt.
        keys(&mut app, " ");
        keys(&mut app, " ");
        wait_picker(&mut app, &rx);
        keys(&mut app, "b.txt");
        press(&mut app, KeyCode::Enter);
        let tab = &app.code.tabs[0];
        let other = &tab.other.as_ref().unwrap().editor;
        assert_eq!(tab.editor.doc.path().unwrap().file_name().unwrap(), "b.txt");
        assert_eq!(other.doc.rope.to_string(), "Xa");
        assert!(other.doc.is_modified());
        // Closing the tab now would lose the left window's change: it asks.
        keys(&mut app, " ");
        app.close_code_tab(0, false);
        assert_eq!(app.code.tabs.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}

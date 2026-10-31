//! The editor: one document, a cursor and Vim's modes. Keys come in, the text and the cursor
//! change; nothing here draws.
//!
//! A Normal-mode command is parsed as a small grammar instead of a table of every combination
//! (fm-research `notes/edtui.md`, modit's `ViCmd`): `[count] [operator [count]] (motion | object)`,
//! so `d3w`, `2dd`, `ci"` and `y}` all come from the same few pieces. The Space key opens
//! LazyVim-style leader commands; while one is being typed, [`Editor::leader_menu`] lists what can
//! follow (which-key).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::command::{Command, GROUPS, When, bindings};
use crate::config::{Config, LineNumbers};
use crate::document::Document;
use crate::lsp;
use crate::picker::Picker;
use crate::state::State;
use crate::syntax::Lang;
use crate::text;
use crate::tree::Tree;

mod cmdline;
mod commands;
mod grammar;
mod insert;
mod language;
mod normal;
mod panes;
mod visual;

use grammar::Motion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    /// F1…F12.
    F(u8),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    /// `:` command line.
    Command,
    /// `/` or `?` search line.
    Search,
}

/// Lines kept visible above and below the cursor (LazyVim: `scrolloff = 4`).
const SCROLL_OFF: usize = 4;
/// Work the editor asks the app to do (it has the threads and the disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Show tab `n` (0-based; `Space 1` is 0, the file manager).
    SwitchTab(usize),
    /// Shift+H / Shift+L: the tab before / after this one.
    CycleTab(isize),
    /// Space g g: lazygit in the project, full screen.
    Lazygit,
    /// Space q s: open the files of the last session again.
    RestoreSession,
    /// Space | / Space -: a second window in this tab (side by side or one above the other).
    Split { vertical: bool },
    /// Space q w: close this window of a split.
    CloseWindow,
    /// Something for the language server, with positions in its units.
    Lsp(LspAsk),
    /// Space c f: run `command` in `dir` with the text on stdin; the result goes to
    /// [`Editor::formatted`] with `version` (stale if the text changed meanwhile).
    Format {
        command: String,
        dir: PathBuf,
        version: u64,
    },
    /// List the files under `root` for the Files picker.
    ListFiles,
    /// Search the files under `root` for this text, for the Grep picker.
    Grep(String),
    /// `state` changed (recent files, theme): write it.
    SaveState,
    /// Show or hide the terminal under the text (and give it the keys).
    ToggleTerminal,
    /// Type this command in the terminal (opened in its folder if needed) and run it.
    Run(crate::run::Run),
}

/// A question for the language server (ADR 0016).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspAsk {
    Hover(lsp::Pos),
    Definition(lsp::Pos),
    References(lsp::Pos),
    /// With the document version it was asked for.
    Completion(lsp::Pos, u64),
}

/// What the language server told this document, and its pop-ups.
#[derive(Debug, Default)]
pub struct LspState {
    /// A server is attached (`K`, `gd`, completion ask it).
    pub active: bool,
    pub encoding: lsp::Encoding,
    pub diagnostics: Vec<lsp::Diagnostic>,
    /// `K`: the answer, shown until the next key.
    pub hover: Option<String>,
    pub completion: Option<CompletionMenu>,
}

/// The completion pop-up in Insert mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionMenu {
    pub items: Vec<lsp::Completion>,
    /// Indices into `items` that fit the word typed so far, best first.
    pub shown: Vec<usize>,
    pub selected: usize,
    /// Where the word being completed starts (char index).
    pub start: usize,
}

/// Text yanked or deleted, and whether it was whole lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

pub struct Editor {
    pub doc: Document,
    /// Char index of the cursor.
    pub cursor: usize,
    pub mode: Mode,
    /// Column that up / down movement aims at (kept across short lines).
    desired_col: Option<usize>,
    /// Keys of the Normal / Visual command being typed.
    pending: Vec<crate::editor::Key>,
    /// The other end of the Visual selection.
    pub anchor: usize,
    pub register: Register,
    /// Text of the `:` or `/` line being typed.
    pub cmdline: String,
    search_forward: bool,
    pub last_search: Option<String>,
    /// Matches of `last_search` are highlighted (`:noh` turns it off until the next search).
    pub highlight_search: bool,
    last_find: Option<Motion>,
    /// One-line message for the command line (errors, "written").
    pub message: Option<String>,
    // `.` repeat: the keys of the command being typed, and of the last one that changed the text.
    recording: Vec<Key>,
    recording_insert: bool,
    last_change: Vec<Key>,
    version_at_start: u64,
    replaying: bool,
    /// First line on screen, and how many lines fit (set by the UI before drawing).
    pub top: usize,
    pub view_height: usize,
    pub number: bool,
    pub relative_number: bool,
    pub quit: bool,
    /// Another window shows this document too: closing this one or opening another file here
    /// loses nothing, so unsaved changes do not stop it.
    pub shared: bool,
    /// The shared document this window let go of when it opened another file (the app hands
    /// it back to the other window).
    pub left: Option<Document>,
    /// The start screen is shown, with this menu row selected.
    pub dashboard: Option<usize>,
    pub picker: Option<Picker>,
    /// Taken and done by the app after each key.
    pub requests: Vec<Request>,
    /// The folder Find Files and Find Text search.
    pub root: PathBuf,
    pub state: State,
    /// Names of the themes the UI has, and the one in use (index).
    pub themes: Vec<&'static str>,
    pub theme: usize,
    /// The folder tree on the left (`Space e`), and whether keys go to it.
    pub tree: Option<Tree>,
    pub tree_focus: bool,
    /// Markdown shown formatted (`Space r`, Ctrl+E); Markdown files open this way. `reader_top`:
    /// the first source line on screen.
    pub reader: bool,
    pub reader_top: usize,
    /// Spaces per indent level (`[editor] indent`; LazyVim: `shiftwidth = 2`).
    pub indent: usize,
    /// Brackets and quotes close themselves (`[editor] auto-pairs`; LazyVim: mini.pairs).
    pub auto_pairs: bool,
    /// The language server's diagnostics, hover and completion for this document.
    pub lsp: LspState,
    /// Keys from the config file's `[keys]`, checked before the built-in ones.
    pub keymap: HashMap<Key, Command>,
    /// The config file (`[run]` commands, options for new tabs).
    pub config: Config,
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            cursor: 0,
            mode: Mode::Normal,
            desired_col: None,
            pending: Vec::new(),
            anchor: 0,
            register: Register::default(),
            cmdline: String::new(),
            search_forward: true,
            last_search: None,
            highlight_search: false,
            last_find: None,
            message: None,
            recording: Vec::new(),
            recording_insert: false,
            last_change: Vec::new(),
            version_at_start: 0,
            replaying: false,
            top: 0,
            view_height: 20,
            number: true,
            // Absolute numbers: they stay put while the cursor moves (the user's choice;
            // LazyVim has relative ones, `Space u L`).
            relative_number: false,
            quit: false,
            shared: false,
            left: None,
            dashboard: None,
            picker: None,
            requests: Vec::new(),
            root: PathBuf::from("."),
            state: State::default(),
            themes: Vec::new(),
            theme: 0,
            tree: None,
            tree_focus: false,
            reader: false,
            reader_top: 0,
            indent: 2,
            auto_pairs: true,
            lsp: LspState::default(),
            keymap: HashMap::new(),
            config: Config::default(),
        }
        .reader_for_markdown()
    }

    /// Markdown files start in the reader.
    fn reader_for_markdown(mut self) -> Self {
        self.reader = self.doc.path().map(Lang::from_path) == Some(Lang::Markdown);
        self.reader_top = 0;
        self
    }

    pub fn line(&self) -> usize {
        text::line_of(&self.doc.rope, self.cursor)
    }

    pub fn col(&self) -> usize {
        text::col_of(&self.doc.rope, self.cursor)
    }

    /// The Visual selection as char indices `from..to` (whole lines in Visual Line mode).
    pub fn selection(&self) -> Option<(usize, usize)> {
        let rope = &self.doc.rope;
        let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        match self.mode {
            Mode::Visual => Some((a, (b + 1).min(rope.len_chars()))),
            Mode::VisualLine => {
                let last = text::line_of(rope, b);
                Some((
                    text::line_start(rope, text::line_of(rope, a)),
                    text::line_end(rope, last),
                ))
            }
            _ => None,
        }
    }

    /// Code or Markdown: decides which Space keys are offered.
    pub fn context(&self) -> When {
        match self.doc.path().map(Lang::from_path) {
            Some(Lang::Markdown) => When::Markdown,
            _ => When::Code,
        }
    }

    /// Keys typed so far for a command in progress (shown at the right of the status line).
    pub fn pending_keys(&self) -> String {
        self.pending.iter().map(key_label).collect()
    }

    /// While a leader command is being typed: the keys that can come next and what they do
    /// (`+name` for a group). The which-key box.
    pub fn leader_menu(&self) -> Option<Vec<(char, &'static str)>> {
        let start = self.pending.iter().position(|k| *k == Key::Char(' '))?;
        let typed: String = self.pending[start + 1..]
            .iter()
            .filter_map(|k| match k {
                Key::Char(c) => Some(*c),
                Key::Enter => Some('↵'),
                _ => None,
            })
            .collect();
        let mut items: Vec<(char, &'static str)> = Vec::new();
        for binding in bindings(self.context()) {
            if let Some(rest) = binding.leader.strip_prefix(typed.as_str())
                && let Some(next) = rest.chars().next()
            {
                let label = if rest.chars().count() == 1 {
                    binding.label
                } else {
                    let group = &binding.leader[..typed.len() + next.len_utf8()];
                    GROUPS
                        .iter()
                        .find(|(g, _)| *g == group)
                        .map_or("+more", |(_, name)| *name)
                };
                if !items.iter().any(|(k, _)| *k == next) {
                    items.push((next, label));
                }
            }
        }
        if typed.is_empty() {
            items.push(('1', "1…9  Go to Tab (1: files)"));
        }
        // Enter, Space and the tab digits first, then the letters.
        let rank = |k: char| ['↵', ' ', '1'].iter().position(|&f| f == k).unwrap_or(3);
        items.sort_by_key(|&(k, _)| (rank(k), k));
        Some(items)
    }

    /// Keeps the cursor `SCROLL_OFF` lines away from the edges of a view `height` lines tall.
    pub fn scroll_to_cursor(&mut self, height: usize) {
        self.view_height = height.max(1);
        let line = self.line();
        let lines = text::line_count(&self.doc.rope);
        let off = SCROLL_OFF.min(height.saturating_sub(1) / 2);
        if line < self.top + off {
            self.top = line.saturating_sub(off);
        } else if line + off >= self.top + height {
            self.top = line + off + 1 - height;
        }
        self.top = self.top.min(lines.saturating_sub(1));
    }

    pub fn handle_key(&mut self, key: Key) {
        if !self.replaying {
            self.message = None;
        }
        if self.picker.is_some() {
            self.picker_key(key);
            return;
        }
        // The hover box goes with the next key (Esc only closes it).
        if self.lsp.hover.take().is_some() && key == Key::Esc {
            return;
        }
        if self.mode == Mode::Insert && self.lsp.completion.is_some() && self.completion_key(key) {
            return;
        }
        // IDE habits that work in every mode: Ctrl+Z / Ctrl+Y undo and redo, and in a Markdown
        // file Ctrl+E switches between the reader and editing (Obsidian's key).
        if self.pending.is_empty() && !self.tree_focus {
            match key {
                Key::Ctrl('z') => return self.undo_redo(true),
                Key::Ctrl('y') => return self.undo_redo(false),
                // Alt+J / Alt+K: move the line (or the selected lines) down / up.
                Key::Alt('j') => return self.move_lines(true),
                Key::Alt('k') => return self.move_lines(false),
                Key::Ctrl('e') if self.context() == When::Markdown => {
                    if self.mode != Mode::Normal {
                        self.handle_key(Key::Esc);
                    }
                    return self.run_command(Command::ToggleReader);
                }
                _ => {}
            }
        }
        if self.dashboard.is_some()
            && self.mode == Mode::Normal
            && self.pending.is_empty()
            && self.dashboard_key(key)
        {
            return;
        }
        if self.mode == Mode::Normal && self.pending.is_empty() {
            if let Some(&command) = self.keymap.get(&key) {
                return self.run_command(command);
            }
            match key {
                Key::Ctrl('h') if self.tree.is_some() => {
                    self.tree_focus = true;
                    return;
                }
                Key::Ctrl('l') => {
                    self.tree_focus = false;
                    return;
                }
                // Tab: between the folder tree and the text (opens the tree if needed).
                Key::Tab => {
                    if self.tree.is_some() {
                        self.tree_focus = !self.tree_focus;
                    } else {
                        self.show_tree(true);
                    }
                    return;
                }
                Key::Ctrl('b') => return self.run_command(Command::Explorer),
                // LazyVim: Shift+H / Shift+L go to the previous / next buffer (here: tab).
                Key::Char('H') => return self.requests.push(Request::CycleTab(-1)),
                Key::Char('L') => return self.requests.push(Request::CycleTab(1)),
                Key::Ctrl('f') => return self.run_command(Command::SearchLines),
                Key::F(5) => return self.run_command(Command::Run),
                Key::F(4) | Key::Ctrl('/' | '_' | '7') => {
                    return self.run_command(Command::Terminal);
                }
                // Space (leader) and `:` work from the tree and the reader too.
                Key::Char(' ' | ':') => {}
                _ if self.reader && !self.tree_focus => {
                    self.reader_key(key);
                    return;
                }
                _ if self.tree_focus && self.tree.is_some() => {
                    self.tree_key(key);
                    return;
                }
                _ => {}
            }
        }
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::Insert => self.insert_key(key),
            Mode::Visual | Mode::VisualLine => self.visual_key(key),
            Mode::Command | Mode::Search => self.cmdline_key(key),
        }
    }

    /// Opens `path` (at 0-based `line`) in place of the current document, unless that has
    /// unsaved changes and `force` is off.
    pub fn open(&mut self, path: &Path, line: Option<usize>, force: bool) {
        let same = self.doc.path().is_some_and(|p| p == path);
        if !same || force {
            if self.doc.is_modified() && !force && !self.shared {
                self.message = Some(NOT_SAVED.into());
                return;
            }
            match Document::open(path) {
                Ok(doc) => self.load(doc),
                Err(e) => {
                    self.message = Some(format!("Cannot open \"{}\": {e}", path.display()));
                    return;
                }
            }
        }
        self.dashboard = None;
        if let Some(tree) = &mut self.tree {
            tree.reveal(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
        }
        if let Some(line) = line {
            let line = line.min(text::line_count(&self.doc.rope) - 1);
            self.cursor = text::first_non_blank(&self.doc.rope, line);
            // Show a jumped-to line in the middle, as Vim does for a far jump.
            self.top = line.saturating_sub(self.view_height / 2);
        }
        self.remember(path);
    }

    /// Ctrl+Z / Ctrl+Y. In Insert mode the typing so far becomes its own undo step first, and
    /// typing goes on after; Visual mode ends.
    fn undo_redo(&mut self, undo: bool) {
        let insert = self.mode == Mode::Insert;
        if insert {
            self.doc.end_step(self.cursor);
            self.recording_insert = false;
        } else if self.mode != Mode::Normal {
            self.handle_key(Key::Esc);
        }
        if self.reader {
            self.reader = false;
            self.leave_reader();
        }
        let to = if undo {
            self.doc.undo()
        } else {
            self.doc.redo()
        };
        match to {
            Some(pos) => self.cursor = pos.min(self.doc.rope.len_chars()),
            None => {
                self.message = Some(
                    if undo {
                        "Already at oldest change"
                    } else {
                        "Already at newest change"
                    }
                    .into(),
                );
            }
        }
        if insert {
            self.doc.begin_step(self.cursor);
        } else {
            self.clamp();
        }
    }

    /// Takes the config file's settings (line numbers, indent, keys, `[run]`).
    pub fn apply_config(&mut self, config: &Config) {
        (self.number, self.relative_number) = match config.editor.line_numbers {
            LineNumbers::Absolute => (true, false),
            LineNumbers::Relative => (true, true),
            LineNumbers::Off => (false, false),
        };
        self.indent = config.editor.indent.clamp(1, 16);
        self.auto_pairs = config.editor.auto_pairs;
        self.keymap = config.keymap();
        self.config = config.clone();
    }

    /// Adds `path` to the recent files.
    pub fn remember(&mut self, path: &Path) {
        self.state.remember(path);
        self.requests.push(Request::SaveState);
    }

    /// A path for people: relative to `root` when inside it, `~/...` in the home folder.
    pub fn display_path(&self, path: &Path) -> String {
        let root = std::path::absolute(&self.root).unwrap_or_else(|_| self.root.clone());
        if let Ok(rel) = path.strip_prefix(&root) {
            return rel.display().to_string();
        }
        if let Some(home) = std::env::var_os("HOME")
            && let Ok(rel) = path.strip_prefix(&home)
        {
            return format!("~/{}", rel.display());
        }
        path.display().to_string()
    }

    /// Replaces the document; the cursor and the view start over.
    fn load(&mut self, doc: Document) {
        self.reader = doc.path().map(Lang::from_path) == Some(Lang::Markdown);
        self.reader_top = 0;
        let old = std::mem::replace(&mut self.doc, doc);
        if self.shared {
            self.shared = false;
            self.left = Some(old);
        }
        self.cursor = 0;
        self.anchor = 0;
        self.top = 0;
        self.mode = Mode::Normal;
        self.desired_col = None;
        self.pending.clear();
        self.dashboard = None;
    }

    fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        self.desired_col = None;
    }

    /// In Normal mode the cursor sits on a character, never past the end of a line.
    fn clamp(&mut self) {
        let rope = &self.doc.rope;
        let len = rope.len_chars();
        self.cursor = self.cursor.min(len);
        if matches!(self.mode, Mode::Normal | Mode::Visual | Mode::VisualLine) {
            let line = text::line_of(rope, self.cursor);
            let (start, end) = (text::line_start(rope, line), text::line_end(rope, line));
            if end > start && self.cursor >= end {
                self.cursor = end - 1;
            } else if end == start {
                self.cursor = start;
            }
        }
    }
}

/// A closing bracket or a quote (typed where it already is, the cursor steps over it).
fn closes(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'' | '`')
}

const NOT_SAVED: &str = "No write since last change (:w saves, :e! FILE drops the changes)";

/// Spaces and tabs at the start of `line`.
fn leading_blank(line: &str) -> String {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn key_label(key: &Key) -> String {
    match key {
        Key::Char(' ') => "␣".into(),
        Key::Char(c) => c.to_string(),
        Key::Ctrl(c) => format!("^{}", c.to_ascii_uppercase()),
        Key::Esc => "<Esc>".into(),
        other => format!("<{other:?}>"),
    }
}

#[cfg(test)]
mod tests;

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

use crate::command::{
    COMMANDS, Command, DASHBOARD, GROUPS, VIM_KEYS, When, bindings, leader_label,
};
use crate::config::{Config, LineNumbers};
use crate::document::Document;
use crate::picker::{Item, Kind, Pick, Picker};
use crate::state::State;
use crate::syntax::Lang;
use crate::text;
use crate::tree::{self, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Ctrl(char),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordStart(bool),
    WordBack(bool),
    WordEnd(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    FileStart,
    FileEnd,
    Find {
        target: char,
        forward: bool,
        till: bool,
    },
    RepeatFind {
        reverse: bool,
    },
    ParagraphForward,
    ParagraphBack,
    MatchBracket,
    SearchNext {
        reverse: bool,
    },
}

impl Motion {
    fn vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down)
    }

    fn linewise(self) -> bool {
        matches!(
            self,
            Self::Up | Self::Down | Self::FileStart | Self::FileEnd
        )
    }

    /// The character under the target belongs to the range (`e`, `$`, `f`, `t`, `%`).
    fn inclusive(self) -> bool {
        matches!(
            self,
            Self::WordEnd(_)
                | Self::LineEnd
                | Self::MatchBracket
                | Self::Find { forward: true, .. }
        ) || matches!(self, Self::RepeatFind { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Object {
    Word { big: bool },
    Quote(char),
    Brackets(char, char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Motion(Motion),
    Object {
        object: Object,
        around: bool,
    },
    /// `dd`, `cc`, `yy`, `>>`: whole lines.
    Lines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Insert,
    Append,
    InsertLineStart,
    AppendLineEnd,
    OpenBelow,
    OpenAbove,
    PutAfter,
    PutBefore,
    Join,
    Replace(char),
    ToggleCase,
    Undo,
    Redo,
    Repeat,
    Visual,
    VisualLine,
    CommandLine,
    Search { forward: bool },
    SearchWord,
    HalfPageDown,
    HalfPageUp,
    Save,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Move(Motion),
    Operate(Operator, Target),
    Act(Action),
    Leader(Command),
    /// `Space 1`…`Space 9`: a tab (the app's; 1 is the file manager).
    GoTab(usize),
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    /// More keys are needed (`d`, `g`, `f`, `Space q`).
    Incomplete,
    Invalid,
    /// Count (if one was typed) and the command.
    Done(Option<usize>, Cmd),
}

/// Lines kept visible above and below the cursor (LazyVim: `scrolloff = 4`).
const SCROLL_OFF: usize = 4;
/// Work the editor asks the app to do (it has the threads and the disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Show tab `n` (0-based; `Space 1` is 0, the file manager).
    SwitchTab(usize),
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
    /// Markdown shown formatted (`Space u m`); Markdown files open this way. `reader_top`:
    /// the first source line on screen.
    pub reader: bool,
    pub reader_top: usize,
    /// Spaces per indent level (`[editor] indent`; LazyVim: `shiftwidth = 2`).
    pub indent: usize,
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
        // IDE habits that work in every mode: Ctrl+Z / Ctrl+Y undo and redo, and in a Markdown
        // file Ctrl+E switches between the reader and editing (Obsidian's key).
        if self.pending.is_empty() && !self.tree_focus {
            match key {
                Key::Ctrl('z') => return self.undo_redo(true),
                Key::Ctrl('y') => return self.undo_redo(false),
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

    // ---- Normal mode ----

    fn normal_key(&mut self, key: Key) {
        if self.pending.is_empty() {
            self.recording.clear();
            self.version_at_start = self.doc.version();
            self.doc.begin_step(self.cursor);
        }
        self.pending.push(key);
        self.recording.push(key);
        let (count, cmd) = match parse(&self.pending, self.context()) {
            Parsed::Incomplete => return,
            Parsed::Invalid => {
                self.pending.clear();
                self.doc.end_step(self.cursor);
                return;
            }
            Parsed::Done(count, cmd) => (count, cmd),
        };
        self.pending.clear();
        self.run(count, cmd);
        if self.mode == Mode::Insert {
            self.recording_insert = !self.replaying;
            return; // the step and the recording go on until Esc
        }
        self.doc.end_step(self.cursor);
        let records = !matches!(cmd, Cmd::Act(Action::Undo | Action::Redo | Action::Repeat));
        if records && !self.replaying && self.doc.version() != self.version_at_start {
            self.last_change = self.recording.clone();
        }
        self.clamp();
    }

    fn run(&mut self, count: Option<usize>, cmd: Cmd) {
        let n = count.unwrap_or(1);
        match cmd {
            Cmd::Move(motion) => {
                if let Some(to) = self.motion_target(motion, n, count, false) {
                    self.cursor = to;
                    if !motion.vertical() {
                        self.desired_col = None;
                    }
                }
            }
            Cmd::Operate(op, target) => self.operate(op, target, n, count),
            Cmd::Act(action) => self.act(action, n),
            Cmd::Leader(command) => self.run_command(command),
            Cmd::GoTab(n) => self.requests.push(Request::SwitchTab(n - 1)),
        }
    }

    /// Where `motion` goes from the cursor, `n` times. `for_operator`: `l` may reach the end
    /// of the line (so `dl` on the last character deletes it).
    fn motion_target(
        &mut self,
        motion: Motion,
        n: usize,
        count: Option<usize>,
        for_operator: bool,
    ) -> Option<usize> {
        let rope = &self.doc.rope;
        let pos = self.cursor;
        let line = text::line_of(rope, pos);
        let lines = text::line_count(rope);
        Some(match motion {
            Motion::Left => pos.saturating_sub(n).max(text::line_start(rope, line)),
            Motion::Right => {
                let end = text::line_end(rope, line);
                let max = if for_operator || end == text::line_start(rope, line) {
                    end
                } else {
                    end - 1
                };
                (pos + n).min(max)
            }
            Motion::Up | Motion::Down => {
                let col = *self.desired_col.get_or_insert(text::col_of(rope, pos));
                let target = if motion == Motion::Up {
                    line.saturating_sub(n)
                } else {
                    (line + n).min(lines - 1)
                };
                text::at_col(rope, target, col, false)
            }
            Motion::WordStart(big) => (0..n).fold(pos, |p, _| text::next_word_start(rope, p, big)),
            Motion::WordBack(big) => (0..n).fold(pos, |p, _| text::prev_word_start(rope, p, big)),
            Motion::WordEnd(big) => (0..n).fold(pos, |p, _| text::word_end(rope, p, big)),
            Motion::LineStart => text::line_start(rope, line),
            Motion::FirstNonBlank => text::first_non_blank(rope, line),
            Motion::LineEnd => {
                let target = (line + n - 1).min(lines - 1);
                text::line_end(rope, target)
                    .saturating_sub(1)
                    .max(text::line_start(rope, target))
            }
            Motion::FileStart | Motion::FileEnd => {
                let target = match (motion, count) {
                    (_, Some(c)) => c.saturating_sub(1).min(lines - 1),
                    (Motion::FileStart, None) => 0,
                    _ => lines - 1,
                };
                text::first_non_blank(rope, target)
            }
            Motion::Find {
                target,
                forward,
                till,
            } => {
                self.last_find = Some(motion);
                text::find_in_line(rope, pos, target, n, forward, till)?
            }
            Motion::RepeatFind { reverse } => {
                let Some(Motion::Find {
                    target,
                    forward,
                    till,
                }) = self.last_find
                else {
                    return None;
                };
                text::find_in_line(rope, pos, target, n, forward != reverse, till)?
            }
            Motion::ParagraphForward => (0..n).fold(pos, |p, _| text::paragraph(rope, p, true)),
            Motion::ParagraphBack => (0..n).fold(pos, |p, _| text::paragraph(rope, p, false)),
            Motion::MatchBracket => text::matching_bracket(rope, pos)?,
            Motion::SearchNext { reverse } => {
                let pattern = self.last_search.clone()?;
                self.highlight_search = true;
                let forward = self.search_forward != reverse;
                let found = (0..n).try_fold(pos, |p, _| text::search(rope, &pattern, p, forward));
                if found.is_none() {
                    self.message = Some(format!("Pattern not found: {pattern}"));
                }
                found?
            }
        })
    }

    /// The range an operator works on: `from..to` and whether it is whole lines.
    fn operator_range(
        &mut self,
        op: Operator,
        target: Target,
        n: usize,
        count: Option<usize>,
    ) -> Option<(usize, usize, bool)> {
        let line = self.line();
        match target {
            Target::Lines => {
                let last = (line + n - 1).min(text::line_count(&self.doc.rope) - 1);
                Some(self.line_range(line, last, true))
            }
            Target::Object { object, around } => {
                let rope = &self.doc.rope;
                let range = match object {
                    Object::Word { big } => text::word_object(rope, self.cursor, big, around),
                    Object::Quote(q) => text::quote_object(rope, self.cursor, q, around),
                    Object::Brackets(o, c) => text::bracket_object(rope, self.cursor, o, c, around),
                }?;
                Some((range.0, range.1, false))
            }
            Target::Motion(motion) => {
                // `cw` changes to the end of the word, like `ce` (Vim's special case).
                let rope = &self.doc.rope;
                let on_word =
                    self.cursor < rope.len_chars() && !rope.char(self.cursor).is_whitespace();
                let motion = match motion {
                    Motion::WordStart(big) if op == Operator::Change && on_word => {
                        Motion::WordEnd(big)
                    }
                    m => m,
                };
                let to = self.motion_target(motion, n, count, true)?;
                let rope = &self.doc.rope;
                if motion.linewise() {
                    let (a, b) = (
                        line.min(text::line_of(rope, to)),
                        line.max(text::line_of(rope, to)),
                    );
                    return Some(self.line_range(a, b, true));
                }
                let (mut from, mut to) = (self.cursor.min(to), self.cursor.max(to));
                if motion.inclusive() {
                    to = (to + 1).min(rope.len_chars());
                }
                // `dw` on the last word of a line stops at the line end, not on the next line.
                if matches!(motion, Motion::WordStart(_)) && text::line_of(rope, to) > line {
                    to = to.min(text::line_end(rope, line)).max(from);
                    if to == from {
                        to = text::line_end(rope, line);
                    }
                }
                from = from.min(to);
                Some((from, to, false))
            }
        }
    }

    /// Lines `first..=last` as a char range for deleting / yanking whole lines.
    fn line_range(&self, first: usize, last: usize, linewise: bool) -> (usize, usize, bool) {
        let rope = &self.doc.rope;
        let from = text::line_start(rope, first);
        let to = if last + 1 < text::line_count(rope) {
            text::line_start(rope, last + 1)
        } else {
            rope.len_chars()
        };
        (from, to, linewise)
    }

    fn operate(&mut self, op: Operator, target: Target, n: usize, count: Option<usize>) {
        let Some((from, to, linewise)) = self.operator_range(op, target, n, count) else {
            return;
        };
        self.apply_operator(op, from, to, linewise);
    }

    fn apply_operator(&mut self, op: Operator, from: usize, to: usize, linewise: bool) {
        let rope = &self.doc.rope;
        let mut yanked = rope.slice(from..to).to_string();
        if linewise && !yanked.ends_with('\n') {
            yanked.push('\n');
        }
        let first_line = text::line_of(rope, from);
        let last_line = text::line_of(rope, to.saturating_sub(1).max(from));
        match op {
            Operator::Yank => {
                self.register = Register {
                    text: yanked,
                    linewise,
                };
                self.cursor = if linewise {
                    text::at_col(rope, first_line, self.col(), false).min(self.cursor.max(from))
                } else {
                    from
                };
            }
            Operator::Delete => {
                self.register = Register {
                    text: yanked,
                    linewise,
                };
                self.doc.edit(from, to, "");
                self.cursor = if linewise {
                    let line = text::line_of(&self.doc.rope, from);
                    text::first_non_blank(&self.doc.rope, line)
                } else {
                    from
                };
            }
            Operator::Change => {
                self.register = Register {
                    text: yanked,
                    linewise,
                };
                if linewise {
                    // Keep the line (and its indentation), replace what is on it.
                    let indent = leading_blank(&text::line_text(rope, first_line));
                    let from = text::line_start(rope, first_line);
                    let to = text::line_end(rope, last_line);
                    self.doc.edit(from, to, &indent);
                    self.cursor = from + indent.chars().count();
                } else {
                    self.doc.edit(from, to, "");
                    self.cursor = from;
                }
                self.enter_insert();
            }
            Operator::Indent | Operator::Outdent => {
                for line in first_line..=last_line {
                    let start = text::line_start(&self.doc.rope, line);
                    let content = text::line_text(&self.doc.rope, line);
                    if op == Operator::Indent {
                        if !content.is_empty() {
                            self.doc.edit(start, start, &" ".repeat(self.indent));
                        }
                    } else {
                        let blank = content
                            .chars()
                            .take_while(|c| *c == ' ')
                            .count()
                            .min(self.indent);
                        let tab = usize::from(blank == 0 && content.starts_with('\t'));
                        self.doc.edit(start, start + blank + tab, "");
                    }
                }
                self.cursor = text::first_non_blank(&self.doc.rope, first_line);
            }
        }
    }

    fn act(&mut self, action: Action, n: usize) {
        let line = self.line();
        match action {
            Action::Insert => self.enter_insert(),
            Action::Append => {
                if text::line_len(&self.doc.rope, line) > 0 {
                    self.cursor += 1;
                }
                self.enter_insert();
            }
            Action::InsertLineStart => {
                self.cursor = text::first_non_blank(&self.doc.rope, line);
                self.enter_insert();
            }
            Action::AppendLineEnd => {
                self.cursor = text::line_end(&self.doc.rope, line);
                self.enter_insert();
            }
            Action::OpenBelow | Action::OpenAbove => {
                let indent = leading_blank(&text::line_text(&self.doc.rope, line));
                if action == Action::OpenBelow {
                    let at = text::line_end(&self.doc.rope, line);
                    self.doc.edit(at, at, &format!("\n{indent}"));
                    self.cursor = at + 1 + indent.chars().count();
                } else {
                    let at = text::line_start(&self.doc.rope, line);
                    self.doc.edit(at, at, &format!("{indent}\n"));
                    self.cursor = at + indent.chars().count();
                }
                self.enter_insert();
            }
            Action::PutAfter | Action::PutBefore => self.put(action == Action::PutAfter, n),
            Action::Join => {
                for _ in 0..n.max(2) - 1 {
                    let rope = &self.doc.rope;
                    let line = self.line();
                    if line + 1 >= text::line_count(rope) {
                        break;
                    }
                    let end = text::line_end(rope, line);
                    let next = text::first_non_blank(rope, line + 1);
                    let next_empty = next == text::line_end(rope, line + 1);
                    let ends_blank =
                        end > text::line_start(rope, line) && rope.char(end - 1) == ' ';
                    let space = if next_empty || ends_blank || rope.char(next) == ')' {
                        ""
                    } else {
                        " "
                    };
                    self.doc.edit(end, next, space);
                    self.cursor = end;
                }
            }
            Action::Replace(c) => {
                let len = text::line_len(&self.doc.rope, line);
                if self.col() + n <= len {
                    let with: String = std::iter::repeat_n(c, n).collect();
                    self.doc.edit(self.cursor, self.cursor + n, &with);
                    self.cursor += n - 1;
                }
            }
            Action::ToggleCase => {
                let end = text::line_end(&self.doc.rope, line);
                let to = (self.cursor + n).min(end);
                let flipped: String = self
                    .doc
                    .rope
                    .slice(self.cursor..to)
                    .chars()
                    .flat_map(|c| -> Vec<char> {
                        if c.is_uppercase() {
                            c.to_lowercase().collect()
                        } else {
                            c.to_uppercase().collect()
                        }
                    })
                    .collect();
                self.doc.edit(self.cursor, to, &flipped);
                self.cursor = to;
            }
            Action::Undo | Action::Redo => {
                for _ in 0..n {
                    let to = if action == Action::Undo {
                        self.doc.undo()
                    } else {
                        self.doc.redo()
                    };
                    match to {
                        Some(pos) => self.cursor = pos.min(self.doc.rope.len_chars()),
                        None => {
                            self.message = Some(
                                if action == Action::Undo {
                                    "Already at oldest change"
                                } else {
                                    "Already at newest change"
                                }
                                .into(),
                            );
                            break;
                        }
                    }
                }
            }
            Action::Repeat => {
                let keys = self.last_change.clone();
                self.replaying = true;
                for _ in 0..n {
                    for key in &keys {
                        self.handle_key(*key);
                    }
                }
                self.replaying = false;
            }
            Action::Visual | Action::VisualLine => {
                self.anchor = self.cursor;
                self.mode = if action == Action::Visual {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
            }
            Action::CommandLine => {
                self.cmdline.clear();
                self.mode = Mode::Command;
            }
            Action::Search { forward } => {
                self.cmdline.clear();
                self.search_forward = forward;
                self.mode = Mode::Search;
            }
            Action::SearchWord => {
                if let Some((from, to)) =
                    text::word_object(&self.doc.rope, self.cursor, false, false)
                {
                    self.last_search = Some(self.doc.rope.slice(from..to).to_string());
                    self.search_forward = true;
                    if let Some(to) =
                        self.motion_target(Motion::SearchNext { reverse: false }, n, None, false)
                    {
                        self.cursor = to;
                    }
                }
            }
            Action::HalfPageDown | Action::HalfPageUp => {
                let half = (self.view_height / 2).max(1);
                let lines = text::line_count(&self.doc.rope);
                let target = if action == Action::HalfPageDown {
                    self.top = (self.top + half).min(lines.saturating_sub(1));
                    (line + half).min(lines - 1)
                } else {
                    self.top = self.top.saturating_sub(half);
                    line.saturating_sub(half)
                };
                let col = *self.desired_col.get_or_insert(self.col());
                self.cursor = text::at_col(&self.doc.rope, target, col, false);
            }
            Action::Save => self.save(None),
            Action::Center => {
                self.top = line.saturating_sub(self.view_height / 2);
            }
        }
    }

    fn put(&mut self, after: bool, n: usize) {
        let reg = self.register.clone();
        if reg.text.is_empty() {
            return;
        }
        let text: String = std::iter::repeat_n(reg.text.as_str(), n).collect();
        let rope = &self.doc.rope;
        let line = self.line();
        if reg.linewise {
            let (at, insert) = if after {
                if line + 1 < text::line_count(rope) {
                    (text::line_start(rope, line + 1), text)
                } else {
                    // After the last line: it may have no line break of its own.
                    let end = rope.len_chars();
                    if end > 0 && rope.char(end - 1) == '\n' {
                        (end, text)
                    } else {
                        (end, format!("\n{}", text.trim_end_matches('\n')))
                    }
                }
            } else {
                (text::line_start(rope, line), text)
            };
            let leading = usize::from(insert.starts_with('\n') && !reg.text.starts_with('\n'));
            self.doc.edit(at, at, &insert);
            let first = text::line_of(&self.doc.rope, at + leading);
            self.cursor = text::first_non_blank(&self.doc.rope, first);
        } else {
            let at = if after && text::line_len(rope, line) > 0 {
                self.cursor + 1
            } else {
                self.cursor
            };
            let len = text.chars().count();
            self.doc.edit(at, at, &text);
            self.cursor = at + len - 1;
        }
    }

    pub fn run_command(&mut self, command: Command) {
        match command {
            Command::FindFiles => {
                let mut p = Picker::new(Kind::Files, "Find Files", Vec::new());
                p.loading = true;
                self.picker = Some(p);
                self.requests.push(Request::ListFiles);
            }
            Command::RecentFiles => {
                let items = self
                    .state
                    .recent
                    .iter()
                    .filter(|p| p.exists())
                    .map(|path| Item {
                        text: self.display_path(path),
                        detail: String::new(),
                        pick: Pick::File {
                            path: path.clone(),
                            line: None,
                        },
                    })
                    .collect();
                self.picker = Some(Picker::new(Kind::Recent, "Recent Files", items));
            }
            Command::FindText => self.picker = Some(Picker::new(Kind::Grep, "Grep", Vec::new())),
            Command::NewFile => {
                if self.doc.is_modified() {
                    self.message = Some(NOT_SAVED.into());
                } else {
                    self.load(Document::new(""));
                    self.enter_insert();
                }
            }
            Command::Keymaps => {
                // The config file's keys first, so a changed key is easy to see.
                let mut user: Vec<Item> = self
                    .config
                    .keys
                    .iter()
                    .filter_map(|(key, name)| {
                        let command = Command::from_name(name)?;
                        let label = COMMANDS.iter().find(|b| b.command == command)?.label;
                        Some(Item {
                            text: format!("{label} (config)"),
                            detail: key.clone(),
                            pick: Pick::Command(command),
                        })
                    })
                    .collect();
                user.sort_by(|a, b| a.detail.cmp(&b.detail));
                let commands = bindings(self.context()).map(|b| Item {
                    text: b.label.into(),
                    detail: if b.leader.is_empty() {
                        String::new()
                    } else {
                        leader_label(b.leader)
                    },
                    pick: Pick::Command(b.command),
                });
                let vim = VIM_KEYS.iter().map(|(keys, label)| Item {
                    text: (*label).into(),
                    detail: (*keys).into(),
                    pick: Pick::Info,
                });
                let items = user.into_iter().chain(commands).chain(vim).collect();
                self.picker = Some(Picker::new(Kind::Keymaps, "Keymaps", items));
            }
            Command::Themes => {
                let items = self
                    .themes
                    .iter()
                    .enumerate()
                    .map(|(i, name)| Item {
                        text: (*name).into(),
                        detail: String::new(),
                        pick: Pick::Theme(i),
                    })
                    .collect();
                let mut p = Picker::new(Kind::Themes, "Colorschemes", items);
                p.selected = self.theme.min(p.matches.len().saturating_sub(1));
                p.theme_before = self.theme;
                self.picker = Some(p);
            }
            Command::Explorer => {
                if self.tree.is_some() {
                    self.tree = None;
                    self.tree_focus = false;
                } else {
                    self.show_tree(true);
                }
            }
            Command::SearchLines => {
                let path = self.doc.path().map(Path::to_path_buf);
                let items = (0..text::line_count(&self.doc.rope))
                    .map(|line| Item {
                        text: text::line_text(&self.doc.rope, line),
                        detail: (line + 1).to_string(),
                        pick: match &path {
                            Some(p) => Pick::File {
                                path: p.clone(),
                                line: Some(line),
                            },
                            None => Pick::Line(line),
                        },
                    })
                    .collect();
                self.picker = Some(Picker::new(Kind::Lines, "Find in File", items));
            }
            Command::Headings => {
                let path = self.doc.path().map(Path::to_path_buf);
                let mut fence = false;
                let mut items = Vec::new();
                for line in 0..text::line_count(&self.doc.rope) {
                    let content = text::line_text(&self.doc.rope, line);
                    let trimmed = content.trim_start();
                    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                        fence = !fence;
                    } else if !fence && trimmed.starts_with('#') {
                        let level = trimmed.chars().take_while(|&c| c == '#').count();
                        items.push(Item {
                            // Indented by level, without the #s: an outline.
                            text: format!(
                                "{}{}",
                                "  ".repeat(level.saturating_sub(1)),
                                trimmed[level..].trim()
                            ),
                            detail: (line + 1).to_string(),
                            pick: match &path {
                                Some(p) => Pick::File {
                                    path: p.clone(),
                                    line: Some(line),
                                },
                                None => Pick::Line(line),
                            },
                        });
                    }
                }
                self.picker = Some(Picker::new(Kind::Lines, "Headings", items));
            }
            Command::Run => self.run_file(crate::run::Goal::Run),
            Command::Build => self.run_file(crate::run::Goal::Build),
            Command::Terminal => self.requests.push(Request::ToggleTerminal),
            Command::Dashboard => self.dashboard = Some(0),
            Command::Save => self.save(None),
            Command::Quit => self.ex("qa"),
            Command::ToggleReader => {
                self.reader = !self.reader;
                if self.reader {
                    self.reader_top = self.top;
                } else {
                    self.leave_reader();
                }
            }
            Command::ToggleNumbers => self.number = !self.number,
            Command::ToggleRelativeNumbers => self.relative_number = !self.relative_number,
            Command::ClearSearch => self.highlight_search = false,
        }
    }

    /// Opens `path` (at 0-based `line`) in place of the current document, unless that has
    /// unsaved changes and `force` is off.
    pub fn open(&mut self, path: &Path, line: Option<usize>, force: bool) {
        let same = self.doc.path().is_some_and(|p| p == path);
        if !same || force {
            if self.doc.is_modified() && !force {
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

    /// F5 / `Space Enter` / `Space b`: saves, then asks the app to type the file's run (or
    /// build) command in the terminal. The config file's `[run]` / `[build]` comes first.
    fn run_file(&mut self, goal: crate::run::Goal) {
        let Some(path) = self.doc.path().map(Path::to_path_buf) else {
            self.message = Some("Save the file first (:w name)".into());
            return;
        };
        let file = std::path::absolute(&path).unwrap_or(path);
        let configured = self
            .config
            .run_command(&file, goal)
            .map(|command| crate::run::Run {
                dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
                command,
            });
        match configured.or_else(|| crate::run::command_for(&file, goal)) {
            Some(run) => {
                if self.doc.is_modified() {
                    self.save(None);
                }
                self.requests.push(Request::Run(run));
            }
            None => self.message = Some("No run command for this kind of file".into()),
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
        self.keymap = config.keymap();
        self.config = config.clone();
    }

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
    fn tree_key(&mut self, key: Key) {
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
    fn reader_key(&mut self, key: Key) {
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
            _ => self.message = Some("Reader: j k scroll, i or Esc to edit, Space u m".into()),
        }
        self.reader_top = self.reader_top.min(last);
    }

    /// Back to the source at the place the reader was showing.
    fn leave_reader(&mut self) {
        let line = self
            .reader_top
            .min(text::line_count(&self.doc.rope).saturating_sub(1));
        self.top = line;
        self.cursor = text::first_non_blank(&self.doc.rope, line);
    }

    /// Adds `path` to the recent files.
    pub fn remember(&mut self, path: &Path) {
        self.state.remember(path);
        self.requests.push(Request::SaveState);
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
        self.doc = doc;
        self.cursor = 0;
        self.anchor = 0;
        self.top = 0;
        self.mode = Mode::Normal;
        self.desired_col = None;
        self.pending.clear();
        self.dashboard = None;
    }

    /// A key on the start screen; `false` when it is not the menu's (Space, `:` go on as usual).
    fn dashboard_key(&mut self, key: Key) -> bool {
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

    fn picker_key(&mut self, key: Key) {
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

    // ---- Insert mode ----

    fn insert_key(&mut self, key: Key) {
        if self.recording_insert {
            self.recording.push(key);
        }
        let rope = &self.doc.rope;
        let line = text::line_of(rope, self.cursor);
        match key {
            Key::Esc => {
                if self.cursor > text::line_start(rope, line) {
                    self.cursor -= 1;
                }
                self.mode = Mode::Normal;
                self.doc.end_step(self.cursor);
                if self.recording_insert {
                    self.last_change = self.recording.clone();
                    self.recording_insert = false;
                }
                self.clamp();
            }
            Key::Char(c) => {
                self.doc
                    .edit(self.cursor, self.cursor, c.encode_utf8(&mut [0; 4]));
                self.cursor += 1;
            }
            Key::Tab => {
                self.doc
                    .edit(self.cursor, self.cursor, &" ".repeat(self.indent));
                self.cursor += self.indent;
            }
            Key::Enter => {
                // New line with the indentation of this one (autoindent).
                let indent = leading_blank(&text::line_text(rope, line));
                let keep = indent
                    .chars()
                    .count()
                    .min(self.cursor - text::line_start(rope, line));
                let indent: String = indent.chars().take(keep).collect();
                self.doc
                    .edit(self.cursor, self.cursor, &format!("\n{indent}"));
                self.cursor += 1 + indent.chars().count();
            }
            Key::Backspace if self.cursor > 0 => {
                self.doc.edit(self.cursor - 1, self.cursor, "");
                self.cursor -= 1;
            }
            Key::Delete if self.cursor < rope.len_chars() => {
                self.doc.edit(self.cursor, self.cursor + 1, "");
            }
            Key::Ctrl('w') => {
                let start = text::line_start(rope, line);
                let to = text::prev_word_start(rope, self.cursor, false).max(start);
                self.doc.edit(to, self.cursor, "");
                self.cursor = to;
            }
            Key::Ctrl('s') => self.save(None),
            Key::Left => {
                self.cursor = self
                    .cursor
                    .saturating_sub(1)
                    .max(text::line_start(rope, line))
            }
            Key::Right => self.cursor = (self.cursor + 1).min(text::line_end(rope, line)),
            Key::Up | Key::Down => {
                let col = text::col_of(rope, self.cursor);
                let target = if key == Key::Up {
                    line.saturating_sub(1)
                } else {
                    (line + 1).min(text::line_count(rope) - 1)
                };
                self.cursor = text::at_col(rope, target, col, true);
            }
            Key::Home => self.cursor = text::line_start(rope, line),
            Key::End => self.cursor = text::line_end(rope, line),
            _ => {}
        }
    }

    // ---- Visual mode ----

    fn visual_key(&mut self, key: Key) {
        self.pending.push(key);
        let keys = self.pending.clone();
        let (count, rest) = parse_count(&keys);
        let Some(&first) = rest.first() else {
            return;
        };
        let op = match first {
            Key::Char('d' | 'x') | Key::Delete => Some(Operator::Delete),
            Key::Char('y') => Some(Operator::Yank),
            Key::Char('c' | 's') => Some(Operator::Change),
            Key::Char('>') => Some(Operator::Indent),
            Key::Char('<') => Some(Operator::Outdent),
            _ => None,
        };
        if let Some(op) = op {
            self.pending.clear();
            let (from, to) = self.selection().expect("in visual mode");
            let linewise = self.mode == Mode::VisualLine;
            let (from, to, linewise) = if linewise {
                let rope = &self.doc.rope;
                self.line_range(
                    text::line_of(rope, from),
                    text::line_of(rope, to.saturating_sub(1).max(from)),
                    true,
                )
            } else {
                (from, to, false)
            };
            self.mode = Mode::Normal;
            self.doc.begin_step(self.cursor);
            self.apply_operator(op, from, to, linewise);
            if self.mode != Mode::Insert {
                self.doc.end_step(self.cursor);
                self.clamp();
            }
            return;
        }
        match first {
            Key::Esc => {
                self.pending.clear();
                self.mode = Mode::Normal;
                self.clamp();
            }
            Key::Char('v') | Key::Char('V') => {
                self.pending.clear();
                let wanted = if first == Key::Char('v') {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
                self.mode = if self.mode == wanted {
                    Mode::Normal
                } else {
                    wanted
                };
                self.clamp();
            }
            Key::Char('o') => {
                self.pending.clear();
                std::mem::swap(&mut self.anchor, &mut self.cursor);
            }
            Key::Char('i' | 'a') => match rest.get(1) {
                None => {}
                Some(&k) => {
                    self.pending.clear();
                    if let Some(object) = object_of(k) {
                        let around = first == Key::Char('a');
                        let rope = &self.doc.rope;
                        let range = match object {
                            Object::Word { big } => {
                                text::word_object(rope, self.cursor, big, around)
                            }
                            Object::Quote(q) => text::quote_object(rope, self.cursor, q, around),
                            Object::Brackets(o, c) => {
                                text::bracket_object(rope, self.cursor, o, c, around)
                            }
                        };
                        if let Some((from, to)) = range {
                            self.anchor = from;
                            self.cursor = to.saturating_sub(1).max(from);
                        }
                    }
                }
            },
            _ => match parse_motion(rest) {
                Parsed::Incomplete => {}
                Parsed::Invalid => self.pending.clear(),
                Parsed::Done(_, Cmd::Move(motion)) => {
                    self.pending.clear();
                    let n = count.unwrap_or(1);
                    if let Some(to) = self.motion_target(motion, n, count, false) {
                        self.cursor = to;
                        if !motion.vertical() {
                            self.desired_col = None;
                        }
                    }
                }
                Parsed::Done(..) => self.pending.clear(),
            },
        }
    }

    // ---- `:` and `/` lines ----

    fn cmdline_key(&mut self, key: Key) {
        match key {
            Key::Esc => self.mode = Mode::Normal,
            Key::Backspace if self.cmdline.is_empty() => self.mode = Mode::Normal,
            Key::Backspace => {
                self.cmdline.pop();
            }
            Key::Char(c) => self.cmdline.push(c),
            Key::Enter => {
                let line = std::mem::take(&mut self.cmdline);
                let search = self.mode == Mode::Search;
                self.mode = Mode::Normal;
                if search {
                    if !line.is_empty() {
                        self.last_search = Some(line);
                    }
                    if let Some(to) =
                        self.motion_target(Motion::SearchNext { reverse: false }, 1, None, false)
                    {
                        self.cursor = to;
                    }
                } else {
                    self.ex(line.trim());
                }
                self.clamp();
            }
            _ => {}
        }
    }

    /// Runs an Ex command (what follows `:`).
    fn ex(&mut self, cmd: &str) {
        let (name, arg) = cmd
            .split_once(' ')
            .map_or((cmd, ""), |(n, a)| (n, a.trim()));
        let modified = self.doc.is_modified();
        match name {
            "w" | "write" => self.save((!arg.is_empty()).then_some(arg)),
            "q" | "quit" | "qa" | "qall" if modified => {
                self.message = Some("No write since last change (add ! to override)".into());
            }
            "q" | "quit" | "qa" | "qall" | "q!" | "quit!" | "qa!" | "qall!" => self.quit = true,
            "wq" | "wqa" | "x" | "xa" | "xit" => {
                if name.starts_with('x') && !modified {
                    self.quit = true;
                } else {
                    self.save((!arg.is_empty()).then_some(arg));
                    self.quit = !self.doc.is_modified();
                }
            }
            "e" | "edit" | "e!" | "edit!" => {
                let force = name.ends_with('!');
                match (arg, self.doc.path().map(Path::to_path_buf)) {
                    ("", Some(current)) => self.open(&current, Some(self.line()), force),
                    ("", None) => self.message = Some("No file name".into()),
                    (file, _) => self.open(Path::new(file), None, force),
                }
            }
            "Dashboard" => self.run_command(Command::Dashboard),
            "noh" | "nohlsearch" => self.highlight_search = false,
            "set" => match arg {
                "nu" | "number" => self.number = true,
                "nonu" | "nonumber" => self.number = false,
                "rnu" | "relativenumber" => self.relative_number = true,
                "nornu" | "norelativenumber" => self.relative_number = false,
                _ => self.message = Some(format!("Unknown option: {arg}")),
            },
            "" => {}
            n if n.chars().all(|c| c.is_ascii_digit()) => {
                let line = n.parse::<usize>().unwrap_or(1).saturating_sub(1);
                let line = line.min(text::line_count(&self.doc.rope) - 1);
                self.cursor = text::first_non_blank(&self.doc.rope, line);
            }
            _ => self.message = Some(format!("Not an editor command: {cmd}")),
        }
    }

    fn save(&mut self, as_path: Option<&str>) {
        if let Some(path) = as_path {
            self.doc.set_path(path.into());
        }
        let name = self
            .doc
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.message = Some(match self.doc.save() {
            Ok(()) => format!("\"{name}\" {}L written", text::line_count(&self.doc.rope)),
            Err(e) => format!("Cannot write \"{name}\": {e}"),
        });
    }
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

// ---- the Normal-mode command language ----

/// A count before a command: digits, not starting with 0 (`0` is a motion).
fn parse_count(keys: &[Key]) -> (Option<usize>, &[Key]) {
    let digits = keys
        .iter()
        .enumerate()
        .take_while(
            |(i, k)| matches!(k, Key::Char(c) if c.is_ascii_digit() && (*i > 0 || *c != '0')),
        )
        .count();
    if digits == 0 {
        return (None, keys);
    }
    let number: String = keys[..digits]
        .iter()
        .filter_map(|k| match k {
            Key::Char(c) => Some(*c),
            _ => None,
        })
        .collect();
    (number.parse().ok(), &keys[digits..])
}

fn parse(keys: &[Key], context: When) -> Parsed {
    let (count, rest) = parse_count(keys);
    let Some(&first) = rest.first() else {
        return Parsed::Incomplete;
    };
    if first == Key::Char(' ') {
        return parse_leader(&rest[1..], context);
    }
    if let Some(op) = operator_of(first) {
        let (count2, rest) = parse_count(&rest[1..]);
        let count = match (count, count2) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1) * b.unwrap_or(1)),
        };
        let Some(&next) = rest.first() else {
            return Parsed::Incomplete;
        };
        if next == first {
            return Parsed::Done(count, Cmd::Operate(op, Target::Lines));
        }
        if let Key::Char(around @ ('i' | 'a')) = next {
            return match rest.get(1) {
                None => Parsed::Incomplete,
                Some(&k) => object_of(k).map_or(Parsed::Invalid, |object| {
                    Parsed::Done(
                        count,
                        Cmd::Operate(
                            op,
                            Target::Object {
                                object,
                                around: around == 'a',
                            },
                        ),
                    )
                }),
            };
        }
        return match parse_motion(rest) {
            Parsed::Done(_, Cmd::Move(m)) => {
                Parsed::Done(count, Cmd::Operate(op, Target::Motion(m)))
            }
            other => other,
        };
    }
    match parse_motion(rest) {
        Parsed::Done(_, cmd) => Parsed::Done(count, cmd),
        Parsed::Incomplete => Parsed::Incomplete,
        Parsed::Invalid => parse_action(rest).map_count(count),
    }
}

impl Parsed {
    fn map_count(self, count: Option<usize>) -> Self {
        match self {
            Self::Done(_, cmd) => Self::Done(count, cmd),
            other => other,
        }
    }
}

fn operator_of(key: Key) -> Option<Operator> {
    Some(match key {
        Key::Char('d') => Operator::Delete,
        Key::Char('c') => Operator::Change,
        Key::Char('y') => Operator::Yank,
        Key::Char('>') => Operator::Indent,
        Key::Char('<') => Operator::Outdent,
        _ => return None,
    })
}

fn object_of(key: Key) -> Option<Object> {
    let Key::Char(c) = key else { return None };
    Some(match c {
        'w' => Object::Word { big: false },
        'W' => Object::Word { big: true },
        '"' | '\'' | '`' => Object::Quote(c),
        '(' | ')' | 'b' => Object::Brackets('(', ')'),
        '[' | ']' => Object::Brackets('[', ']'),
        '{' | '}' | 'B' => Object::Brackets('{', '}'),
        '<' | '>' => Object::Brackets('<', '>'),
        _ => return None,
    })
}

fn parse_motion(keys: &[Key]) -> Parsed {
    let Some(&first) = keys.first() else {
        return Parsed::Incomplete;
    };
    let char_arg = |make: fn(char) -> Motion| match keys.get(1) {
        None => Parsed::Incomplete,
        Some(Key::Char(c)) => Parsed::Done(None, Cmd::Move(make(*c))),
        Some(_) => Parsed::Invalid,
    };
    let motion = match first {
        Key::Char('h') | Key::Left | Key::Backspace => Motion::Left,
        Key::Char('l') | Key::Right => Motion::Right,
        Key::Char('j') | Key::Down => Motion::Down,
        Key::Char('k') | Key::Up => Motion::Up,
        Key::Char('w') => Motion::WordStart(false),
        Key::Char('W') => Motion::WordStart(true),
        Key::Char('b') => Motion::WordBack(false),
        Key::Char('B') => Motion::WordBack(true),
        Key::Char('e') => Motion::WordEnd(false),
        Key::Char('E') => Motion::WordEnd(true),
        Key::Char('0') | Key::Home => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonBlank,
        Key::Char('$') | Key::End => Motion::LineEnd,
        Key::Char('G') => Motion::FileEnd,
        Key::Char('g') => {
            return match keys.get(1) {
                None => Parsed::Incomplete,
                Some(Key::Char('g')) => Parsed::Done(None, Cmd::Move(Motion::FileStart)),
                Some(_) => Parsed::Invalid,
            };
        }
        Key::Char('f') => {
            return char_arg(|c| Motion::Find {
                target: c,
                forward: true,
                till: false,
            });
        }
        Key::Char('t') => {
            return char_arg(|c| Motion::Find {
                target: c,
                forward: true,
                till: true,
            });
        }
        Key::Char('F') => {
            return char_arg(|c| Motion::Find {
                target: c,
                forward: false,
                till: false,
            });
        }
        Key::Char('T') => {
            return char_arg(|c| Motion::Find {
                target: c,
                forward: false,
                till: true,
            });
        }
        Key::Char(';') => Motion::RepeatFind { reverse: false },
        Key::Char(',') => Motion::RepeatFind { reverse: true },
        Key::Char('}') => Motion::ParagraphForward,
        Key::Char('{') => Motion::ParagraphBack,
        Key::Char('%') => Motion::MatchBracket,
        Key::Char('n') => Motion::SearchNext { reverse: false },
        Key::Char('N') => Motion::SearchNext { reverse: true },
        _ => return Parsed::Invalid,
    };
    Parsed::Done(None, Cmd::Move(motion))
}

fn parse_action(keys: &[Key]) -> Parsed {
    let Some(&first) = keys.first() else {
        return Parsed::Incomplete;
    };
    let op = |op: Operator, target: Target| Parsed::Done(None, Cmd::Operate(op, target));
    let act = |a: Action| Parsed::Done(None, Cmd::Act(a));
    match first {
        Key::Char('i') => act(Action::Insert),
        Key::Char('a') => act(Action::Append),
        Key::Char('I') => act(Action::InsertLineStart),
        Key::Char('A') => act(Action::AppendLineEnd),
        Key::Char('o') => act(Action::OpenBelow),
        Key::Char('O') => act(Action::OpenAbove),
        Key::Char('x') | Key::Delete => op(Operator::Delete, Target::Motion(Motion::Right)),
        Key::Char('X') => op(Operator::Delete, Target::Motion(Motion::Left)),
        Key::Char('s') => op(Operator::Change, Target::Motion(Motion::Right)),
        Key::Char('S') => op(Operator::Change, Target::Lines),
        Key::Char('D') => op(Operator::Delete, Target::Motion(Motion::LineEnd)),
        Key::Char('C') => op(Operator::Change, Target::Motion(Motion::LineEnd)),
        Key::Char('Y') => op(Operator::Yank, Target::Motion(Motion::LineEnd)),
        Key::Char('p') => act(Action::PutAfter),
        Key::Char('P') => act(Action::PutBefore),
        Key::Char('J') => act(Action::Join),
        Key::Char('r') => match keys.get(1) {
            None => Parsed::Incomplete,
            Some(Key::Char(c)) => act(Action::Replace(*c)),
            Some(_) => Parsed::Invalid,
        },
        Key::Char('~') => act(Action::ToggleCase),
        Key::Char('u') => act(Action::Undo),
        Key::Ctrl('r') => act(Action::Redo),
        Key::Char('.') => act(Action::Repeat),
        Key::Char('v') => act(Action::Visual),
        Key::Char('V') => act(Action::VisualLine),
        Key::Char(':') => act(Action::CommandLine),
        Key::Char('/') => act(Action::Search { forward: true }),
        Key::Char('?') => act(Action::Search { forward: false }),
        Key::Char('*') => act(Action::SearchWord),
        Key::Ctrl('d') => act(Action::HalfPageDown),
        Key::Ctrl('u') => act(Action::HalfPageUp),
        Key::Ctrl('s') => act(Action::Save),
        Key::Char('z') => match keys.get(1) {
            None => Parsed::Incomplete,
            Some(Key::Char('z')) => act(Action::Center),
            Some(_) => Parsed::Invalid,
        },
        _ => Parsed::Invalid,
    }
}

fn parse_leader(keys: &[Key], context: When) -> Parsed {
    let mut typed = String::new();
    for key in keys {
        match key {
            Key::Char(c) => typed.push(*c),
            Key::Enter => typed.push('↵'),
            _ => return Parsed::Invalid,
        }
    }
    if let Some(n) = typed.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
        return Parsed::Done(None, Cmd::GoTab(n));
    }
    if let Some(b) = bindings(context).find(|b| !b.leader.is_empty() && b.leader == typed) {
        return Parsed::Done(None, Cmd::Leader(b.command));
    }
    if bindings(context).any(|b| b.leader.starts_with(typed.as_str())) {
        Parsed::Incomplete
    } else {
        Parsed::Invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys from a Vim-like string: `<Esc>`, `<CR>`, `<BS>`, `<C-r>`, `<Space>`; other characters
    /// are themselves.
    fn keys(s: &str) -> Vec<Key> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            if c == '<'
                && let Some(end) = rest.find('>')
            {
                let name = &rest[1..end];
                let key = match name {
                    "Esc" => Some(Key::Esc),
                    "CR" => Some(Key::Enter),
                    "BS" => Some(Key::Backspace),
                    "Space" => Some(Key::Char(' ')),
                    "Tab" => Some(Key::Tab),
                    n if n.starts_with("C-") => n[2..].chars().next().map(Key::Ctrl),
                    _ => None,
                };
                if let Some(key) = key {
                    out.push(key);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            out.push(Key::Char(c));
            rest = &rest[c.len_utf8()..];
        }
        out
    }

    /// An editor on `text` with the cursor on the first `|` (which is removed).
    fn ed(text: &str) -> Editor {
        let at = text.find('|').map(|i| text[..i].chars().count());
        let mut e = Editor::new(Document::new(&text.replace('|', "")));
        e.cursor = at.unwrap_or(0);
        e
    }

    /// The text with `|` where the cursor is.
    fn show(e: &Editor) -> String {
        let mut s = e.doc.rope.to_string();
        let byte = s.char_indices().nth(e.cursor).map_or(s.len(), |(i, _)| i);
        s.insert(byte, '|');
        s
    }

    fn run(text: &str, input: &str) -> String {
        let mut e = ed(text);
        for k in keys(input) {
            e.handle_key(k);
        }
        show(&e)
    }

    #[test]
    fn motions() {
        assert_eq!(run("|one two three", "w"), "one |two three");
        assert_eq!(run("|one two three", "2w"), "one two |three");
        assert_eq!(run("one two |three", "b"), "one |two three");
        assert_eq!(run("|one two", "e"), "on|e two");
        assert_eq!(run("|one two", "$"), "one tw|o");
        assert_eq!(run("  on|e", "0"), "|  one");
        assert_eq!(run("  on|e", "^"), "  |one");
        assert_eq!(run("|a(b)c", "fc"), "a(b)|c");
        assert_eq!(run("|a(b)c", "tc"), "a(b|)c");
        assert_eq!(run("|a,b,c", "f,;"), "a,b|,c");
        assert_eq!(run("|a(b)c", "%"), "a(b|)c");
        assert_eq!(run("|1\n2\n3\n", "G"), "1\n2\n|3\n");
        assert_eq!(run("1\n2\n|3\n", "gg"), "|1\n2\n3\n");
        assert_eq!(run("|1\n2\n3\n", "2G"), "1\n|2\n3\n");
        // Up and down keep the column across a short line.
        assert_eq!(run("abc|d\nx\nabcd\n", "jj"), "abcd\nx\nabc|d\n");
    }

    #[test]
    fn delete_change_yank_with_counts_and_objects() {
        assert_eq!(run("|one two three", "dw"), "|two three");
        assert_eq!(run("|one two three", "d2w"), "|three");
        assert_eq!(run("|one two three", "2dw"), "|three");
        assert_eq!(run("one |two\nnext", "dw"), "one| \nnext"); // stops at the line end
        assert_eq!(run("|one two", "cwONE<Esc>"), "ON|E two"); // cw = ce
        assert_eq!(
            run("say \"hi |there\" ok", "ci\"bye<Esc>"),
            "say \"by|e\" ok"
        );
        assert_eq!(run("f(a, |b)", "da("), "|f");
        assert_eq!(run("|foo bar", "diw"), "| bar");
        assert_eq!(run("|foo bar", "daw"), "|bar");
        assert_eq!(run("|abc", "x"), "|bc");
        assert_eq!(run("a|bc", "D"), "|a"); // cursor back on the last character
        assert_eq!(run("|a(b)c", "dt)"), "|)c");
    }

    #[test]
    fn whole_lines() {
        assert_eq!(run("|1\n2\n3\n", "dd"), "|2\n3\n");
        assert_eq!(run("1\n2\n|3\n", "dd"), "1\n|2\n");
        assert_eq!(run("|1\n2\n3\n", "2dd"), "|3\n");
        assert_eq!(run("|1\n2\n3\n", "dj"), "|3\n");
        assert_eq!(run("|1\n2\n", "yyp"), "1\n|1\n2\n");
        assert_eq!(run("1\n|2\n", "yyP"), "1\n|2\n2\n");
        assert_eq!(run("|1\n2", "ddp"), "2\n|1"); // after a last line without a line break
        assert_eq!(run("  |a\n", "ccb<Esc>"), "  |b\n"); // keeps the indentation
        assert_eq!(run("|a\nb\n", ">j"), "  |a\n  b\n");
        assert_eq!(run("  |a\n", "<<"), "|a\n");
    }

    #[test]
    fn insert_open_join_replace() {
        assert_eq!(run("|ab", "ix<Esc>"), "|xab");
        assert_eq!(run("|ab", "ax<Esc>"), "a|xb");
        assert_eq!(run("|ab", "Ax<Esc>"), "ab|x");
        assert_eq!(run("  |a", "Ix<Esc>"), "  |xa");
        assert_eq!(run("  |a\n", "ob<Esc>"), "  a\n  |b\n"); // autoindent
        assert_eq!(run("|a\n", "Ob<Esc>"), "|b\na\n");
        assert_eq!(run("|a\n    b\n", "J"), "a| b\n");
        assert_eq!(run("|abc", "2rx"), "x|xc");
        assert_eq!(run("|ab", "~"), "A|b");
        assert_eq!(run("a|b", "i<CR><Esc>"), "a\n|b");
        assert_eq!(run("a|b", "a<BS><BS><Esc>"), "|"); // backspace in insert
        assert_eq!(run("|", "ifoo bar<C-w><Esc>"), "foo| ");
    }

    #[test]
    fn undo_redo_and_dot() {
        assert_eq!(run("|one two", "dwu"), "|one two");
        assert_eq!(run("|one two", "dwu<C-r>"), "|two");
        assert_eq!(run("|a b c d", "dw."), "|c d");
        assert_eq!(run("|a b c d", "dw2."), "|d");
        assert_eq!(run("|x\ny\n", "Ahi<Esc>j."), "xhi\nyh|i\n"); // repeats the insert too
        assert_eq!(run("|one two", "cwX<Esc>w."), "X |X");
        // A whole insert session is one undo step.
        assert_eq!(run("|", "iabc<Esc>u"), "|");
    }

    #[test]
    fn visual_mode() {
        assert_eq!(run("|one two", "vwd"), "|wo");
        assert_eq!(run("|one two", "veyP"), "on|eone two");
        assert_eq!(run("|1\n2\n3\n", "Vjd"), "|3\n");
        assert_eq!(run("a \"|bc\" d", "vi\"d"), "a \"|\" d");
        assert_eq!(run("|1\n2\n", "Vj>"), "  |1\n  2\n");
    }

    #[test]
    fn search_and_command_line() {
        assert_eq!(run("|foo bar foo", "/foo<CR>"), "foo bar |foo");
        assert_eq!(run("|foo bar foo", "/foo<CR>n"), "|foo bar foo"); // wraps
        assert_eq!(run("foo bar |foo", "?bar<CR>"), "foo |bar foo");
        assert_eq!(run("|foo bar foo", "*"), "foo bar |foo");
        assert_eq!(run("|1\n2\n3\n", ":3<CR>"), "1\n2\n|3\n");
        let mut e = ed("|a");
        for k in keys("x:q<CR>") {
            e.handle_key(k);
        }
        assert!(!e.quit);
        assert!(e.message.as_deref().unwrap().contains("No write"));
        for k in keys(":q!<CR>") {
            e.handle_key(k);
        }
        assert!(e.quit);
    }

    #[test]
    fn leader_menu_like_which_key() {
        let mut e = ed("|a");
        e.handle_key(Key::Char(' '));
        assert_eq!(
            e.leader_menu(),
            Some(vec![
                ('↵', "Run (F5)"),
                (' ', "Find Files"),
                ('1', "1…9  Go to Tab (1: files)"),
                ('/', "Find Text (Grep)"),
                ('b', "Build / Check (no run)"),
                ('e', "Explorer (folder tree; Ctrl+B)"),
                ('f', "+file/find"),
                ('o', "Find in File (Ctrl+F)"),
                ('q', "+quit/session"),
                ('s', "+search"),
                ('t', "Terminal open / close (Ctrl+/)"),
                ('u', "+ui"),
                ('w', "Save (Ctrl+S)")
            ])
        );
        e.handle_key(Key::Char('u'));
        assert_eq!(
            e.leader_menu(),
            Some(vec![
                ('C', "Colorscheme with Preview"),
                ('L', "Toggle Relative Numbers"),
                ('l', "Toggle Line Numbers"),
                ('m', "Toggle Markdown Reader"),
                ('r', "Clear Search Highlight")
            ])
        );
        e.handle_key(Key::Char('l'));
        assert!(!e.number);
        assert_eq!(e.leader_menu(), None);
        // Space q q quits (nothing changed).
        for k in keys("<Space>qq") {
            e.handle_key(k);
        }
        assert!(e.quit);
    }

    #[test]
    fn tree_opens_files_and_hands_focus_back() {
        let dir = std::env::temp_dir().join(format!("fener-ed-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.txt"), "aaa\n").unwrap();
        std::fs::write(dir.join("b.txt"), "bbb\n").unwrap();
        let mut e = Editor::new(Document::open(&dir.join("a.txt")).unwrap());
        for k in keys("<Space>e") {
            e.handle_key(k);
        }
        assert!(e.tree_focus);
        assert_eq!(
            e.tree.as_ref().unwrap().selected_row().unwrap().name,
            "a.txt"
        );
        for k in keys("j<CR>") {
            e.handle_key(k);
        }
        assert_eq!(e.doc.rope.to_string(), "bbb\n");
        assert!(!e.tree_focus && e.tree.is_some());
        // Ctrl+H back to the tree, q closes it.
        e.handle_key(Key::Ctrl('h'));
        e.handle_key(Key::Char('q'));
        assert!(e.tree.is_none() && !e.tree_focus);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_opens_in_the_reader_and_i_edits_where_it_was() {
        let dir = std::env::temp_dir().join(format!("fener-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.md");
        std::fs::write(&file, "# A\n\none\ntwo\nthree\n").unwrap();
        let mut e = Editor::new(Document::open(&file).unwrap());
        assert!(e.reader);
        for k in keys("jjj") {
            e.handle_key(k);
        }
        assert_eq!(e.reader_top, 3);
        // x does nothing in the reader (no edit by accident).
        e.handle_key(Key::Char('x'));
        assert!(!e.doc.is_modified());
        e.handle_key(Key::Char('i'));
        assert!(!e.reader);
        assert_eq!((e.line(), e.mode), (3, Mode::Normal));
        for k in keys("<Space>um") {
            e.handle_key(k);
        }
        assert!(e.reader);
        // The Space menu of a Markdown file: no run / build, but reader and headings.
        e.handle_key(Key::Char(' '));
        let menu = e.leader_menu().unwrap();
        assert!(menu.contains(&('m', "Reader / Source (Ctrl+E)")));
        assert!(menu.contains(&('h', "Go to Heading")));
        assert!(!menu.iter().any(|(k, _)| *k == '↵' || *k == 'b'));
        // Space h: the headings; picking one scrolls the reader there.
        e.handle_key(Key::Char('h'));
        assert_eq!(e.picker.as_ref().unwrap().items[0].text, "A");
        e.handle_key(Key::Enter);
        assert_eq!(e.reader_top, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_f_finds_in_the_file_and_n_goes_on() {
        let mut e = ed("|alpha\nbeta\ngamma beta\n");
        e.handle_key(Key::Ctrl('f'));
        for k in keys("beta") {
            e.handle_key(k);
        }
        let p = e.picker.as_ref().unwrap();
        assert_eq!(p.matches.len(), 2);
        assert_eq!(p.matches[1].positions, vec![6, 7, 8, 9]);
        e.handle_key(Key::Down);
        e.handle_key(Key::Enter);
        assert_eq!(e.line(), 2);
        assert_eq!(e.last_search.as_deref(), Some("beta"));
    }

    #[test]
    fn f5_saves_and_asks_to_run_and_ctrl_slash_asks_for_the_terminal() {
        let dir = std::env::temp_dir().join(format!("fener-f5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hi.py");
        std::fs::write(&file, "print(1)\n").unwrap();
        let mut e = Editor::new(Document::open(&file).unwrap());
        e.handle_key(Key::Char('x'));
        e.handle_key(Key::F(5));
        assert!(!e.doc.is_modified(), "saved before running");
        let run = crate::run::Run {
            dir: dir.clone(),
            command: "python3 hi.py".into(),
        };
        assert!(e.requests.contains(&Request::Run(run)));
        e.handle_key(Key::Ctrl('/'));
        assert_eq!(e.requests.last(), Some(&Request::ToggleTerminal));
        // Space as a super key: Enter runs, b builds, t terminal, 3 tab three.
        e.requests.clear();
        for k in keys("<Space><CR><Space>b<Space>t<Space>3") {
            e.handle_key(k);
        }
        let build = crate::run::Run {
            dir: dir.clone(),
            command: "python3 -m py_compile hi.py".into(),
        };
        assert_eq!(e.requests.len(), 4);
        assert_eq!(e.requests[1], Request::Run(build));
        assert_eq!(
            e.requests[2..],
            [Request::ToggleTerminal, Request::SwitchTab(2)]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dashboard_menu_and_new_file() {
        let mut e = ed("|");
        e.dashboard = Some(0);
        e.handle_key(Key::Char('j'));
        assert_eq!(e.dashboard, Some(1));
        // `?` opens the keymaps picker over the start screen; Esc goes back to it.
        e.handle_key(Key::Char('?'));
        assert_eq!(e.picker.as_ref().unwrap().kind, Kind::Keymaps);
        e.handle_key(Key::Esc);
        assert!(e.picker.is_none() && e.dashboard.is_some());
        e.handle_key(Key::Char('n'));
        assert_eq!((e.dashboard, e.mode), (None, Mode::Insert));
    }

    #[test]
    fn keymaps_picker_runs_commands() {
        let mut e = ed("|a");
        for k in keys("<Space>sk") {
            e.handle_key(k);
        }
        for k in keys("line num") {
            e.handle_key(k);
        }
        let p = e.picker.as_ref().unwrap();
        assert_eq!(p.current().unwrap().text, "Toggle Line Numbers");
        e.handle_key(Key::Enter);
        assert!(e.picker.is_none() && !e.number);
        // Keys can be searched too: a cheat-sheet row is shown, not run.
        for k in keys("<Space>skdd<CR>") {
            e.handle_key(k);
        }
        assert_eq!(e.message.as_deref(), Some("dd  Delete a line"));
    }

    #[test]
    fn themes_preview_and_escape_restores() {
        let mut e = ed("|a");
        e.themes = vec!["night", "storm", "moon"];
        e.run_command(Command::Themes);
        e.handle_key(Key::Down);
        assert_eq!(e.theme, 1);
        e.handle_key(Key::Esc);
        assert_eq!(e.theme, 0);
        e.run_command(Command::Themes);
        for k in keys("moon<CR>") {
            e.handle_key(k);
        }
        assert_eq!((e.theme, e.state.theme.as_deref()), (2, Some("moon")));
        assert!(e.requests.contains(&Request::SaveState));
    }

    #[test]
    fn grep_results_for_an_old_query_are_dropped() {
        let mut e = ed("|a");
        e.run_command(Command::FindText);
        for k in keys("fo") {
            e.handle_key(k);
        }
        assert_eq!(
            e.requests,
            [Request::Grep("f".into()), Request::Grep("fo".into())]
        );
        let hit = |t: &str| Item {
            text: t.into(),
            detail: String::new(),
            pick: Pick::Info,
        };
        e.receive(Kind::Grep, "f", vec![hit("old")]);
        assert!(e.picker.as_ref().unwrap().items.is_empty());
        e.receive(Kind::Grep, "fo", vec![hit("food")]);
        let p = e.picker.as_ref().unwrap();
        assert_eq!(
            (p.items.len(), p.matches[0].positions.clone()),
            (1, vec![0, 1])
        );
    }

    #[test]
    fn open_refuses_to_drop_changes() {
        let dir = std::env::temp_dir().join(format!("fener-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("b.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let mut e = ed("|a");
        e.handle_key(Key::Char('x'));
        e.open(&file, Some(2), false);
        assert!(e.message.as_deref().unwrap().starts_with("No write"));
        e.open(&file, Some(2), true);
        assert_eq!(
            (e.line(), e.doc.rope.to_string().as_str()),
            (2, "one\ntwo\nthree\n")
        );
        assert_eq!(e.state.recent[0], file);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_z_and_ctrl_y_in_every_mode() {
        let mut e = ed("|abc");
        // Normal mode.
        e.handle_key(Key::Char('x'));
        e.handle_key(Key::Ctrl('z'));
        assert_eq!(e.doc.rope.to_string(), "abc");
        e.handle_key(Key::Ctrl('y'));
        assert_eq!(e.doc.rope.to_string(), "bc");
        // Insert mode: what was typed goes, typing goes on.
        for k in keys("ixy") {
            e.handle_key(k);
        }
        e.handle_key(Key::Ctrl('z'));
        assert_eq!(
            (e.doc.rope.to_string().as_str(), e.mode),
            ("bc", Mode::Insert)
        );
        e.handle_key(Key::Char('q'));
        assert_eq!(e.doc.rope.to_string(), "qbc");
    }

    #[test]
    fn ctrl_e_switches_markdown_between_reader_and_editing() {
        let dir = std::env::temp_dir().join(format!("fener-ctrl-e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("n.md");
        std::fs::write(&file, "# T\n").unwrap();
        let mut e = Editor::new(Document::open(&file).unwrap());
        assert!(e.reader);
        e.handle_key(Key::Ctrl('e'));
        assert!(!e.reader);
        e.handle_key(Key::Char('i'));
        e.handle_key(Key::Ctrl('e'));
        assert!(e.reader && e.mode == Mode::Normal);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

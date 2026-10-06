//! The editor: one document, a cursor and Vim's modes. Keys come in, the text and the cursor
//! change; nothing here draws.
//!
//! A Normal-mode command is parsed as a small grammar instead of a table of every combination
//! (fm-research `notes/edtui.md`, modit's `ViCmd`): `[count] [operator [count]] (motion | object)`,
//! so `d3w`, `2dd`, `ci"` and `y}` all come from the same few pieces. The Space key opens
//! LazyVim-style leader commands; while one is being typed, [`Editor::leader_menu`] lists what can
//! follow (which-key).

use crate::document::Document;
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
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
    Leader(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    /// More keys are needed (`d`, `g`, `f`, `Space q`).
    Incomplete,
    Invalid,
    /// Count (if one was typed) and the command.
    Done(Option<usize>, Cmd),
}

/// Leader (Space) commands as LazyVim has them, and the names of their groups.
const LEADER: &[(&str, &str)] = &[
    ("qq", "Quit all"),
    ("ul", "Toggle line numbers"),
    ("uL", "Toggle relative numbers"),
];
const LEADER_GROUPS: &[(&str, &str)] = &[("q", "+quit/session"), ("u", "+ui")];

/// Lines kept visible above and below the cursor (LazyVim: `scrolloff = 4`).
const SCROLL_OFF: usize = 4;
/// Spaces per indent level (LazyVim: `shiftwidth = 2`, `expandtab`).
const INDENT: &str = "  ";

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
            relative_number: true,
            quit: false,
        }
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
                _ => None,
            })
            .collect();
        let mut items: Vec<(char, &'static str)> = Vec::new();
        for (keys, label) in LEADER {
            if let Some(rest) = keys.strip_prefix(typed.as_str())
                && let Some(next) = rest.chars().next()
            {
                let label = if rest.chars().count() == 1 {
                    *label
                } else {
                    let group = &keys[..typed.len() + next.len_utf8()];
                    LEADER_GROUPS
                        .iter()
                        .find(|(g, _)| *g == group)
                        .map_or("+more", |(_, name)| *name)
                };
                if !items.iter().any(|(k, _)| *k == next) {
                    items.push((next, label));
                }
            }
        }
        items.sort_by_key(|(k, _)| *k);
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
        let (count, cmd) = match parse(&self.pending) {
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
            Cmd::Leader(name) => self.leader(name),
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
                            self.doc.edit(start, start, INDENT);
                        }
                    } else {
                        let blank = content
                            .chars()
                            .take_while(|c| *c == ' ')
                            .count()
                            .min(INDENT.len());
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

    fn leader(&mut self, name: &str) {
        match name {
            "Quit all" => self.ex("qa"),
            "Toggle line numbers" => self.number = !self.number,
            "Toggle relative numbers" => self.relative_number = !self.relative_number,
            _ => {}
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
                self.doc.edit(self.cursor, self.cursor, INDENT);
                self.cursor += INDENT.len();
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

fn parse(keys: &[Key]) -> Parsed {
    let (count, rest) = parse_count(keys);
    let Some(&first) = rest.first() else {
        return Parsed::Incomplete;
    };
    if first == Key::Char(' ') {
        return parse_leader(&rest[1..]);
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

fn parse_leader(keys: &[Key]) -> Parsed {
    let mut typed = String::new();
    for key in keys {
        match key {
            Key::Char(c) => typed.push(*c),
            _ => return Parsed::Invalid,
        }
    }
    if let Some((_, name)) = LEADER.iter().find(|(k, _)| *k == typed) {
        return Parsed::Done(None, Cmd::Leader(name));
    }
    if LEADER.iter().any(|(k, _)| k.starts_with(typed.as_str())) {
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
            Some(vec![('q', "+quit/session"), ('u', "+ui")])
        );
        e.handle_key(Key::Char('u'));
        assert_eq!(
            e.leader_menu(),
            Some(vec![
                ('L', "Toggle relative numbers"),
                ('l', "Toggle line numbers")
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
}

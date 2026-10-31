//! Normal mode: running a parsed command. Motions move the cursor, operators work on the range a
//! motion or text object covers, and actions are the one-key commands (`p`, `J`, `u`, `.`...).

use super::grammar::{Action, Cmd, Motion, Object, Operator, Parsed, Target, parse};
use super::{Editor, Key, LspAsk, Mode, Register, Request, leading_blank};
use crate::syntax::Lang;
use crate::text;

impl Editor {
    pub(super) fn normal_key(&mut self, key: Key) {
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

    pub(super) fn run(&mut self, count: Option<usize>, cmd: Cmd) {
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
    pub(super) fn motion_target(
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
    pub(super) fn line_range(
        &self,
        first: usize,
        last: usize,
        linewise: bool,
    ) -> (usize, usize, bool) {
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

    pub(super) fn apply_operator(&mut self, op: Operator, from: usize, to: usize, linewise: bool) {
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
            Operator::Comment => {
                self.toggle_comment(first_line, last_line);
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

    pub(super) fn act(&mut self, action: Action, n: usize) {
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
            Action::Hover => self.ask_lsp(LspAsk::Hover),
            Action::Definition => self.ask_lsp(LspAsk::Definition),
            Action::References => self.ask_lsp(LspAsk::References),
            Action::NextDiagnostic { forward } => self.next_diagnostic(forward),
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

    /// `gcc` / `gc{motion}` / Visual `gc`: comments the lines out with the language's line
    /// comment, or takes it away when they all have one.
    fn toggle_comment(&mut self, first: usize, last: usize) {
        let Some(mark) = self.comment_mark() else {
            self.message = Some("No line comment known for this kind of file".into());
            return;
        };
        let rope = &self.doc.rope;
        let lines: Vec<String> = (first..=last).map(|l| text::line_text(rope, l)).collect();
        let used: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        let all_commented =
            !used.is_empty() && used.iter().all(|l| l.trim_start().starts_with(mark));
        // New marks go at the smallest indent, so the block stays aligned.
        let indent = used
            .iter()
            .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
            .min()
            .unwrap_or(0);
        for (i, content) in lines.iter().enumerate() {
            if content.trim().is_empty() {
                continue;
            }
            let start = text::line_start(&self.doc.rope, first + i);
            if all_commented {
                let lead = content
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .count();
                let after = &content[content.char_indices().nth(lead).map_or(0, |(b, _)| b)..];
                let mut len = mark.chars().count();
                if after[mark.len()..].starts_with(' ') {
                    len += 1;
                }
                self.doc.edit(start + lead, start + lead + len, "");
            } else {
                self.doc
                    .edit(start + indent, start + indent, &format!("{mark} "));
            }
        }
        self.cursor = text::first_non_blank(&self.doc.rope, first);
    }

    /// The line comment of the open file's language.
    fn comment_mark(&self) -> Option<&'static str> {
        match self.doc.path().map(Lang::from_path) {
            Some(Lang::CLike) => Some("//"),
            Some(Lang::Hash) => Some("#"),
            Some(Lang::Dash) => Some("--"),
            _ => None,
        }
    }

    /// Alt+J / Alt+K: swaps the line under the cursor (Visual: the selected lines) with the one
    /// below / above. One undo step; Visual mode keeps its selection on the moved lines.
    pub(super) fn move_lines(&mut self, down: bool) {
        let rope = &self.doc.rope;
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let (first, last) = if visual {
            let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
            (text::line_of(rope, a), text::line_of(rope, b))
        } else {
            let l = self.line();
            (l, l)
        };
        let lines = text::line_count(rope);
        if (down && last + 1 >= lines) || (!down && first == 0) {
            return;
        }
        let (top, bottom) = if down {
            (first, last + 1)
        } else {
            (first - 1, last)
        };
        let mut block: Vec<String> = (top..=bottom).map(|l| text::line_text(rope, l)).collect();
        if down {
            block.rotate_right(1);
        } else {
            block.rotate_left(1);
        }
        let from = text::line_start(rope, top);
        let to = text::line_start(rope, bottom) + text::line_len(rope, bottom);
        let (col, anchor_col) = (self.col(), text::col_of(rope, self.anchor));
        let anchor_line = text::line_of(rope, self.anchor);
        let insert = self.mode == Mode::Insert;
        if !insert {
            self.doc.begin_step(self.cursor);
        }
        self.doc.edit(from, to, &block.join("\n"));
        let shift = |l: usize| if down { l + 1 } else { l - 1 };
        let rope = &self.doc.rope;
        self.cursor = text::at_col(
            rope,
            shift(self.line_of_cursor_before(first, last)),
            col,
            insert,
        );
        if visual {
            self.anchor = text::at_col(rope, shift(anchor_line), anchor_col, false);
        }
        if !insert {
            self.doc.end_step(self.cursor);
        }
    }

    /// The cursor's line before a move of lines `first..=last` (it is one of them).
    fn line_of_cursor_before(&self, first: usize, last: usize) -> usize {
        self.line().clamp(first, last)
    }
}

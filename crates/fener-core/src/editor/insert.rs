//! Insert mode: typing, auto pairs, indentation and asking for completions.

use super::{Editor, Key, Mode, closes, leading_blank};
use crate::text;

impl Editor {
    pub(super) fn insert_key(&mut self, key: Key) {
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
            Key::Char(c) if self.lsp.active && completes(c) => {
                self.insert_key_plain(c);
                self.ask_completion();
            }
            Key::Char(c) => {
                self.lsp.completion = None;
                let next = rope.get_char(self.cursor);
                let prev = self.cursor.checked_sub(1).and_then(|i| rope.get_char(i));
                if self.auto_pairs && next == Some(c) && closes(c) {
                    // Typing the closer that is already there steps over it.
                    self.cursor += 1;
                } else if let Some(close) = self.pair_for(c, prev, next) {
                    self.doc
                        .edit(self.cursor, self.cursor, &format!("{c}{close}"));
                    self.cursor += 1;
                } else {
                    self.doc
                        .edit(self.cursor, self.cursor, c.encode_utf8(&mut [0; 4]));
                    self.cursor += 1;
                }
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
                let prev = self.cursor.checked_sub(1).and_then(|i| rope.get_char(i));
                let next = rope.get_char(self.cursor);
                if self.auto_pairs && is_pair(prev, next) {
                    // Enter inside `{}`: the closer goes to its own line, the cursor in between.
                    let inner = format!("{indent}{}", " ".repeat(self.indent));
                    self.doc
                        .edit(self.cursor, self.cursor, &format!("\n{inner}\n{indent}"));
                    self.cursor += 1 + inner.chars().count();
                } else {
                    self.doc
                        .edit(self.cursor, self.cursor, &format!("\n{indent}"));
                    self.cursor += 1 + indent.chars().count();
                }
            }
            Key::Backspace if self.cursor > 0 => {
                let prev = rope.get_char(self.cursor - 1);
                let next = rope.get_char(self.cursor);
                // Inside an empty pair, both go.
                let both = self.auto_pairs && is_pair(prev, next);
                self.doc
                    .edit(self.cursor - 1, self.cursor + usize::from(both), "");
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

    /// A typed character in Insert mode, with auto pairs (the part of `insert_key` for `Char`).
    fn insert_key_plain(&mut self, c: char) {
        let rope = &self.doc.rope;
        let next = rope.get_char(self.cursor);
        let prev = self.cursor.checked_sub(1).and_then(|i| rope.get_char(i));
        if self.auto_pairs && next == Some(c) && closes(c) {
            self.cursor += 1;
        } else if let Some(close) = self.pair_for(c, prev, next) {
            self.doc
                .edit(self.cursor, self.cursor, &format!("{c}{close}"));
            self.cursor += 1;
        } else {
            self.doc
                .edit(self.cursor, self.cursor, c.encode_utf8(&mut [0; 4]));
            self.cursor += 1;
        }
    }

    /// The closer to put after `c` (auto pairs), if any. Quotes pair only between words, and
    /// `'` not at all in Rust, where it starts lifetimes (`&'a`).
    fn pair_for(&self, c: char, prev: Option<char>, next: Option<char>) -> Option<char> {
        if !self.auto_pairs {
            return None;
        }
        // Before a word character a pair would wrap the word wrongly: just the character.
        if next.is_some_and(|n| n.is_alphanumeric() || n == '_') {
            return None;
        }
        match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' | '`' | '\'' => {
                let rust = self
                    .doc
                    .path()
                    .and_then(|p| p.extension())
                    .is_some_and(|e| e == "rs");
                let after_word = prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == c);
                (!after_word && !(c == '\'' && rust)).then_some(c)
            }
            _ => None,
        }
    }
}

/// Characters that keep (or start) a completion: word characters and `.` `:` `>`.
fn completes(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '>')
}

/// The cursor sits inside an empty pair: `(|)`, `"|"`.
fn is_pair(prev: Option<char>, next: Option<char>) -> bool {
    matches!(
        (prev, next),
        (Some('('), Some(')'))
            | (Some('['), Some(']'))
            | (Some('{'), Some('}'))
            | (Some('"'), Some('"'))
            | (Some('\''), Some('\''))
            | (Some('`'), Some('`'))
    )
}

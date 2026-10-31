//! Visual mode (`v`, `V`): a selection and the operators that work on it.

use super::grammar::{Cmd, Object, Operator, Parsed, object_of, parse_count, parse_motion};
use super::{Editor, Key, Mode};
use crate::text;

impl Editor {
    pub(super) fn visual_key(&mut self, key: Key) {
        self.pending.push(key);
        let keys = self.pending.clone();
        let (count, rest) = parse_count(&keys);
        let Some(&first) = rest.first() else {
            return;
        };
        let op = match (first, rest.get(1)) {
            (Key::Char('g'), Some(Key::Char('c'))) => Some(Operator::Comment),
            _ => match first {
                Key::Char('d' | 'x') | Key::Delete => Some(Operator::Delete),
                Key::Char('y') => Some(Operator::Yank),
                Key::Char('c' | 's') => Some(Operator::Change),
                Key::Char('>') => Some(Operator::Indent),
                Key::Char('<') => Some(Operator::Outdent),
                _ => None,
            },
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
}

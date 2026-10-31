//! The Normal-mode command language: the keys typed so far parsed as
//! `[count] [operator [count]] (motion | object)`, a single action, or a Space leader command.

use super::Key;
use crate::command::{Command, When, bindings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
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
    pub(super) fn vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down)
    }

    pub(super) fn linewise(self) -> bool {
        matches!(
            self,
            Self::Up | Self::Down | Self::FileStart | Self::FileEnd
        )
    }

    /// The character under the target belongs to the range (`e`, `$`, `f`, `t`, `%`).
    pub(super) fn inclusive(self) -> bool {
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
pub(super) enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    /// `gc`: comment lines out, or back in.
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Object {
    Word { big: bool },
    Quote(char),
    Brackets(char, char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Motion(Motion),
    Object {
        object: Object,
        around: bool,
    },
    /// `dd`, `cc`, `yy`, `>>`: whole lines.
    Lines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
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
    // Language server (ADR 0016).
    Hover,
    Definition,
    References,
    NextDiagnostic { forward: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cmd {
    Move(Motion),
    Operate(Operator, Target),
    Act(Action),
    Leader(Command),
    /// `Space 1`…`Space 9`: a tab (the app's; 1 is the file manager).
    GoTab(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Parsed {
    /// More keys are needed (`d`, `g`, `f`, `Space q`).
    Incomplete,
    Invalid,
    /// Count (if one was typed) and the command.
    Done(Option<usize>, Cmd),
}

/// A count before a command: digits, not starting with 0 (`0` is a motion).
pub(super) fn parse_count(keys: &[Key]) -> (Option<usize>, &[Key]) {
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

pub(super) fn parse(keys: &[Key], context: When) -> Parsed {
    let (count, rest) = parse_count(keys);
    let Some(&first) = rest.first() else {
        return Parsed::Incomplete;
    };
    if first == Key::Char(' ') {
        return parse_leader(&rest[1..], context);
    }
    // `gc` is the one operator of two keys; its line form is `gcc`.
    let operator = match rest {
        [Key::Char('g'), Key::Char('c'), ..] => Some((Operator::Comment, 2)),
        _ => operator_of(first).map(|op| (op, 1)),
    };
    if let Some((op, len)) = operator {
        let first = rest[len - 1];
        let (count2, rest) = parse_count(&rest[len..]);
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

pub(super) fn object_of(key: Key) -> Option<Object> {
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

pub(super) fn parse_motion(keys: &[Key]) -> Parsed {
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
        Key::Char('K') => act(Action::Hover),
        // `g` alone is a motion prefix (gg); gd / gr come here when it is not a motion.
        Key::Char('g') => match keys.get(1) {
            None => Parsed::Incomplete,
            Some(Key::Char('d')) => act(Action::Definition),
            Some(Key::Char('r')) => act(Action::References),
            Some(_) => Parsed::Invalid,
        },
        Key::Char(c @ (']' | '[')) => match keys.get(1) {
            None => Parsed::Incomplete,
            Some(Key::Char('d')) => act(Action::NextDiagnostic { forward: c == ']' }),
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

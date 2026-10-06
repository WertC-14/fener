//! Positions in the text and how Vim moves between them. Positions are char indices into the
//! rope; a "line" never includes its line break. Pure functions: nothing here changes the text.

use ropey::Rope;

/// Number of lines as Vim shows them: a final line break does not start another line.
pub fn line_count(rope: &Rope) -> usize {
    let lines = rope.len_lines();
    if lines > 1 && rope.line(lines - 1).len_chars() == 0 {
        lines - 1
    } else {
        lines
    }
}

pub fn line_of(rope: &Rope, pos: usize) -> usize {
    rope.char_to_line(pos.min(rope.len_chars()))
        .min(line_count(rope) - 1)
}

pub fn line_start(rope: &Rope, line: usize) -> usize {
    rope.line_to_char(line.min(line_count(rope) - 1))
}

/// Characters on `line` without its line break.
pub fn line_len(rope: &Rope, line: usize) -> usize {
    let slice = rope.line(line.min(line_count(rope) - 1));
    let mut len = slice.len_chars();
    while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
        len -= 1;
    }
    len
}

/// Position just past the last character of `line` (where `A` inserts).
pub fn line_end(rope: &Rope, line: usize) -> usize {
    line_start(rope, line) + line_len(rope, line)
}

/// Column of `pos` in its line.
pub fn col_of(rope: &Rope, pos: usize) -> usize {
    pos - line_start(rope, line_of(rope, pos))
}

/// The position on `line` at column `col`, kept inside the line. `past_end` allows the column
/// just after the last character (Insert mode).
pub fn at_col(rope: &Rope, line: usize, col: usize, past_end: bool) -> usize {
    let len = line_len(rope, line);
    let max = if past_end || len == 0 { len } else { len - 1 };
    line_start(rope, line) + col.min(max)
}

/// First non-blank character of `line` (`^`), or its end when it is all blank.
pub fn first_non_blank(rope: &Rope, line: usize) -> usize {
    let start = line_start(rope, line);
    let len = line_len(rope, line);
    (0..len)
        .find(|&i| !rope.char(start + i).is_whitespace())
        .map_or(start + len, |i| start + i)
}

/// The text of `line` without its line break.
pub fn line_text(rope: &Rope, line: usize) -> String {
    let start = line_start(rope, line);
    rope.slice(start..start + line_len(rope, line)).to_string()
}

/// Character classes for word motions: blank, word (letters, digits, `_`), punctuation.
/// With `big` (`W`, `B`, `E`) everything that is not blank is one class.
fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// An empty line starts at `pos` (Vim counts an empty line as a word).
fn empty_line_at(rope: &Rope, pos: usize) -> bool {
    rope.char(pos) == '\n' && (pos == 0 || rope.char(pos - 1) == '\n')
}

/// `w` / `W`: start of the next word.
pub fn next_word_start(rope: &Rope, pos: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if pos >= len {
        return len;
    }
    let mut i = pos;
    let start_class = class(rope.char(i), big);
    if start_class != 0 {
        while i < len && class(rope.char(i), big) == start_class {
            i += 1;
        }
    }
    while i < len && rope.char(i).is_whitespace() {
        if rope.char(i) == '\n' {
            i += 1;
            if i < len && rope.char(i) == '\n' {
                return i; // an empty line
            }
            continue;
        }
        i += 1;
    }
    i
}

/// `b` / `B`: start of this word, or of the previous one.
pub fn prev_word_start(rope: &Rope, pos: usize, big: bool) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos - 1;
    while rope.char(i).is_whitespace() {
        if empty_line_at(rope, i) && i < pos {
            return i;
        }
        if i == 0 {
            return 0;
        }
        i -= 1;
    }
    let c = class(rope.char(i), big);
    while i > 0 && class(rope.char(i - 1), big) == c {
        i -= 1;
    }
    i
}

/// `e` / `E`: end of this word, or of the next one.
pub fn word_end(rope: &Rope, pos: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if len == 0 {
        return 0;
    }
    let mut i = (pos + 1).min(len - 1);
    while i + 1 < len && rope.char(i).is_whitespace() {
        i += 1;
    }
    let c = class(rope.char(i), big);
    while i + 1 < len && class(rope.char(i + 1), big) == c {
        i += 1;
    }
    i
}

/// `f`/`t` (forward) and `F`/`T` (backward) on the current line: the `count`-th `target`.
/// `till` stops one character before it. `None` when it is not there.
pub fn find_in_line(
    rope: &Rope,
    pos: usize,
    target: char,
    count: usize,
    forward: bool,
    till: bool,
) -> Option<usize> {
    let line = line_of(rope, pos);
    let (start, end) = (line_start(rope, line), line_end(rope, line));
    let mut found = 0;
    let mut i = pos;
    loop {
        if forward {
            i += 1;
            if i >= end {
                return None;
            }
        } else {
            if i <= start {
                return None;
            }
            i -= 1;
        }
        if rope.char(i) == target {
            found += 1;
            if found == count {
                return Some(match (till, forward) {
                    (false, _) => i,
                    (true, true) => i - 1,
                    (true, false) => i + 1,
                });
            }
        }
    }
}

/// `}` / `{`: the next / previous empty line, or the end / start of the text.
pub fn paragraph(rope: &Rope, pos: usize, forward: bool) -> usize {
    let count = line_count(rope);
    let mut line = line_of(rope, pos);
    loop {
        if forward {
            if line + 1 >= count {
                return line_end(rope, count - 1);
            }
            line += 1;
        } else {
            if line == 0 {
                return 0;
            }
            line -= 1;
        }
        if line_len(rope, line) == 0 {
            return line_start(rope, line);
        }
    }
}

const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// `%`: the bracket matching the first bracket at or after the cursor on this line.
pub fn matching_bracket(rope: &Rope, pos: usize) -> Option<usize> {
    let end = line_end(rope, line_of(rope, pos));
    let at = (pos..end).find(|&i| {
        PAIRS
            .iter()
            .any(|&(o, c)| rope.char(i) == o || rope.char(i) == c)
    })?;
    let ch = rope.char(at);
    let (open, close, forward) = PAIRS.iter().find_map(|&(o, c)| {
        if ch == o {
            Some((o, c, true))
        } else if ch == c {
            Some((o, c, false))
        } else {
            None
        }
    })?;
    let mut depth = 0usize;
    let len = rope.len_chars();
    let mut i = at;
    loop {
        let c = rope.char(i);
        if c == open {
            depth = if forward {
                depth + 1
            } else {
                depth.checked_sub(1)?
            };
        } else if c == close {
            depth = if forward {
                depth.checked_sub(1)?
            } else {
                depth + 1
            };
        }
        if depth == 0 {
            return Some(i);
        }
        if forward {
            i += 1;
            if i >= len {
                return None;
            }
        } else {
            i = i.checked_sub(1)?;
        }
    }
}

/// `iw` / `aw` (and `iW` / `aW`): the word (or run of blanks) under the cursor on its line;
/// `around` adds the blanks after it (or before it when there are none after).
pub fn word_object(rope: &Rope, pos: usize, big: bool, around: bool) -> Option<(usize, usize)> {
    let line = line_of(rope, pos);
    let (start, end) = (line_start(rope, line), line_end(rope, line));
    if start == end {
        return None;
    }
    let pos = pos.min(end - 1);
    let c = class(rope.char(pos), big);
    let mut from = pos;
    while from > start && class(rope.char(from - 1), big) == c {
        from -= 1;
    }
    let mut to = pos + 1;
    while to < end && class(rope.char(to), big) == c {
        to += 1;
    }
    if around && c != 0 {
        let mut trailing = to;
        while trailing < end && rope.char(trailing).is_whitespace() {
            trailing += 1;
        }
        if trailing > to {
            to = trailing;
        } else {
            while from > start && rope.char(from - 1).is_whitespace() {
                from -= 1;
            }
        }
    }
    Some((from, to))
}

/// `i"` / `a"` (and `'`, `` ` ``): the quoted text around the cursor on its line, or the first
/// quoted text after it.
pub fn quote_object(rope: &Rope, pos: usize, quote: char, around: bool) -> Option<(usize, usize)> {
    let line = line_of(rope, pos);
    let (start, end) = (line_start(rope, line), line_end(rope, line));
    let quotes: Vec<usize> = (start..end).filter(|&i| rope.char(i) == quote).collect();
    let (open, close) = quotes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[o, c]| (o, c))
        .find(|&(_, c)| pos <= c)?;
    Some(if around {
        (open, close + 1)
    } else {
        (open + 1, close)
    })
}

/// `i(` / `a(` (and `[`, `{`, `<`): the brackets around the cursor, which may span lines.
pub fn bracket_object(
    rope: &Rope,
    pos: usize,
    open: char,
    close: char,
    around: bool,
) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    // Backwards to the unmatched opening bracket (the cursor may sit on it).
    let mut depth = 0usize;
    let mut i = pos.min(len.checked_sub(1)?);
    let from = loop {
        let c = rope.char(i);
        if c == close && i != pos {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                break i;
            }
            depth -= 1;
        }
        i = i.checked_sub(1)?;
    };
    let mut depth = 0usize;
    let mut j = from + 1;
    let to = loop {
        if j >= len {
            return None;
        }
        let c = rope.char(j);
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                break j;
            }
            depth -= 1;
        }
        j += 1;
    };
    Some(if around {
        (from, to + 1)
    } else {
        (from + 1, to)
    })
}

/// Next (or previous) match of `pattern` from `pos`, wrapping around the end. Smart case: the
/// search ignores case unless the pattern has a capital letter (LazyVim's default).
pub fn search(rope: &Rope, pattern: &str, pos: usize, forward: bool) -> Option<usize> {
    if pattern.is_empty() {
        return None;
    }
    let ignore_case = !pattern.chars().any(char::is_uppercase);
    let fold = |s: &str| {
        if ignore_case {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };
    let text: Vec<char> = fold(&rope.to_string()).chars().collect();
    let needle: Vec<char> = fold(pattern).chars().collect();
    let hits = |i: &usize| text.get(*i..*i + needle.len()) == Some(&needle[..]);
    let n = text.len();
    if forward {
        (pos + 1..n).chain(0..=pos.min(n)).find(hits)
    } else {
        (0..pos).rev().chain((pos..n).rev()).find(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn lines_ignore_the_final_break() {
        let t = r("one\ntwo\n");
        assert_eq!(line_count(&t), 2);
        assert_eq!(line_len(&t, 1), 3);
        assert_eq!(line_count(&r("")), 1);
        assert_eq!(first_non_blank(&r("  x"), 0), 2);
        assert_eq!(at_col(&t, 1, 9, false), 6); // clamped to the last character
        assert_eq!(at_col(&t, 1, 9, true), 7);
    }

    #[test]
    fn word_motions_like_vim() {
        let t = r("foo.bar baz\n\nqux");
        assert_eq!(next_word_start(&t, 0, false), 3); // `.` is its own word
        assert_eq!(next_word_start(&t, 0, true), 8); // `W` skips to baz
        assert_eq!(next_word_start(&t, 8, false), 12); // the empty line is a word
        assert_eq!(next_word_start(&t, 12, false), 13);
        assert_eq!(prev_word_start(&t, 13, false), 12);
        assert_eq!(prev_word_start(&t, 10, false), 8);
        assert_eq!(word_end(&t, 0, false), 2);
        assert_eq!(word_end(&t, 2, false), 3);
        assert_eq!(word_end(&t, 0, true), 6);
    }

    #[test]
    fn find_paragraph_and_brackets() {
        let t = r("a(b, c(d))\n\nend");
        assert_eq!(find_in_line(&t, 0, 'c', 1, true, false), Some(5));
        assert_eq!(find_in_line(&t, 0, 'c', 1, true, true), Some(4));
        assert_eq!(find_in_line(&t, 5, 'a', 1, false, false), Some(0));
        assert_eq!(find_in_line(&t, 0, 'z', 1, true, false), None);
        assert_eq!(paragraph(&t, 0, true), 11);
        assert_eq!(paragraph(&t, 12, false), 11);
        assert_eq!(matching_bracket(&t, 0), Some(9));
        assert_eq!(matching_bracket(&t, 9), Some(1));
    }

    #[test]
    fn text_objects() {
        let t = r("let x = call(\"hi there\", (1));");
        assert_eq!(word_object(&t, 1, false, false), Some((0, 3))); // let
        assert_eq!(word_object(&t, 1, false, true), Some((0, 4))); // let + space
        assert_eq!(quote_object(&t, 16, '"', false), Some((14, 22)));
        assert_eq!(quote_object(&t, 0, '"', true), Some((13, 23))); // first quotes after the cursor
        assert_eq!(bracket_object(&t, 15, '(', ')', false), Some((13, 28)));
        assert_eq!(bracket_object(&t, 26, '(', ')', true), Some((25, 28)));
    }

    #[test]
    fn search_wraps_with_smart_case() {
        let t = r("Foo bar foo");
        assert_eq!(search(&t, "foo", 0, true), Some(8));
        assert_eq!(search(&t, "foo", 8, true), Some(0)); // wraps, case ignored
        assert_eq!(search(&t, "Foo", 0, true), Some(0)); // capital: exact case, wraps to itself
        assert_eq!(search(&t, "bar", 8, false), Some(4));
    }
}

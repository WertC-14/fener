//! Fuzzy matching for the pickers: the query's characters must appear in the text in order,
//! not necessarily next to each other ("edrs" finds `src/editor.rs`). Smart case, like `/`:
//! a query with an uppercase letter is case-sensitive.
//!
//! The score prefers matches at word starts (after `/ _ - .` or a space, or a camelCase hump),
//! runs of consecutive characters, matches inside the file name, and shorter texts.

/// Starts tried for the first query character (a few are enough to find the file-name match).
const MAX_STARTS: usize = 8;

/// The score of `query` in `text` and the char positions it matched, or `None`.
pub fn score(query: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let ignore_case = !query.chars().any(char::is_uppercase);
    let fold = |c: char| {
        if ignore_case {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let q: Vec<char> = query.chars().map(fold).collect();
    let t: Vec<char> = text.chars().collect();
    let folded: Vec<char> = t.iter().map(|&c| fold(c)).collect();
    let name_start = t.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);

    let mut best: Option<(i64, Vec<usize>)> = None;
    let starts = folded
        .iter()
        .enumerate()
        .filter(|&(_, &c)| c == q[0])
        .map(|(i, _)| i)
        .take(MAX_STARTS);
    for start in starts {
        let Some(positions) = greedy(&q, &folded, start) else {
            break;
        };
        let s = rate(&t, &positions, name_start);
        if best.as_ref().is_none_or(|(b, _)| s > *b) {
            best = Some((s, positions));
        }
    }
    best
}

/// Matches `q` in `t` from `start` on, each character as early as possible.
fn greedy(q: &[char], t: &[char], start: usize) -> Option<Vec<usize>> {
    let mut positions = Vec::with_capacity(q.len());
    let mut i = start;
    for &c in q {
        while i < t.len() && t[i] != c {
            i += 1;
        }
        if i == t.len() {
            return None;
        }
        positions.push(i);
        i += 1;
    }
    Some(positions)
}

fn rate(t: &[char], positions: &[usize], name_start: usize) -> i64 {
    let mut s = 0;
    let mut prev: Option<usize> = None;
    for &p in positions {
        let before = p.checked_sub(1).map(|i| t[i]);
        if before.is_none_or(|b| matches!(b, '/' | '_' | '-' | '.' | ' ')) {
            s += 16;
        } else if before.is_some_and(char::is_lowercase) && t[p].is_uppercase() {
            s += 8;
        }
        match prev {
            Some(q) if p == q + 1 => s += 12,
            Some(q) => s -= (p - q - 1).min(8) as i64,
            None => s -= p.min(16) as i64 / 4,
        }
        prev = Some(p);
    }
    if positions[0] >= name_start {
        s += 20;
    }
    s - t.len() as i64 / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_in_order() {
        assert_eq!(score("edr", "src/editor.rs").unwrap().1, vec![4, 5, 9]);
        assert!(score("rde", "editor").is_none());
        assert_eq!(score("", "anything").unwrap().0, 0);
    }

    #[test]
    fn smart_case() {
        assert!(score("readme", "README.md").is_some());
        assert!(score("README", "readme.md").is_none());
    }

    #[test]
    fn prefers_word_starts_and_the_file_name() {
        let s = |q, t| score(q, t).unwrap().0;
        assert!(s("fb", "foo_bar") > s("fb", "xfxbx"));
        assert!(s("main", "crates/fener/src/main.rs") > s("main", "domain/info.rs"));
        // The match in the file name wins over an earlier one in a folder name.
        let (_, pos) = score("ed", "docs/editor/fed.rs").unwrap();
        assert_eq!(pos, vec![13, 14]);
    }
}

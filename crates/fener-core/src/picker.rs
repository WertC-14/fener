//! A picker: a list narrowed while typing, as LazyVim's (snacks) pickers. The editor owns at
//! most one; the UI draws it as a floating box. Lists that take time (files, grep) are filled
//! later by the app's worker, through `Editor::receive`.

use std::path::{Path, PathBuf};

use crate::command::Command;
use crate::files::Hit;
use crate::fuzzy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Files,
    Recent,
    Grep,
    Keymaps,
    Themes,
    /// The lines of the open file (Ctrl+F), matched as plain text, not fuzzy.
    Lines,
    /// Places in files (LSP references): drawn like grep results, matched like lines.
    Locations,
}

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    File {
        path: PathBuf,
        line: Option<usize>,
    },
    Command(Command),
    /// A cheat-sheet row: shown, not run.
    Info,
    Theme(usize),
    /// A line of the open document (0-based).
    Line(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What is matched and shown.
    pub text: String,
    /// Shown beside it, dimmed (keys, `path:line`).
    pub detail: String,
    pub pick: Pick,
}

/// An item that matches the query, with the char positions in `text` to highlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub index: usize,
    pub positions: Vec<usize>,
}

#[derive(Debug)]
pub struct Picker {
    pub kind: Kind,
    pub title: &'static str,
    pub query: String,
    pub items: Vec<Item>,
    pub matches: Vec<Match>,
    pub selected: usize,
    /// The list is still being made (worker running).
    pub loading: bool,
    /// For Themes: the theme to go back to on Esc.
    pub(crate) theme_before: usize,
}

impl Picker {
    pub fn new(kind: Kind, title: &'static str, items: Vec<Item>) -> Self {
        let mut picker = Self {
            kind,
            title,
            query: String::new(),
            items,
            matches: Vec::new(),
            selected: 0,
            loading: false,
            theme_before: 0,
        };
        picker.refilter();
        picker
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.loading = false;
        self.refilter();
    }

    /// Matches the items against the query again. Grep results already match (the worker
    /// searched for the query), so only the query is highlighted in them.
    pub fn refilter(&mut self) {
        let query = self.query.as_str();
        self.matches = if matches!(self.kind, Kind::Lines | Kind::Locations) {
            let ignore_case = !query.chars().any(char::is_uppercase);
            let needle = if ignore_case {
                query.to_lowercase()
            } else {
                query.to_string()
            };
            self.items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    if ignore_case {
                        item.text.to_lowercase().contains(&needle)
                    } else {
                        item.text.contains(&needle)
                    }
                })
                .map(|(index, item)| Match {
                    index,
                    positions: occurrences(query, &item.text),
                })
                .collect()
        } else if self.kind == Kind::Grep {
            (0..self.items.len())
                .map(|index| Match {
                    index,
                    positions: occurrences(query, &self.items[index].text),
                })
                .collect()
        } else {
            let mut scored: Vec<(i64, Match)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    // Keys can be searched too: typing exactly a key ("dd") puts its row
                    // first; a part of one ("d") still finds it, last.
                    let is_key =
                        !query.is_empty() && item.detail.split_whitespace().any(|k| k == query);
                    let (score, positions) = if is_key {
                        (1000, Vec::new())
                    } else {
                        fuzzy::score(query, &item.text).or_else(|| {
                            let compact: String = item.detail.split_whitespace().collect();
                            compact.contains(query).then(|| (-1000, Vec::new()))
                        })?
                    };
                    Some((score, Match { index, positions }))
                })
                .collect();
            if !query.is_empty() {
                // Stable: equal scores keep the list's order.
                scored.sort_by_key(|(score, _)| -score);
            }
            scored.into_iter().map(|(_, m)| m).collect()
        };
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&Item> {
        self.matches
            .get(self.selected)
            .map(|m| &self.items[m.index])
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.matches.len();
        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }
}

/// Items for files given relative to `root`.
pub fn file_items(root: &Path, paths: Vec<PathBuf>) -> Vec<Item> {
    paths
        .into_iter()
        .map(|rel| Item {
            text: rel.to_string_lossy().into_owned(),
            detail: String::new(),
            pick: Pick::File {
                path: root.join(rel),
                line: None,
            },
        })
        .collect()
}

pub fn grep_items(root: &Path, hits: Vec<Hit>) -> Vec<Item> {
    hits.into_iter()
        .map(|hit| Item {
            detail: format!("{}:{}", hit.path.display(), hit.line + 1),
            text: hit.text,
            pick: Pick::File {
                path: root.join(hit.path),
                line: Some(hit.line),
            },
        })
        .collect()
}

/// Char positions of `query` in `text` (smart case), for highlighting grep results.
fn occurrences(query: &str, text: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let ignore_case = !query.chars().any(char::is_uppercase);
    let fold = |s: &str| -> Vec<char> {
        if ignore_case {
            s.chars()
                .map(|c| c.to_lowercase().next().unwrap_or(c))
                .collect()
        } else {
            s.chars().collect()
        }
    };
    let (q, t) = (fold(query), fold(text));
    let mut out = Vec::new();
    let mut i = 0;
    while i + q.len() <= t.len() {
        if t[i..i + q.len()] == q[..] {
            out.extend(i..i + q.len());
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(texts: &[&str]) -> Vec<Item> {
        texts
            .iter()
            .map(|t| Item {
                text: t.to_string(),
                detail: String::new(),
                pick: Pick::Info,
            })
            .collect()
    }

    #[test]
    fn narrows_ranks_and_wraps() {
        let mut p = Picker::new(
            Kind::Files,
            "Files",
            items(&["xmxaxixn", "src/main.rs", "lib.rs"]),
        );
        assert_eq!(p.matches.len(), 3);
        p.query = "main".into();
        p.refilter();
        let texts: Vec<&str> = p
            .matches
            .iter()
            .map(|m| p.items[m.index].text.as_str())
            .collect();
        assert_eq!(texts, ["src/main.rs", "xmxaxixn"]);
        p.move_by(-1);
        assert_eq!(p.current().unwrap().text, "xmxaxixn");
    }

    #[test]
    fn grep_highlights_every_occurrence() {
        assert_eq!(occurrences("ab", "xAbyab"), vec![1, 2, 4, 5]);
        assert!(occurrences("Ab", "xabyab").is_empty());
    }
}

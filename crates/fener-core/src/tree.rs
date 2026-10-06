//! The folder tree beside the text (LazyVim's explorer, `Space e`). It shows the project the
//! open file belongs to (the git repository around it, or its folder), with that file revealed.
//! `Backspace` re-roots it one folder up, so what is around the project can be seen too.
//!
//! Folders are read when expanded, on the thread that handles the key: one `read_dir` per
//! visible folder is fast enough for a tree a person unfolds by hand.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

#[derive(Debug)]
pub struct Tree {
    pub root: PathBuf,
    expanded: HashSet<PathBuf>,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// First row on screen (kept by the UI).
    pub top: usize,
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let mut tree = Self {
            root,
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            top: 0,
        };
        tree.rebuild();
        tree
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Expands the folders down to `path` and selects it. A path outside the root re-roots the
    /// tree at that path's project.
    pub fn reveal(&mut self, path: &Path) {
        if !path.starts_with(&self.root) {
            self.root = project_root(path);
        }
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !d.starts_with(&self.root) {
                break;
            }
            self.expanded.insert(d.to_path_buf());
            dir = d.parent();
        }
        self.rebuild();
        if let Some(i) = self.rows.iter().position(|r| r.path == path) {
            self.selected = i;
        }
    }

    /// Enter on the selected row: a folder opens or closes, a file is returned (to be opened).
    pub fn activate(&mut self) -> Option<PathBuf> {
        let row = self.selected_row()?.clone();
        if !row.is_dir {
            return Some(row.path);
        }
        if !self.expanded.remove(&row.path) {
            self.expanded.insert(row.path);
        }
        self.rebuild();
        None
    }

    /// `h`: closes the selected folder, or else goes to the folder the row is in.
    pub fn collapse(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.is_dir && row.expanded {
            self.expanded.remove(&row.path);
            self.rebuild();
        } else if let Some(parent) = row.path.parent()
            && let Some(i) = self.rows.iter().position(|r| r.path == parent)
        {
            self.selected = i;
        }
    }

    /// `Backspace`: the folder above the root becomes the root (the old root stays open).
    pub fn root_up(&mut self) {
        let Some(parent) = self.root.parent().map(Path::to_path_buf) else {
            return;
        };
        let old = std::mem::replace(&mut self.root, parent);
        self.expanded.insert(old.clone());
        self.rebuild();
        if let Some(i) = self.rows.iter().position(|r| r.path == old) {
            self.selected = i;
        }
    }

    /// Lists the root and every expanded folder again; the selected path stays selected.
    pub fn rebuild(&mut self) {
        let keep = self.selected_row().map(|r| r.path.clone());
        let mut rows = Vec::new();
        self.list(&self.root, 0, &mut rows);
        self.rows = rows;
        self.selected = keep
            .and_then(|p| self.rows.iter().position(|r| r.path == p))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    /// The rows of `dir` at `depth`, each expanded folder followed by its own rows.
    fn list(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for (name, path, is_dir) in read(dir) {
            let expanded = is_dir && self.expanded.contains(&path);
            rows.push(Row {
                path: path.clone(),
                name,
                depth,
                is_dir,
                expanded,
            });
            if expanded {
                self.list(&path, depth + 1, rows);
            }
        }
    }
}

/// The folder a file's tree starts at: the git repository around it, else its own folder.
pub fn project_root(path: &Path) -> PathBuf {
    let start = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(start)
        .to_path_buf()
}

/// A folder's entries: folders first, then by name (ignoring case); `.git` left out.
fn read(dir: &Path) -> Vec<(String, PathBuf, bool)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf, bool)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                return None;
            }
            // Follows symlinks: a link to a folder unfolds like one.
            let is_dir = e.path().is_dir();
            Some((name, e.path(), is_dir))
        })
        .collect();
    out.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_toggles_and_goes_up() {
        let base = std::env::temp_dir().join(format!("fener-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("proj");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::write(root.join("src/deep/a.rs"), "").unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(base.join("notes.txt"), "").unwrap();

        let file = root.join("src/deep/a.rs");
        assert_eq!(project_root(&file), root);
        let mut tree = Tree::new(project_root(&file));
        tree.reveal(&file);
        let names: Vec<(usize, &str)> = tree
            .rows
            .iter()
            .map(|r| (r.depth, r.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (0, "src"),
                (1, "deep"),
                (2, "a.rs"),
                (1, "main.rs"),
                (0, "README.md")
            ]
        );
        assert_eq!(tree.selected_row().unwrap().path, file);
        assert_eq!(tree.activate(), Some(file.clone()));

        // h goes to the folder, h again closes it.
        tree.collapse();
        assert_eq!(tree.selected_row().unwrap().name, "deep");
        tree.collapse();
        assert_eq!(tree.rows.len(), 4);

        // Backspace: the folder around the project, with the project still open.
        tree.root_up();
        assert_eq!(tree.root, base);
        assert_eq!(tree.selected_row().unwrap().name, "proj");
        assert_eq!(tree.rows.last().unwrap().name, "notes.txt");
        fs::remove_dir_all(&base).unwrap();
    }
}

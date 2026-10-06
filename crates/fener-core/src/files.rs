//! The files under the working folder, and searching their text: what "Find Files" and
//! "Find Text" pick from. Both run on a worker thread; `cancel` lets a newer request stop them.
//!
//! Hidden entries (`.git`, `.cache`, ...) and the usual build folders are skipped; `.gitignore`
//! is not read yet.

use std::fs;
use std::path::{Path, PathBuf};

/// Folders that are almost never what one wants to open.
const SKIP_DIRS: &[&str] = &["target", "node_modules", "__pycache__"];
/// Files larger than this are not searched.
const MAX_GREP_SIZE: u64 = 2 * 1024 * 1024;
/// Characters of a matching line that are kept.
const MAX_LINE: usize = 200;

/// Files under `root` as paths relative to it, sorted, at most `limit`.
pub fn list(root: &Path, limit: usize, cancel: &dyn Fn() -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        if cancel() || out.len() >= limit {
            break;
        }
        let Ok(entries) = fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            // `file_type` does not follow symlinks: a link to a folder is not walked into.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = rel.join(name.as_ref());
            if kind.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else {
                out.push(path);
            }
        }
    }
    out.truncate(limit);
    out.sort();
    out
}

/// A line that contains the searched text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Relative to the searched folder.
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    pub text: String,
}

/// Lines of the files under `root` that contain `query` (smart case), at most `limit`.
pub fn grep(root: &Path, query: &str, limit: usize, cancel: &dyn Fn() -> bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    if query.is_empty() {
        return hits;
    }
    let ignore_case = !query.chars().any(char::is_uppercase);
    let needle = if ignore_case {
        query.to_lowercase()
    } else {
        query.to_string()
    };
    for rel in list(root, 50_000, cancel) {
        if cancel() {
            break;
        }
        let path = root.join(&rel);
        if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_GREP_SIZE) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        // Binary files (a NUL near the start) and non-UTF-8 text are skipped.
        if bytes[..bytes.len().min(8000)].contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for (line, content) in text.lines().enumerate() {
            let found = if ignore_case {
                content.to_lowercase().contains(&needle)
            } else {
                content.contains(&needle)
            };
            if found {
                hits.push(Hit {
                    path: rel.clone(),
                    line,
                    text: content.trim_start().chars().take(MAX_LINE).collect(),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fener-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["src", ".git", "target"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("src/main.rs"), "fn main() {\n    Hello();\n}\n").unwrap();
        fs::write(root.join("README.md"), "hello world\n").unwrap();
        fs::write(root.join(".git/config"), "hello\n").unwrap();
        fs::write(root.join("target/out"), "hello\n").unwrap();
        fs::write(root.join("blob.bin"), b"hello\0\x01").unwrap();
        root
    }

    #[test]
    fn lists_and_greps_skipping_hidden_build_and_binary() {
        let root = tree();
        let no = &|| false;
        let files = list(&root, 100, no);
        assert_eq!(
            files,
            [
                PathBuf::from("README.md"),
                "blob.bin".into(),
                "src/main.rs".into()
            ]
        );
        let hits = grep(&root, "hello", 100, no);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].path, PathBuf::from("src/main.rs"));
        assert_eq!((hits[1].line, hits[1].text.as_str()), (1, "Hello();"));
        // An uppercase letter makes the search case-sensitive.
        assert_eq!(grep(&root, "Hello", 100, no).len(), 1);
        assert!(grep(&root, "hello", 100, &|| true).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}

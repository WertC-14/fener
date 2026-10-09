//! What fener remembers between runs: the recently opened files and the chosen theme.
//! Plain text files in `$XDG_STATE_HOME/fener/` (default `~/.local/state/fener/`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How many recent files are kept.
const MAX_RECENT: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Absolute paths, most recent first.
    pub recent: Vec<PathBuf>,
    pub theme: Option<String>,
}

impl State {
    /// Moves `path` (made absolute) to the front of the recent files.
    pub fn remember(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }

    pub fn load() -> Self {
        dir().map_or_else(Self::default, |d| Self::load_from(&d))
    }

    pub fn save(&self) -> io::Result<()> {
        match dir() {
            Some(d) => self.save_to(&d),
            None => Ok(()),
        }
    }

    fn load_from(dir: &Path) -> Self {
        let recent = fs::read_to_string(dir.join("recent"))
            .map(|s| {
                s.lines()
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let theme = fs::read_to_string(dir.join("theme"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self { recent, theme }
    }

    fn save_to(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let mut recent = String::new();
        for path in &self.recent {
            recent.push_str(&path.to_string_lossy());
            recent.push('\n');
        }
        fs::write(dir.join("recent"), recent)?;
        fs::write(dir.join("theme"), self.theme.as_deref().unwrap_or(""))
    }
}

/// The code tabs open when fener last quit: each file and its cursor line (Space q s, Alt+S).
pub fn save_session(files: &[(PathBuf, usize)]) -> io::Result<()> {
    let Some(d) = dir() else { return Ok(()) };
    fs::create_dir_all(&d)?;
    let text: String = files
        .iter()
        .map(|(path, line)| format!("{line}\t{}\n", path.display()))
        .collect();
    fs::write(d.join("session"), text)
}

/// The last session's files that still exist.
pub fn load_session() -> Vec<(PathBuf, usize)> {
    let Some(text) = dir().and_then(|d| fs::read_to_string(d.join("session")).ok()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let (line, path) = l.split_once('\t')?;
            let path = PathBuf::from(path);
            path.exists().then(|| (path, line.parse().unwrap_or(0)))
        })
        .collect()
}

fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("fener"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_most_recent_first_and_round_trips() {
        let mut state = State::default();
        state.remember(Path::new("/a"));
        state.remember(Path::new("/b"));
        state.remember(Path::new("/a"));
        assert_eq!(state.recent, [PathBuf::from("/a"), "/b".into()]);
        state.theme = Some("tokyonight-moon".into());

        let dir = std::env::temp_dir().join(format!("fener-state-{}", std::process::id()));
        state.save_to(&dir).unwrap();
        assert_eq!(State::load_from(&dir), state);
        fs::remove_dir_all(&dir).unwrap();
    }
}

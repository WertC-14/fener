//! `~/.config/fener/config.toml` (fm-research ADR 0013): what makes fener changeable without
//! touching its code. Every field has a default, so an empty or missing file is fine; a broken
//! one is reported and ignored.
//!
//! ```toml
//! [editor]
//! line-numbers = "absolute"   # absolute | relative | off
//! indent = 4                  # spaces per indent level (Tab, >>)
//! tree-open = true            # the folder tree beside a newly opened file
//!
//! [run]                       # F5 by file extension; {file} {stem} {dir}
//! rs = "rustc {file} && ./{stem}"
//!
//! [keys]                      # key = command (names as in the Keymaps list, e.g. "run")
//! F9 = "run"
//! ctrl-t = "terminal"
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::command::Command;
use crate::editor::Key;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub editor: EditorOptions,
    /// File extension → F5 / `Space Enter` command template.
    pub run: HashMap<String, String>,
    /// File extension → `Space b` command template.
    pub build: HashMap<String, String>,
    /// File extension → `Space c f` formatter (stdin → stdout; `{file}` is the file's name).
    pub format: HashMap<String, String>,
    /// Key (`F9`, `ctrl-t`) → command name (`run`, `terminal`, ...).
    pub keys: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct EditorOptions {
    pub line_numbers: LineNumbers,
    pub indent: usize,
    pub tree_open: bool,
    /// Brackets and quotes close themselves.
    pub auto_pairs: bool,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            line_numbers: LineNumbers::Absolute,
            indent: 2,
            tree_open: true,
            auto_pairs: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineNumbers {
    #[default]
    Absolute,
    Relative,
    Off,
}

impl Config {
    /// The user's config, or the defaults and an error message to show.
    pub fn load() -> (Self, Option<String>) {
        match path() {
            Some(p) if p.exists() => match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|text| Self::parse(&text))
            {
                Ok(config) => (config, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", p.display()))),
            },
            _ => (Self::default(), None),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.message().to_string())?;
        // Unknown keys and command names are errors now, not silent no-ops later.
        for (key, command) in &config.keys {
            parse_key(key).ok_or_else(|| format!("[keys] unknown key \"{key}\""))?;
            Command::from_name(command)
                .ok_or_else(|| format!("[keys] unknown command \"{command}\""))?;
        }
        Ok(config)
    }

    /// The `[keys]` table as editor keys and commands.
    pub fn keymap(&self) -> HashMap<Key, Command> {
        self.keys
            .iter()
            .filter_map(|(k, c)| Some((parse_key(k)?, Command::from_name(c)?)))
            .collect()
    }

    /// The `[run]` (or `[build]`) command for `file`, with its placeholders filled in.
    pub fn run_command(&self, file: &Path, goal: crate::run::Goal) -> Option<String> {
        let ext = file.extension()?.to_str()?;
        let table = match goal {
            crate::run::Goal::Run => &self.run,
            crate::run::Goal::Build => &self.build,
        };
        let template = table.get(ext)?;
        let quote = |s: &str| {
            if s.chars()
                .all(|c| c.is_alphanumeric() || "._-/+".contains(c))
            {
                s.to_string()
            } else {
                format!("'{}'", s.replace('\'', r"'\''"))
            }
        };
        let name = file.file_name()?.to_str()?;
        let stem = file.file_stem()?.to_str()?;
        let dir = file.parent()?.to_str()?;
        Some(
            template
                .replace("{file}", &quote(name))
                .replace("{stem}", &quote(stem))
                .replace("{dir}", &quote(dir)),
        )
    }
}

/// `F5`, `ctrl-b` → the editor's key.
pub fn parse_key(text: &str) -> Option<Key> {
    let lower = text.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f')
        && let Ok(n) = n.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Some(Key::F(n));
    }
    let c = lower.strip_prefix("ctrl-")?;
    let mut chars = c.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(Key::Ctrl(c)),
        _ => None,
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("fener/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_options_run_and_keys() {
        let config = Config::parse(
            r#"
            [editor]
            line-numbers = "relative"
            indent = 4

            [run]
            rs = "rustc {file} && ./{stem}"

            [keys]
            F9 = "run"
            ctrl-t = "terminal"
            "#,
        )
        .unwrap();
        assert_eq!(config.editor.line_numbers, LineNumbers::Relative);
        assert_eq!(config.editor.indent, 4);
        assert!(config.editor.tree_open, "missing fields keep their default");
        assert_eq!(
            config
                .run_command(Path::new("/x/my lab.rs"), crate::run::Goal::Run)
                .unwrap(),
            "rustc 'my lab.rs' && ./'my lab'"
        );
        let keymap = config.keymap();
        assert_eq!(keymap.get(&Key::F(9)), Some(&Command::Run));
        assert_eq!(keymap.get(&Key::Ctrl('t')), Some(&Command::Terminal));
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(
            Config::parse("[editor]\nindnet = 4")
                .unwrap_err()
                .contains("indnet")
        );
        assert!(
            Config::parse("[keys]\nF9 = \"rnu\"")
                .unwrap_err()
                .contains("rnu")
        );
        assert!(
            Config::parse("[keys]\nshift-q = \"run\"")
                .unwrap_err()
                .contains("shift-q")
        );
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }
}

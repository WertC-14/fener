//! The `:` and `/` lines: Ex commands (`:w`, `:q`, `:e`...) and search.

use std::path::Path;

use super::grammar::Motion;
use super::{Editor, Key, Mode};
use crate::command::Command;
use crate::text;

impl Editor {
    pub(super) fn cmdline_key(&mut self, key: Key) {
        match key {
            Key::Esc => self.mode = Mode::Normal,
            Key::Backspace if self.cmdline.is_empty() => self.mode = Mode::Normal,
            Key::Backspace => {
                self.cmdline.pop();
            }
            Key::Char(c) => self.cmdline.push(c),
            Key::Enter => {
                let line = std::mem::take(&mut self.cmdline);
                let search = self.mode == Mode::Search;
                self.mode = Mode::Normal;
                if search {
                    if !line.is_empty() {
                        self.last_search = Some(line);
                    }
                    if let Some(to) =
                        self.motion_target(Motion::SearchNext { reverse: false }, 1, None, false)
                    {
                        self.cursor = to;
                    }
                } else {
                    self.ex(line.trim());
                }
                self.clamp();
            }
            _ => {}
        }
    }

    /// Runs an Ex command (what follows `:`).
    pub(super) fn ex(&mut self, cmd: &str) {
        let (name, arg) = cmd
            .split_once(' ')
            .map_or((cmd, ""), |(n, a)| (n, a.trim()));
        let modified = self.doc.is_modified();
        match name {
            "w" | "write" => self.save((!arg.is_empty()).then_some(arg)),
            "q" | "quit" | "qa" | "qall" if modified && !self.shared => {
                self.message = Some("No write since last change (add ! to override)".into());
            }
            "q" | "quit" | "qa" | "qall" | "q!" | "quit!" | "qa!" | "qall!" => self.quit = true,
            "wq" | "wqa" | "x" | "xa" | "xit" => {
                if name.starts_with('x') && !modified {
                    self.quit = true;
                } else {
                    self.save((!arg.is_empty()).then_some(arg));
                    self.quit = !self.doc.is_modified();
                }
            }
            "e" | "edit" | "e!" | "edit!" => {
                let force = name.ends_with('!');
                match (arg, self.doc.path().map(Path::to_path_buf)) {
                    ("", Some(current)) => self.open(&current, Some(self.line()), force),
                    ("", None) => self.message = Some("No file name".into()),
                    (file, _) => self.open(Path::new(file), None, force),
                }
            }
            "Dashboard" => self.run_command(Command::Dashboard),
            "noh" | "nohlsearch" => self.highlight_search = false,
            "set" => match arg {
                "nu" | "number" => self.number = true,
                "nonu" | "nonumber" => self.number = false,
                "rnu" | "relativenumber" => self.relative_number = true,
                "nornu" | "norelativenumber" => self.relative_number = false,
                _ => self.message = Some(format!("Unknown option: {arg}")),
            },
            "" => {}
            n if n.chars().all(|c| c.is_ascii_digit()) => {
                let line = n.parse::<usize>().unwrap_or(1).saturating_sub(1);
                let line = line.min(text::line_count(&self.doc.rope) - 1);
                self.cursor = text::first_non_blank(&self.doc.rope, line);
            }
            _ => self.message = Some(format!("Not an editor command: {cmd}")),
        }
    }

    pub(super) fn save(&mut self, as_path: Option<&str>) {
        if let Some(path) = as_path {
            self.doc.set_path(path.into());
        }
        let name = self
            .doc
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.message = Some(match self.doc.save() {
            Ok(()) => format!("\"{name}\" {}L written", text::line_count(&self.doc.rope)),
            Err(e) => format!("Cannot write \"{name}\": {e}"),
        });
    }
}

//! Named commands (the Space menu, the dashboard, the keymaps picker), formatting (`Space c f`)
//! and running the file (F5).

use std::path::Path;

use super::{Editor, NOT_SAVED, Request};
use crate::command::{COMMANDS, Command, VIM_KEYS, bindings, leader_label};
use crate::document::Document;
use crate::picker::{Item, Kind, Pick, Picker};
use crate::text;

impl Editor {
    pub fn run_command(&mut self, command: Command) {
        match command {
            Command::FindFiles => {
                let mut p = Picker::new(Kind::Files, "Find Files", Vec::new());
                p.loading = true;
                self.picker = Some(p);
                self.requests.push(Request::ListFiles);
            }
            Command::RecentFiles => {
                let items = self
                    .state
                    .recent
                    .iter()
                    .filter(|p| p.exists())
                    .map(|path| Item {
                        text: self.display_path(path),
                        detail: String::new(),
                        pick: Pick::File {
                            path: path.clone(),
                            line: None,
                        },
                    })
                    .collect();
                self.picker = Some(Picker::new(Kind::Recent, "Recent Files", items));
            }
            Command::FindText => self.picker = Some(Picker::new(Kind::Grep, "Grep", Vec::new())),
            Command::NewFile => {
                if self.doc.is_modified() && !self.shared {
                    self.message = Some(NOT_SAVED.into());
                } else {
                    self.load(Document::new(""));
                    self.enter_insert();
                }
            }
            Command::Keymaps => {
                // The config file's keys first, so a changed key is easy to see.
                let mut user: Vec<Item> = self
                    .config
                    .keys
                    .iter()
                    .filter_map(|(key, name)| {
                        let command = Command::from_name(name)?;
                        let label = COMMANDS.iter().find(|b| b.command == command)?.label;
                        Some(Item {
                            text: format!("{label} (config)"),
                            detail: key.clone(),
                            pick: Pick::Command(command),
                        })
                    })
                    .collect();
                user.sort_by(|a, b| a.detail.cmp(&b.detail));
                let commands = bindings(self.context()).map(|b| Item {
                    text: b.label.into(),
                    detail: if b.leader.is_empty() {
                        String::new()
                    } else {
                        leader_label(b.leader)
                    },
                    pick: Pick::Command(b.command),
                });
                let vim = VIM_KEYS.iter().map(|(keys, label)| Item {
                    text: (*label).into(),
                    detail: (*keys).into(),
                    pick: Pick::Info,
                });
                let items = user.into_iter().chain(commands).chain(vim).collect();
                self.picker = Some(Picker::new(Kind::Keymaps, "Keymaps", items));
            }
            Command::Themes => {
                let items = self
                    .themes
                    .iter()
                    .enumerate()
                    .map(|(i, name)| Item {
                        text: (*name).into(),
                        detail: String::new(),
                        pick: Pick::Theme(i),
                    })
                    .collect();
                let mut p = Picker::new(Kind::Themes, "Colorschemes", items);
                p.selected = self.theme.min(p.matches.len().saturating_sub(1));
                p.theme_before = self.theme;
                self.picker = Some(p);
            }
            Command::Explorer => {
                if self.tree.is_some() {
                    self.tree = None;
                    self.tree_focus = false;
                } else {
                    self.show_tree(true);
                }
            }
            Command::SearchLines => {
                let path = self.doc.path().map(Path::to_path_buf);
                let items = (0..text::line_count(&self.doc.rope))
                    .map(|line| Item {
                        text: text::line_text(&self.doc.rope, line),
                        detail: (line + 1).to_string(),
                        pick: match &path {
                            Some(p) => Pick::File {
                                path: p.clone(),
                                line: Some(line),
                            },
                            None => Pick::Line(line),
                        },
                    })
                    .collect();
                self.picker = Some(Picker::new(Kind::Lines, "Find in File", items));
            }
            Command::Headings => {
                let path = self.doc.path().map(Path::to_path_buf);
                let mut fence = false;
                let mut items = Vec::new();
                for line in 0..text::line_count(&self.doc.rope) {
                    let content = text::line_text(&self.doc.rope, line);
                    let trimmed = content.trim_start();
                    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                        fence = !fence;
                    } else if !fence && trimmed.starts_with('#') {
                        let level = trimmed.chars().take_while(|&c| c == '#').count();
                        items.push(Item {
                            // Indented by level, without the #s: an outline.
                            text: format!(
                                "{}{}",
                                "  ".repeat(level.saturating_sub(1)),
                                trimmed[level..].trim()
                            ),
                            detail: (line + 1).to_string(),
                            pick: match &path {
                                Some(p) => Pick::File {
                                    path: p.clone(),
                                    line: Some(line),
                                },
                                None => Pick::Line(line),
                            },
                        });
                    }
                }
                self.picker = Some(Picker::new(Kind::Lines, "Headings", items));
            }
            Command::Run => self.run_file(crate::run::Goal::Run),
            Command::Build => self.run_file(crate::run::Goal::Build),
            Command::Terminal => self.requests.push(Request::ToggleTerminal),
            Command::Lazygit => self.requests.push(Request::Lazygit),
            Command::Format => self.format_request(),
            Command::SplitRight => self.requests.push(Request::Split { vertical: true }),
            Command::SplitBelow => self.requests.push(Request::Split { vertical: false }),
            Command::CloseWindow => self.requests.push(Request::CloseWindow),
            Command::RestoreSession => self.requests.push(Request::RestoreSession),
            Command::Dashboard => self.dashboard = Some(0),
            Command::Save => self.save(None),
            Command::Quit => self.ex("qa"),
            Command::ToggleReader => {
                self.reader = !self.reader;
                if self.reader {
                    self.reader_top = self.top;
                } else {
                    self.leave_reader();
                }
            }
            Command::ToggleNumbers => self.number = !self.number,
            Command::ToggleRelativeNumbers => self.relative_number = !self.relative_number,
            Command::ClearSearch => self.highlight_search = false,
        }
    }

    fn format_request(&mut self) {
        let Some(path) = self.doc.path().map(Path::to_path_buf) else {
            self.message = Some("Save the file first (:w name)".into());
            return;
        };
        let file = std::path::absolute(&path).unwrap_or(path);
        let configured = file
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|e| self.config.format.get(e))
            .map(|t| crate::format::fill(t, &file));
        match configured.or_else(|| crate::format::command_for(&file)) {
            Some(command) => {
                self.message = Some(format!("Formatting: {command}"));
                self.requests.push(Request::Format {
                    command,
                    dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
                    version: self.doc.version(),
                });
            }
            None => {
                self.message =
                    Some("No formatter for this kind of file ([format] in the config)".into())
            }
        }
    }

    /// The formatter's answer: the new text (one undo step; the cursor stays on its line and
    /// column), or its error. Dropped when the text changed since the request.
    pub fn formatted(&mut self, version: u64, result: Result<String, String>) {
        if version != self.doc.version() {
            return;
        }
        match result {
            Ok(text) if self.doc.rope == text.as_str() => {
                self.message = Some("Already formatted".into());
            }
            Ok(text) => {
                let (line, col) = (self.line(), self.col());
                self.doc.begin_step(self.cursor);
                self.doc.edit(0, self.doc.rope.len_chars(), &text);
                let last = text::line_count(&self.doc.rope).saturating_sub(1);
                self.cursor = text::at_col(&self.doc.rope, line.min(last), col, false);
                self.doc.end_step(self.cursor);
                self.clamp();
                self.message = Some("Formatted".into());
            }
            Err(e) => self.message = Some(format!("Format failed: {e}")),
        }
    }

    /// F5 / `Space Enter` / `Space b`: saves, then asks the app to type the file's run (or
    /// build) command in the terminal. The config file's `[run]` / `[build]` comes first.
    fn run_file(&mut self, goal: crate::run::Goal) {
        let Some(path) = self.doc.path().map(Path::to_path_buf) else {
            self.message = Some("Save the file first (:w name)".into());
            return;
        };
        let file = std::path::absolute(&path).unwrap_or(path);
        let configured = self
            .config
            .run_command(&file, goal)
            .map(|command| crate::run::Run {
                dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
                command,
            });
        match configured.or_else(|| crate::run::command_for(&file, goal)) {
            Some(run) => {
                if self.doc.is_modified() {
                    self.save(None);
                }
                self.requests.push(Request::Run(run));
            }
            None => self.message = Some("No run command for this kind of file".into()),
        }
    }
}

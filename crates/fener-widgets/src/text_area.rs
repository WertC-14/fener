//! The text itself: line numbers, syntax colors, the selection, search matches, diagnostics
//! and the cursor; and the Markdown reader in its place.

use fener_core::syntax::{Kind as TextKind, Lang};
use fener_core::{Editor, highlight, lsp, syntax, text};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::View;
use crate::markdown;
use crate::theme::t;

/// Cells a tab takes (LazyVim: `tabstop = 2`).
const TAB_WIDTH: usize = 2;

/// tree-sitter's spans for one version of a document.
type Colors = (u64, std::sync::Arc<Vec<highlight::Span>>);

/// Runs tree-sitter for the newest version of a document on a worker thread and keeps the
/// latest result.
#[derive(Default)]
pub struct Painter {
    /// The version asked for last; a worker that sees a newer one gives up.
    wanted: std::sync::Arc<std::sync::atomic::AtomicU64>,
    asked: Option<u64>,
    /// The file the colors are for; another file starts over.
    path: Option<std::path::PathBuf>,
    done: std::sync::Arc<std::sync::Mutex<Option<Colors>>>,
}

impl Painter {
    fn colors(
        &mut self,
        grammar: highlight::Grammar,
        path: &std::path::Path,
        version: u64,
        rope: &ropey::Rope,
        on_ready: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    ) -> Option<std::sync::Arc<Vec<highlight::Span>>> {
        use std::sync::atomic::Ordering;
        if self.path.as_deref() != Some(path) {
            // Fresh slots: a worker still busy with the old file writes into the old ones.
            *self = Self {
                path: Some(path.to_path_buf()),
                ..Self::default()
            };
        }
        if self.asked != Some(version) {
            self.asked = Some(version);
            self.wanted.store(version, Ordering::Relaxed);
            let (wanted, done) = (self.wanted.clone(), self.done.clone());
            let text = rope.to_string();
            std::thread::spawn(move || {
                if wanted.load(Ordering::Relaxed) != version {
                    return; // typed on meanwhile: a newer job follows
                }
                let Some(spans) = highlight::highlight(grammar, &text) else {
                    return;
                };
                if let Ok(mut slot) = done.lock()
                    && slot.as_ref().is_none_or(|(v, _)| *v < version)
                {
                    *slot = Some((version, std::sync::Arc::new(spans)));
                }
                if let Some(wake) = on_ready {
                    wake();
                }
            });
        }
        self.done.lock().ok()?.as_ref().map(|(_, s)| s.clone())
    }
}

fn line_number_width(editor: &Editor) -> usize {
    if !editor.number && !editor.relative_number {
        return 0;
    }
    let lines = text::line_count(&editor.doc.rope);
    lines.to_string().len().max(3) + 1
}

/// Screen width of a character starting at screen column `col` (tabs reach the next stop).
fn cell_width(c: char, col: usize) -> usize {
    if c == '\t' {
        TAB_WIDTH - col % TAB_WIDTH
    } else {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

pub fn draw_text(
    buf: &mut Buffer,
    area: Rect,
    editor: &Editor,
    view: &mut View,
) -> Option<(u16, u16)> {
    let rope = &editor.doc.rope;
    let gutter = line_number_width(editor);
    let width = usize::from(area.width).saturating_sub(gutter + 1).max(1);
    let lines = text::line_count(rope);
    let cursor_line = editor.line();
    let selection = editor.selection();
    // Keep the cursor's screen column visible, with a margin (LazyVim: `sidescrolloff = 8`).
    let cursor_screen_col = {
        let start = text::line_start(rope, cursor_line);
        rope.slice(start..editor.cursor)
            .chars()
            .fold(0, |col, c| col + cell_width(c, col))
    };
    let margin = 8.min(width / 2);
    if cursor_screen_col < view.left + margin {
        view.left = cursor_screen_col.saturating_sub(margin);
    } else if cursor_screen_col + margin >= view.left + width {
        view.left = cursor_screen_col + margin + 1 - width;
    }
    let matches = search_matches(editor, area.height);
    // Diagnostics on screen as char ranges, and the worst one (and its message) per line.
    let diags: Vec<(usize, usize, lsp::Severity, usize, &str)> = editor
        .lsp
        .diagnostics
        .iter()
        .map(|d| {
            let from = lsp::to_char(rope, d.start, editor.lsp.encoding);
            let to = lsp::to_char(rope, d.end, editor.lsp.encoding).max(from + 1);
            (from, to, d.severity, d.start.line, d.message.as_str())
        })
        .collect();
    let worst_on = |line: usize| {
        diags
            .iter()
            .filter(|d| d.3 == line && d.2 <= lsp::Severity::Warning)
            .min_by_key(|d| d.2)
    };
    let lang = editor.doc.path().map_or(Lang::Plain, Lang::from_path);
    // tree-sitter when the language has a grammar. Until the worker has colors for this version,
    // the last ones are used (a few characters may be off for a moment while typing).
    let grammar = editor.doc.path().and_then(highlight::Grammar::from_path);
    let spans = grammar.zip(editor.doc.path()).and_then(|(g, path)| {
        let on_ready = view.on_ready.clone();
        view.tree_sitter
            .colors(g, path, editor.doc.version(), rope, on_ready)
    });
    let spans = spans.as_deref();
    let key = (editor.doc.version(), editor.top);
    let mut state = match view.syntax {
        Some((version, top, state)) if (version, top) == key => state,
        _ => syntax::state_after(lang, rope, editor.top),
    };
    view.syntax = Some((key.0, key.1, state));
    let mut cursor = None;
    for row in 0..usize::from(area.height) {
        let line = editor.top + row;
        let y = area.y + row as u16;
        let row_area = Rect::new(area.x, y, area.width, 1);
        if line >= lines {
            buf.set_string(
                area.x + gutter as u16,
                y,
                "~",
                Style::new().fg(t().gutter).bg(t().bg),
            );
            continue;
        }
        let is_cursor_line = line == cursor_line;
        if is_cursor_line {
            buf.set_style(row_area, Style::new().bg(t().bg_line));
        }
        if gutter > 0 {
            let (label, style) = if is_cursor_line {
                let n = (line + 1).to_string();
                (
                    format!("{n:<w$} ", w = gutter - 1),
                    Style::new().fg(t().orange).add_modifier(Modifier::BOLD),
                )
            } else if let Some(d) = worst_on(line) {
                (
                    format!("{:>w$} ", line + 1, w = gutter - 1),
                    Style::new()
                        .fg(severity_color(d.2))
                        .add_modifier(Modifier::BOLD),
                )
            } else if editor.relative_number {
                (
                    format!("{:>w$} ", line.abs_diff(cursor_line), w = gutter - 1),
                    Style::new().fg(t().gutter),
                )
            } else {
                (
                    format!("{:>w$} ", line + 1, w = gutter - 1),
                    Style::new().fg(t().gutter),
                )
            };
            buf.set_string(area.x, y, label, style);
        }
        // The characters of the line, scrolled sideways by `view.left`.
        let start = text::line_start(rope, line);
        let len = text::line_len(rope, line);
        let x0 = area.x + gutter as u16 + 1;
        let content = text::line_text(rope, line);
        let scanned = syntax::line_kinds(lang, &content, &mut state);
        let kinds = match spans {
            Some(spans) => kinds_from_spans(spans, rope.char_to_byte(start), &content),
            None => scanned,
        };
        let mut col = 0;
        for (i, c) in rope.slice(start..start + len).chars().enumerate() {
            let pos = start + i;
            let w = cell_width(c, col);
            let visible = col >= view.left && col + w <= view.left + width;
            if visible {
                let x = x0 + (col - view.left) as u16;
                let mut style = kind_style(kinds.get(i).copied().unwrap_or(TextKind::Plain));
                if matches.iter().any(|&(a, b)| pos >= a && pos < b) {
                    style = style.bg(t().bg_search);
                }
                if selection.is_some_and(|(a, b)| pos >= a && pos < b) {
                    style = style.bg(t().bg_visual);
                }
                // A diagnostic's range is underlined in its color (LazyVim: undercurl).
                if let Some(d) = diags
                    .iter()
                    .filter(|d| pos >= d.0 && pos < d.1 && d.2 <= lsp::Severity::Warning)
                    .min_by_key(|d| d.2)
                {
                    style = style
                        .add_modifier(Modifier::UNDERLINED)
                        .underline_color(severity_color(d.2));
                }
                let symbol = if c == '\t' {
                    " ".repeat(w)
                } else {
                    c.to_string()
                };
                buf.set_string(x, y, symbol, style);
            }
            if pos == editor.cursor && visible {
                cursor = Some((x0 + (col - view.left) as u16, y));
            }
            col += w;
        }
        // An empty line in a Visual Line selection still shows one selected cell.
        if len == 0 && selection.is_some_and(|(a, b)| start >= a && start <= b) && view.left == 0 {
            buf.set_style(Rect::new(x0, y, 1, 1), Style::new().bg(t().bg_visual));
        }
        // The line's worst diagnostic after its end, as LazyVim's virtual text.
        if let Some(d) = worst_on(line) {
            let at = col.saturating_sub(view.left) + 3;
            if at + 4 < width {
                let text = format!("● {}", d.4.lines().next().unwrap_or(""));
                buf.set_stringn(
                    x0 + at as u16,
                    y,
                    text,
                    width - at,
                    Style::new()
                        .fg(severity_color(d.2))
                        .add_modifier(Modifier::ITALIC),
                );
            }
        }
        if is_cursor_line && editor.cursor == start + len {
            // On the end of the line (Insert mode, or an empty line).
            let x = col.saturating_sub(view.left);
            if x < width {
                cursor = Some((x0 + x as u16, y));
            }
        }
    }
    cursor
}

/// The Markdown reader: formatted lines from `reader_top`, wrapped at words, in a column of
/// at most 100 cells.
pub fn draw_reader(buf: &mut Buffer, area: Rect, editor: &Editor) {
    let rope = &editor.doc.rope;
    let width = usize::from(area.width.saturating_sub(4)).clamp(10, 100);
    let x = area.x + 2;
    let in_code = syntax::state_after(Lang::Markdown, rope, editor.reader_top).in_fence();
    let lines = (editor.reader_top..text::line_count(rope)).map(|line| text::line_text(rope, line));
    let rows = markdown::layout(lines, in_code, width, usize::from(area.height));
    for (row, y) in rows.into_iter().zip(area.top()..area.bottom()) {
        row.render(Rect::new(x, y, width as u16, 1), buf);
    }
}

/// The kind of each character of a line from tree-sitter's spans (`line_byte`: where the line
/// starts in the text, in bytes).
fn kinds_from_spans(spans: &[highlight::Span], line_byte: usize, content: &str) -> Vec<TextKind> {
    let mut kinds = Vec::with_capacity(content.len());
    let line_end = line_byte + content.len();
    let mut i = spans.partition_point(|s| s.end <= line_byte);
    let mut byte = line_byte;
    for c in content.chars() {
        while i < spans.len() && spans[i].end <= byte {
            i += 1;
        }
        let kind = match spans.get(i) {
            Some(s) if s.start <= byte && byte < s.end && s.start < line_end => s.kind,
            _ => TextKind::Plain,
        };
        kinds.push(kind);
        byte += c.len_utf8();
    }
    kinds
}

pub fn severity_color(severity: lsp::Severity) -> Color {
    match severity {
        lsp::Severity::Error => t().red,
        lsp::Severity::Warning => t().yellow,
        lsp::Severity::Info => t().blue,
        lsp::Severity::Hint => t().fg_dim,
    }
}

/// The color of a kind of text (tokyonight's choices: keywords purple, functions blue,
/// types cyan, strings green, numbers orange, comments dim and italic).
fn kind_style(kind: TextKind) -> Style {
    let t = t();
    match kind {
        TextKind::Plain => Style::new().fg(t.fg),
        TextKind::Comment => Style::new().fg(t.fg_dim).add_modifier(Modifier::ITALIC),
        TextKind::Str => Style::new().fg(t.green),
        TextKind::Number => Style::new().fg(t.orange),
        TextKind::Keyword => Style::new().fg(t.magenta),
        TextKind::Type => Style::new().fg(t.cyan),
        TextKind::Function => Style::new().fg(t.blue),
        TextKind::Heading => Style::new().fg(t.blue).add_modifier(Modifier::BOLD),
        TextKind::Marker => Style::new().fg(t.orange),
    }
}

/// Ranges of `last_search` on the lines on screen (when matches are shown).
fn search_matches(editor: &Editor, height: u16) -> Vec<(usize, usize)> {
    let Some(pattern) = editor
        .last_search
        .as_deref()
        .filter(|_| editor.highlight_search)
    else {
        return Vec::new();
    };
    if pattern.is_empty() {
        return Vec::new();
    }
    let rope = &editor.doc.rope;
    let ignore_case = !pattern.chars().any(char::is_uppercase);
    let needle: Vec<char> = if ignore_case {
        pattern.to_lowercase().chars().collect()
    } else {
        pattern.chars().collect()
    };
    let mut out = Vec::new();
    let last = (editor.top + usize::from(height)).min(text::line_count(rope));
    for line in editor.top..last {
        let start = text::line_start(rope, line);
        let chars: Vec<char> = if ignore_case {
            text::line_text(rope, line).to_lowercase().chars().collect()
        } else {
            text::line_text(rope, line).chars().collect()
        };
        for i in 0..chars.len().saturating_sub(needle.len() - 1) {
            if chars[i..i + needle.len()] == needle[..] {
                out.push((start + i, start + i + needle.len()));
            }
        }
    }
    out
}

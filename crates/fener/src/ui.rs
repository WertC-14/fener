//! Draws the editor: the text with line numbers, the status line, the command line and the
//! which-key box. Colors are LazyVim's default theme (tokyonight "night").

use fener_core::text;
use fener_core::{Editor, Mode};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};

const BG: Color = Color::Rgb(0x1a, 0x1b, 0x26);
const BG_DARK: Color = Color::Rgb(0x16, 0x16, 0x1e);
const BG_LINE: Color = Color::Rgb(0x29, 0x2e, 0x42);
const BG_VISUAL: Color = Color::Rgb(0x28, 0x34, 0x57);
const BG_SEARCH: Color = Color::Rgb(0x3d, 0x59, 0xa1);
const FG: Color = Color::Rgb(0xc0, 0xca, 0xf5);
const FG_DIM: Color = Color::Rgb(0x56, 0x5f, 0x89);
const GUTTER: Color = Color::Rgb(0x3b, 0x42, 0x61);
const CURRENT_NR: Color = Color::Rgb(0xff, 0x9e, 0x64);
const BLUE: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
const GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);
const MAGENTA: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
const YELLOW: Color = Color::Rgb(0xe0, 0xaf, 0x68);
const RED: Color = Color::Rgb(0xf7, 0x76, 0x8e);
const CYAN: Color = Color::Rgb(0x7d, 0xcf, 0xff);

/// Cells a tab takes (LazyVim: `tabstop = 2`).
const TAB_WIDTH: usize = 2;

/// What the UI keeps between frames.
#[derive(Default)]
pub struct View {
    /// First screen column shown (no wrapping: long lines scroll sideways).
    pub left: usize,
}

/// Draws a frame; returns where the terminal cursor goes.
pub fn render(frame: &mut Frame, editor: &mut Editor, view: &mut View) -> Option<(u16, u16)> {
    let [body, status, command] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(Block::new().style(Style::new().bg(BG)), frame.area());
    editor.scroll_to_cursor(usize::from(body.height));
    let cursor = draw_text(frame.buffer_mut(), body, editor, view);
    draw_status(frame, status, editor);
    let command_cursor = draw_command_line(frame, command, editor);
    if let Some(menu) = editor.leader_menu() {
        draw_which_key(frame, body, &menu, &editor.pending_keys());
    }
    command_cursor.or(cursor)
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

fn draw_text(buf: &mut Buffer, area: Rect, editor: &Editor, view: &mut View) -> Option<(u16, u16)> {
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
                Style::new().fg(GUTTER).bg(BG),
            );
            continue;
        }
        let is_cursor_line = line == cursor_line;
        if is_cursor_line {
            buf.set_style(row_area, Style::new().bg(BG_LINE));
        }
        if gutter > 0 {
            let (label, style) = if is_cursor_line {
                let n = (line + 1).to_string();
                (
                    format!("{n:<w$} ", w = gutter - 1),
                    Style::new().fg(CURRENT_NR).add_modifier(Modifier::BOLD),
                )
            } else if editor.relative_number {
                (
                    format!("{:>w$} ", line.abs_diff(cursor_line), w = gutter - 1),
                    Style::new().fg(GUTTER),
                )
            } else {
                (
                    format!("{:>w$} ", line + 1, w = gutter - 1),
                    Style::new().fg(GUTTER),
                )
            };
            buf.set_string(area.x, y, label, style);
        }
        // The characters of the line, scrolled sideways by `view.left`.
        let start = text::line_start(rope, line);
        let len = text::line_len(rope, line);
        let x0 = area.x + gutter as u16 + 1;
        let mut col = 0;
        for (i, c) in rope.slice(start..start + len).chars().enumerate() {
            let pos = start + i;
            let w = cell_width(c, col);
            let visible = col >= view.left && col + w <= view.left + width;
            if visible {
                let x = x0 + (col - view.left) as u16;
                let mut style = Style::new().fg(FG);
                if matches.iter().any(|&(a, b)| pos >= a && pos < b) {
                    style = style.bg(BG_SEARCH);
                }
                if selection.is_some_and(|(a, b)| pos >= a && pos < b) {
                    style = style.bg(BG_VISUAL);
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
            buf.set_style(Rect::new(x0, y, 1, 1), Style::new().bg(BG_VISUAL));
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

fn mode_label(mode: Mode) -> (&'static str, Color) {
    match mode {
        Mode::Normal => ("NORMAL", BLUE),
        Mode::Insert => ("INSERT", GREEN),
        Mode::Visual => ("VISUAL", MAGENTA),
        Mode::VisualLine => ("V-LINE", MAGENTA),
        Mode::Command | Mode::Search => ("COMMAND", YELLOW),
    }
}

/// lualine-like: the mode in its color, the file, and on the right pending keys, position, %.
fn draw_status(frame: &mut Frame, area: Rect, editor: &Editor) {
    let (mode, color) = mode_label(editor.mode);
    let name = editor
        .doc
        .path()
        .map_or_else(|| "[No Name]".to_string(), |p| p.display().to_string());
    let modified = if editor.doc.is_modified() { " ●" } else { "" };
    let left = Line::from(vec![
        Span::styled(
            format!(" {mode} "),
            Style::new()
                .fg(BG_DARK)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {name}"), Style::new().fg(FG)),
        Span::styled(modified, Style::new().fg(YELLOW)),
    ]);
    let lines = text::line_count(&editor.doc.rope);
    let percent = match editor.line() {
        0 => "Top".to_string(),
        l if l + 1 == lines => "Bot".to_string(),
        l => format!("{}%", (l + 1) * 100 / lines),
    };
    let right = Line::from(vec![
        Span::styled(
            format!("{}  ", editor.pending_keys()),
            Style::new().fg(FG_DIM),
        ),
        Span::styled(format!(" {percent} "), Style::new().fg(color).bg(BG_LINE)),
        Span::styled(
            format!(" {}:{} ", editor.line() + 1, editor.col() + 1),
            Style::new()
                .fg(BG_DARK)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .right_aligned();
    frame.render_widget(Block::new().style(Style::new().bg(BG_DARK)), area);
    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(Paragraph::new(right), area);
}

/// The `:` / `/` line while typing, otherwise the last message. Returns the cursor there.
fn draw_command_line(frame: &mut Frame, area: Rect, editor: &Editor) -> Option<(u16, u16)> {
    let prefix = match editor.mode {
        Mode::Command => ":",
        Mode::Search => "/",
        _ => {
            if let Some(message) = &editor.message {
                let color = if message.starts_with("Not ")
                    || message.starts_with("No ")
                    || message.starts_with("Cannot")
                    || message.starts_with("Pattern")
                {
                    RED
                } else {
                    FG
                };
                frame.render_widget(
                    Paragraph::new(message.as_str()).style(Style::new().fg(color).bg(BG)),
                    area,
                );
            }
            return None;
        }
    };
    let text = format!("{prefix}{}", editor.cmdline);
    let width = unicode_width::UnicodeWidthStr::width(text.as_str()) as u16;
    frame.render_widget(Paragraph::new(text).style(Style::new().fg(FG).bg(BG)), area);
    Some((area.x + width.min(area.width.saturating_sub(1)), area.y))
}

/// which-key: the keys that can follow, in a box at the bottom right.
fn draw_which_key(frame: &mut Frame, area: Rect, menu: &[(char, &str)], typed: &str) {
    let width = menu
        .iter()
        .map(|(_, l)| l.chars().count())
        .max()
        .unwrap_or(0) as u16
        + 10;
    let height = menu.len() as u16 + 2;
    let rect = Rect::new(
        area.right().saturating_sub(width + 1),
        area.bottom().saturating_sub(height),
        width.min(area.width),
        height.min(area.height),
    );
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BLUE))
        .title(Span::styled(
            format!(" {} ", typed.trim()),
            Style::new().fg(CYAN),
        ))
        .style(Style::new().bg(BG_DARK));
    let inner = block.inner(rect);
    block.render(rect, frame.buffer_mut());
    let lines: Vec<Line> = menu
        .iter()
        .map(|(key, label)| {
            let group = label.starts_with('+');
            Line::from(vec![
                Span::styled(
                    format!(" {key} "),
                    Style::new().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled("➜ ", Style::new().fg(FG_DIM)),
                Span::styled(*label, Style::new().fg(if group { MAGENTA } else { FG })),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

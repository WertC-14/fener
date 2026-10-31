//! The two bottom lines: the status line (mode, file, position) and the `:` / `/` line.

use fener_core::{Editor, Mode, text};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};

use crate::text_area::severity_color;
use crate::theme::t;

fn mode_label(mode: Mode) -> (&'static str, Color) {
    match mode {
        Mode::Normal => ("NORMAL", t().blue),
        Mode::Insert => ("INSERT", t().green),
        Mode::Visual => ("VISUAL", t().magenta),
        Mode::VisualLine => ("V-LINE", t().magenta),
        Mode::Command | Mode::Search => ("COMMAND", t().yellow),
    }
}

/// lualine-like: the mode in its color, the file, and on the right pending keys, position, %.
pub fn draw_status(buf: &mut Buffer, area: Rect, editor: &Editor) {
    let (mode, color) = if editor.reader {
        ("READ", t().cyan)
    } else {
        mode_label(editor.mode)
    };
    let name = editor
        .doc
        .path()
        .map_or_else(|| "[No Name]".to_string(), |p| editor.display_path(p));
    let modified = if editor.doc.is_modified() { " ●" } else { "" };
    let left = Line::from(vec![
        Span::styled(
            format!(" {mode} "),
            Style::new()
                .fg(t().bg_dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {name}"), Style::new().fg(t().fg)),
        Span::styled(modified, Style::new().fg(t().yellow)),
    ]);
    let lines = text::line_count(&editor.doc.rope);
    // The reader scrolls without moving the cursor: its place is the line at the top.
    let line = if editor.reader {
        editor.reader_top
    } else {
        editor.line()
    };
    let percent = match line {
        0 => "Top".to_string(),
        l if l + 1 == lines => "Bot".to_string(),
        l => format!("{}%", (l + 1) * 100 / lines),
    };
    // Markdown: the key between reading and editing, where the eye already is.
    let hint = match (editor.context(), editor.reader) {
        (fener_core::command::When::Markdown, true) => "Ctrl+E edit  ",
        (fener_core::command::When::Markdown, false) => "Ctrl+E read  ",
        _ => "",
    };
    let right = Line::from(vec![
        Span::styled(
            format!("{}  {hint}", editor.pending_keys()),
            Style::new().fg(t().fg_dim),
        ),
        Span::styled(
            format!(" {percent} "),
            Style::new().fg(color).bg(t().bg_line),
        ),
        Span::styled(
            if editor.reader {
                format!(" {}/{lines} ", line + 1)
            } else {
                format!(" {}:{} ", line + 1, editor.col() + 1)
            },
            Style::new()
                .fg(t().bg_dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .right_aligned();
    Widget::render(Block::new().style(Style::new().bg(t().bg_dark)), area, buf);
    Widget::render(Paragraph::new(left), area, buf);
    Widget::render(Paragraph::new(right), area, buf);
}

/// The `:` / `/` line while typing, otherwise the last message. Returns the cursor there.
pub fn draw_command_line(buf: &mut Buffer, area: Rect, editor: &Editor) -> Option<(u16, u16)> {
    let prefix = match editor.mode {
        Mode::Command => ":",
        Mode::Search => "/",
        _ => {
            let diagnostic = editor
                .lsp
                .diagnostics
                .iter()
                .filter(|d| d.start.line == editor.line())
                .min_by_key(|d| d.severity);
            if editor.message.is_none()
                && let Some(d) = diagnostic
            {
                let text = d.message.lines().next().unwrap_or("");
                Paragraph::new(text)
                    .style(Style::new().fg(severity_color(d.severity)).bg(t().bg))
                    .render(area, buf);
                return None;
            }
            if let Some(message) = &editor.message {
                let color = if message.starts_with("Not ")
                    || message.starts_with("No ")
                    || message.starts_with("Cannot")
                    || message.starts_with("Pattern")
                {
                    t().red
                } else {
                    t().fg
                };
                Widget::render(
                    Paragraph::new(message.as_str()).style(Style::new().fg(color).bg(t().bg)),
                    area,
                    buf,
                );
            }
            return None;
        }
    };
    let text = format!("{prefix}{}", editor.cmdline);
    let width = unicode_width::UnicodeWidthStr::width(text.as_str()) as u16;
    Widget::render(
        Paragraph::new(text).style(Style::new().fg(t().fg).bg(t().bg)),
        area,
        buf,
    );
    Some((area.x + width.min(area.width.saturating_sub(1)), area.y))
}

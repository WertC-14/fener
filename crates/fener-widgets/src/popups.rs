//! Boxes over the text: hover (`K`), the completion menu and which-key.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};

use crate::markdown;
use crate::theme::t;

/// `K`'s answer in a box under (or over) the cursor, as formatted Markdown.
pub fn draw_hover(buf: &mut Buffer, area: Rect, cursor: (u16, u16), text: &str) {
    let width = area.width.saturating_sub(4).min(80);
    if width < 20 {
        return;
    }
    let inner_width = usize::from(width - 2);
    // Compact: code without its fence frame, blank lines merged.
    let mut in_code = false;
    let mut rows: Vec<Line> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        let spans = if in_code {
            vec![Span::styled(line.to_string(), Style::new().fg(t().green))]
        } else {
            let mut plain = false;
            markdown::line(line, &mut plain, inner_width)
        };
        let blank = line.trim().is_empty();
        if blank && rows.last().is_none_or(|l| l.width() == 0) {
            continue;
        }
        rows.extend(markdown::wrap(spans, inner_width));
    }
    while rows.last().is_some_and(|l| l.width() == 0) {
        rows.pop();
    }
    let height = (rows.len() as u16 + 2)
        .min(area.height.saturating_sub(2) / 2)
        .max(3);
    let below = cursor.1 + 1 + height <= area.bottom();
    let y = if below {
        cursor.1 + 1
    } else {
        cursor.1.saturating_sub(height)
    };
    let x = cursor.0.min(area.right().saturating_sub(width)).max(area.x);
    let rect = Rect::new(x, y, width, height);
    Clear.render(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t().blue))
        .style(Style::new().bg(t().bg_dark));
    let inner = block.inner(rect);
    block.render(rect, buf);
    for (i, row) in rows.into_iter().take(usize::from(inner.height)).enumerate() {
        row.render(Rect::new(inner.x, inner.y + i as u16, inner.width, 1), buf);
    }
}

/// The completion menu under the word being typed.
pub fn draw_completion(
    buf: &mut Buffer,
    area: Rect,
    cursor: (u16, u16),
    menu: &fener_core::CompletionMenu,
    typed: usize,
) {
    let rows = menu.shown.len().min(10);
    if rows == 0 {
        return;
    }
    let first = menu.selected.saturating_sub(rows - 1);
    let label_w = menu
        .shown
        .iter()
        .map(|&i| menu.items[i].label.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    let width = (label_w + 30).min(usize::from(area.width).saturating_sub(2)) as u16;
    let height = rows as u16 + 2;
    let x = cursor
        .0
        .saturating_sub(typed as u16 + 1)
        .min(area.right().saturating_sub(width));
    let y = if cursor.1 + 1 + height <= area.bottom() {
        cursor.1 + 1
    } else {
        cursor.1.saturating_sub(height)
    };
    let rect = Rect::new(x.max(area.x), y, width, height);
    Clear.render(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t().gutter))
        .style(Style::new().bg(t().bg_dark));
    let inner = block.inner(rect);
    block.render(rect, buf);
    for (row, (n, &i)) in menu
        .shown
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .enumerate()
    {
        let item = &menu.items[i];
        let y = inner.y + row as u16;
        let chosen = n == menu.selected;
        let bg = if chosen { t().bg_line } else { t().bg_dark };
        buf.set_style(Rect::new(inner.x, y, inner.width, 1), Style::new().bg(bg));
        let line = Line::from(vec![
            Span::styled(format!("{:<6}", item.kind), Style::new().fg(t().magenta)),
            Span::styled(
                format!("{:<w$} ", item.label, w = label_w),
                Style::new().fg(if chosen { t().blue } else { t().fg }),
            ),
            Span::styled(item.detail.clone(), Style::new().fg(t().fg_dim)),
        ]);
        line.render(Rect::new(inner.x, y, inner.width, 1), buf);
    }
}

/// which-key: the keys that can follow, in a box at the bottom right.
pub fn draw_which_key(buf: &mut Buffer, area: Rect, menu: &[(char, &str)], typed: &str) {
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
    Widget::render(Clear, rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t().blue))
        .title(Span::styled(
            format!(" {} ", typed.trim()),
            Style::new().fg(t().cyan),
        ))
        .style(Style::new().bg(t().bg_dark));
    let inner = block.inner(rect);
    block.render(rect, buf);
    let lines: Vec<Line> = menu
        .iter()
        .map(|(key, label)| {
            let group = label.starts_with('+');
            Line::from(vec![
                Span::styled(
                    format!(" {} ", if *key == ' ' { '␣' } else { *key }),
                    Style::new().fg(t().cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled("➜ ", Style::new().fg(t().fg_dim)),
                Span::styled(
                    *label,
                    Style::new().fg(if group { t().magenta } else { t().fg }),
                ),
            ])
        })
        .collect();
    Widget::render(Paragraph::new(lines), inner, buf);
}

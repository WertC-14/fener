//! Pickers (Find Files, grep, recent files, keymaps, themes...): the query, the list and a
//! preview of the selected item.

use fener_core::picker::{Kind, Picker};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};

use crate::icons::{file_icon, nerd};
use crate::theme::t;

/// A picker as a floating box: the query on top, the matching items below, the selected one
/// marked. Returns the cursor (at the end of the query).
pub fn draw_picker(buf: &mut Buffer, area: Rect, picker: &Picker) -> Option<(u16, u16)> {
    let width = area.width.saturating_sub(4).min(100);
    // Short fixed lists get a box their size, so what is behind (a theme being tried) shows.
    let fit = match picker.kind {
        Kind::Themes | Kind::Recent => picker.items.len().max(1) as u16 + 4,
        _ => 26,
    };
    let height = area.height.saturating_sub(4).min(26).min(fit);
    if width < 20 || height < 5 {
        return None;
    }
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    Clear.render(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t().blue))
        .title(Span::styled(
            format!(" {} ", picker.title),
            Style::new().fg(t().orange),
        ))
        .style(Style::new().bg(t().bg_dark));
    let inner = block.inner(rect);
    block.render(rect, buf);

    // The query line, with "12/340" on the right.
    let y = inner.y;
    let arrow = if nerd() { "\u{f054} " } else { "> " };
    buf.set_string(inner.x + 1, y, arrow, Style::new().fg(t().cyan));
    let prompt = 3;
    buf.set_string(inner.x + prompt, y, &picker.query, Style::new().fg(t().fg));
    let count = if picker.loading {
        "…".to_string()
    } else {
        format!("{}/{}", picker.matches.len(), picker.items.len())
    };
    let count_w = count.chars().count() as u16;
    buf.set_string(
        inner.right().saturating_sub(count_w + 1),
        y,
        &count,
        Style::new().fg(t().fg_dim),
    );
    let query_w = unicode_width::UnicodeWidthStr::width(picker.query.as_str()) as u16;
    let cursor = (
        (inner.x + prompt + query_w).min(inner.right().saturating_sub(1)),
        y,
    );
    buf.set_string(
        inner.x,
        y + 1,
        "─".repeat(usize::from(inner.width)),
        Style::new().fg(t().gutter),
    );

    // The items, scrolled so the selected one is on screen.
    let rows = usize::from(inner.height.saturating_sub(2));
    let first = picker.selected.saturating_sub(rows.saturating_sub(1));
    for (row, (i, m)) in picker
        .matches
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .enumerate()
    {
        let item = &picker.items[m.index];
        let y = inner.y + 2 + row as u16;
        let chosen = i == picker.selected;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        if chosen {
            buf.set_style(row_rect, Style::new().bg(t().bg_line));
            buf.set_string(inner.x, y, "▍", Style::new().fg(t().blue));
        }
        let mut spans = Vec::new();
        match picker.kind {
            Kind::Files | Kind::Recent | Kind::Grep | Kind::Locations => {
                let located = matches!(picker.kind, Kind::Grep | Kind::Locations);
                let path = if located { &item.detail } else { &item.text };
                let (icon, color) = file_icon(path.split(':').next().unwrap_or(path));
                spans.push(Span::styled(icon, Style::new().fg(color)));
                if located {
                    spans.push(Span::styled(
                        format!("{} ", item.detail),
                        Style::new().fg(t().fg_dim),
                    ));
                }
            }
            Kind::Themes => {
                let mark = if nerd() { "\u{f1fc} " } else { "" };
                spans.push(Span::styled(mark, Style::new().fg(t().magenta)));
            }
            Kind::Keymaps => {}
            // The line number, dim, before the line.
            Kind::Lines => spans.push(Span::styled(
                format!("{:>5}  ", item.detail),
                Style::new().fg(t().fg_dim),
            )),
        }
        // The text, with the matched characters lit.
        let base = Style::new().fg(t().fg);
        let lit = Style::new().fg(t().orange).add_modifier(Modifier::BOLD);
        for (ci, c) in item.text.chars().enumerate() {
            let style = if m.positions.binary_search(&ci).is_ok() {
                lit
            } else {
                base
            };
            spans.push(Span::styled(c.to_string(), style));
        }
        Line::from(spans).render(
            Rect::new(inner.x + 2, y, inner.width.saturating_sub(3), 1),
            buf,
        );
        if picker.kind == Kind::Keymaps && !item.detail.is_empty() {
            let w = unicode_width::UnicodeWidthStr::width(item.detail.as_str()) as u16;
            let x = inner.right().saturating_sub(w + 1);
            buf.set_string(x.saturating_sub(1), y, " ", Style::new());
            buf.set_string(x, y, &item.detail, Style::new().fg(t().cyan));
            if chosen {
                buf.set_style(
                    Rect::new(x.saturating_sub(1), y, w + 1, 1),
                    Style::new().bg(t().bg_line),
                );
            }
        }
    }
    if picker.matches.is_empty() && !picker.loading {
        let note = match picker.kind {
            Kind::Grep if picker.query.is_empty() => "Type to search the text of the files",
            Kind::Recent if picker.items.is_empty() => "No recent files yet",
            _ => "No matches",
        };
        buf.set_string(inner.x + 2, inner.y + 2, note, Style::new().fg(t().fg_dim));
    }
    Some(cursor)
}

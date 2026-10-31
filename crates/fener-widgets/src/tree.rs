//! The project's folder tree beside the text (`Space e`).

use fener_core::Editor;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Widget};

use crate::icons::{file_icon, nerd};
use crate::theme::t;

/// The folder tree on the left: the root's name, then folders (▸ closed, ▾ open) and files,
/// the open file lit, the selected row marked when the tree has the focus.
pub fn draw_tree(buf: &mut Buffer, area: Rect, editor: &mut Editor) {
    let open_file = editor
        .doc
        .path()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
    let focus = editor.tree_focus;
    let Some(tree) = &mut editor.tree else {
        return;
    };
    Block::new()
        .style(Style::new().bg(t().bg_dark))
        .render(area, buf);
    // A thin line between the tree and the text.
    for y in area.top()..area.bottom() {
        buf.set_string(
            area.right() - 1,
            y,
            "│",
            Style::new().fg(t().gutter).bg(t().bg),
        );
    }
    let width = area.width.saturating_sub(1);
    let root_name = tree
        .root
        .file_name()
        .map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned());
    let folder = if nerd() { "\u{f07c} " } else { "" };
    buf.set_stringn(
        area.x + 1,
        area.y,
        format!("{folder}{root_name}"),
        usize::from(width.saturating_sub(1)),
        Style::new().fg(t().blue).add_modifier(Modifier::BOLD),
    );
    let rows = usize::from(area.height.saturating_sub(1));
    if tree.selected < tree.top {
        tree.top = tree.selected;
    } else if rows > 0 && tree.selected >= tree.top + rows {
        tree.top = tree.selected + 1 - rows;
    }
    for (i, row) in tree.rows.iter().enumerate().skip(tree.top).take(rows) {
        let y = area.y + 1 + (i - tree.top) as u16;
        let chosen = i == tree.selected;
        if chosen {
            let bg = if focus { t().bg_line } else { t().bg_dark };
            buf.set_style(Rect::new(area.x, y, width, 1), Style::new().bg(bg));
        }
        let mut spans = vec![Span::raw("  ".repeat(row.depth + 1))];
        let current = open_file.as_deref() == Some(row.path.as_path());
        if row.is_dir {
            let arrow = if row.expanded { "▾ " } else { "▸ " };
            let icon = match (nerd(), row.expanded) {
                (false, _) => "",
                (true, true) => "\u{f07c} ",
                (true, false) => "\u{f07b} ",
            };
            spans.push(Span::styled(arrow, Style::new().fg(t().fg_dim)));
            spans.push(Span::styled(icon, Style::new().fg(t().blue)));
            spans.push(Span::styled(row.name.as_str(), Style::new().fg(t().blue)));
        } else {
            let (icon, color) = file_icon(&row.name);
            let style = if current {
                Style::new().fg(t().orange).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(t().fg)
            };
            spans.push(Span::raw("  "));
            spans.push(Span::styled(icon, Style::new().fg(color)));
            spans.push(Span::styled(row.name.as_str(), style));
        }
        Line::from(spans).render(Rect::new(area.x, y, width, 1), buf);
    }
}

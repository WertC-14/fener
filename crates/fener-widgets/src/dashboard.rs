//! The start screen: logo and the menu of `fener_core::command::DASHBOARD`.

use fener_core::command::DASHBOARD;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::icons::command_icon;
use crate::theme::t;

const LOGO: [&str; 6] = [
    "███████╗███████╗███╗   ██╗███████╗██████╗ ",
    "██╔════╝██╔════╝████╗  ██║██╔════╝██╔══██╗",
    "█████╗  █████╗  ██╔██╗ ██║█████╗  ██████╔╝",
    "██╔══╝  ██╔══╝  ██║╚██╗██║██╔══╝  ██╔══██╗",
    "██║     ███████╗██║ ╚████║███████╗██║  ██║",
    "╚═╝     ╚══════╝╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝",
];

/// The start screen: the logo, a menu with one-letter keys, and a hint (LazyVim's dashboard).
pub fn draw_dashboard(buf: &mut Buffer, area: Rect, selected: usize) {
    let gap = u16::from(area.height >= 26);
    let menu_height = DASHBOARD.len() as u16 * (1 + gap);
    let logo = area.height >= menu_height + 12;
    let height = menu_height + 2 + if logo { LOGO.len() as u16 + 2 } else { 0 };
    let mut y = area.y + area.height.saturating_sub(height) / 2;
    let center = |w: u16| area.x + area.width.saturating_sub(w) / 2;
    if logo {
        for line in LOGO {
            let w = unicode_width::UnicodeWidthStr::width(line) as u16;
            buf.set_string(center(w), y, line, Style::new().fg(t().blue));
            y += 1;
        }
        y += 2;
    }
    let width = 46.min(area.width);
    let x = center(width);
    for (i, (key, label, command)) in DASHBOARD.iter().enumerate() {
        let row = Rect::new(x, y, width, 1);
        let chosen = i == selected;
        if chosen {
            buf.set_style(row, Style::new().bg(t().bg_line));
        }
        let label_style = if chosen {
            Style::new().fg(t().blue).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t().fg)
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {}", command_icon(*command)),
                Style::new().fg(t().cyan),
            ),
            Span::styled(format!(" {label}"), label_style),
        ]);
        line.render(row, buf);
        buf.set_string(
            x + width - 3,
            y,
            key.to_string(),
            Style::new().fg(t().orange).add_modifier(Modifier::BOLD),
        );
        y += 1 + gap;
    }
    y += 1;
    let hint = Line::from(vec![
        Span::styled("Press ", Style::new().fg(t().fg_dim)),
        Span::styled("Space", Style::new().fg(t().cyan)),
        Span::styled(" for the command menu, ", Style::new().fg(t().fg_dim)),
        Span::styled("?", Style::new().fg(t().orange)),
        Span::styled(" for all keys", Style::new().fg(t().fg_dim)),
    ]);
    let w = hint.width() as u16;
    hint.render(Rect::new(center(w), y, w.min(area.width), 1), buf);
}

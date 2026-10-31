//! The shell's screen (the bottom panel, full screen, or under a code tab's text), cell by cell
//! from vt100's screen.

use crate::app::{App, Focus, TermMode};
use liman_core::i18n::tr;
use liman_widgets::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::style::{Style, Stylize};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType};

/// Draws the shell's screen (vt100 cells) inside a rounded frame; blue frame = keys go to the shell.
pub(super) fn render_terminal(frame: &mut Frame, app: &mut App, area: Rect) {
    app.term_area = area;
    let focused = app.tab.focus == Focus::Terminal || app.tab.term_mode == TermMode::Fullscreen;
    let accent = if focused {
        theme::accent()
    } else {
        theme::dim()
    };
    let hint = match app.tab.term_mode {
        TermMode::Fullscreen => tr(" Terminal · Ctrl+O back to files "),
        _ if focused => tr(
            " Terminal · Tab on empty line: next panel · Ctrl+↑↓ size · F4 close · Ctrl+O full screen ",
        ),
        _ => tr(" Terminal · Tab or click to type "),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent))
        .title(Span::raw(hint).fg(if focused { theme::fg() } else { theme::dim() }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(term) = &mut app.tab.terminal else {
        return;
    };
    if let Some(cursor) = draw_term_screen(frame.buffer_mut(), inner, term, focused) {
        frame.set_cursor_position(cursor);
    }
}

/// A shell's screen into `inner` (resizing the shell to fit); the cursor when `focused`.
/// Shared by the folder tabs' terminal and the code tabs' one.
pub(crate) fn draw_term_screen(
    buf: &mut ratatui::buffer::Buffer,
    inner: Rect,
    term: &mut crate::terminal::Terminal,
    focused: bool,
) -> Option<(u16, u16)> {
    term.resize(inner.height, inner.width);
    let screen = term.screen();
    for row in 0..inner.height {
        for col in 0..inner.width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue; // covered by the wide character to its left
            }
            let mut style = Style::new()
                .fg(term_color(cell.fgcolor(), theme::fg()))
                .bg(term_color(cell.bgcolor(), theme::bg()));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let symbol = if cell.has_contents() {
                cell.contents()
            } else {
                " "
            };
            buf[(inner.x + col, inner.y + row)]
                .set_symbol(symbol)
                .set_style(style);
        }
    }
    (focused && !screen.hide_cursor()).then(|| {
        let (row, col) = screen.cursor_position();
        (inner.x + col, inner.y + row)
    })
}

fn term_color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

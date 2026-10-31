//! Ratatui drawing of a fener editor: the text with line numbers, the status line, the command
//! line, the which-key box, the start screen and the pickers. The `fener` app draws its code tabs
//! with it, and nothing here owns the terminal: [`render`] draws into a buffer area with a theme
//! it is given. The default colors are LazyVim's (tokyonight "night").

mod dashboard;
mod icons;
mod markdown;
mod picker;
mod popups;
mod status;
mod text_area;
mod theme;
mod tree;

pub use icons::set_nerd;
pub use theme::{THEMES, Theme};

use fener_core::{Editor, syntax};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Widget};

use crate::dashboard::draw_dashboard;
use crate::picker::draw_picker;
use crate::popups::{draw_completion, draw_hover, draw_which_key};
use crate::status::{draw_command_line, draw_status};
use crate::text_area::{Painter, draw_reader, draw_text};
use crate::theme::t;
use crate::tree::draw_tree;

/// What the UI keeps between frames.
#[derive(Default)]
pub struct View {
    /// First screen column shown (no wrapping: long lines scroll sideways).
    pub left: usize,
    /// Highlighting state at the first line on screen, for (document version, that line):
    /// scanning from the top of the file is needed only when either changes.
    syntax: Option<(u64, usize, syntax::State)>,
    /// tree-sitter colors (ADR 0015), made on a worker thread so typing never waits for a
    /// parse (a 3000-line file takes ~30 ms).
    tree_sitter: Painter,
    /// Called from the worker when new colors are ready: the app redraws.
    pub on_ready: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

/// Draws `editor` into `area` with `theme`; returns where the terminal cursor goes.
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    editor: &mut Editor,
    view: &mut View,
    theme: &Theme,
) -> Option<(u16, u16)> {
    theme::set_current(theme);
    let [body, status, command] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    Block::new()
        .style(Style::new().bg(t().bg))
        .render(area, buf);
    let mut cursor = None;
    if let Some(selected) = editor.dashboard {
        draw_dashboard(buf, body.union(status), selected);
    } else {
        let mut text_area = body;
        if editor.tree.is_some() {
            let width = (body.width / 3)
                .clamp(18, 34)
                .min(body.width.saturating_sub(10));
            let [left, right] =
                Layout::horizontal([Constraint::Length(width), Constraint::Min(1)]).areas(body);
            draw_tree(buf, left, editor);
            text_area = right;
        }
        if editor.reader {
            editor.view_height = usize::from(text_area.height);
            draw_reader(buf, text_area, editor);
        } else {
            editor.scroll_to_cursor(usize::from(text_area.height));
            cursor = draw_text(buf, text_area, editor, view);
        }
        if editor.tree_focus {
            cursor = None;
        }
        draw_status(buf, status, editor);
    }
    let command_cursor = draw_command_line(buf, command, editor);
    if let Some(at) = cursor {
        if let Some(menu) = &editor.lsp.completion {
            draw_completion(
                buf,
                body,
                at,
                menu,
                editor.cursor.saturating_sub(menu.start),
            );
        } else if let Some(text) = &editor.lsp.hover {
            draw_hover(buf, body, at, text);
        }
    }
    if let Some(menu) = editor.leader_menu() {
        // The title says which menu this is: the keys change with the file (code / Markdown).
        let title = format!("{} {}", editor.pending_keys(), editor.context().label());
        draw_which_key(buf, body, &menu, &title);
    }
    if let Some(picker) = &editor.picker {
        return draw_picker(buf, area, picker);
    }
    command_cursor.or(cursor)
}

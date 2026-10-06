//! Ratatui drawing of a fener editor: the text with line numbers, the status line, the command
//! line, the which-key box, the start screen and the pickers. Used by the `fener` app and by
//! liman's code tabs (fm-research ADR 0011), so nothing here owns the terminal: [`render`] draws
//! into a buffer area with a theme it is given. The default colors are LazyVim's (tokyonight
//! "night").

use std::cell::Cell;

use fener_core::command::DASHBOARD;
use fener_core::picker::{Kind, Picker};
use fener_core::text;
use fener_core::{Command, Editor, Mode};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};

/// A color scheme. [`THEMES`] has fener's own; an embedding app can make one from its palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    /// Text background.
    pub bg: Color,
    /// Status line, panels, picker box.
    pub bg_dark: Color,
    /// Cursor line and selected rows.
    pub bg_line: Color,
    pub bg_visual: Color,
    pub bg_search: Color,
    pub fg: Color,
    pub fg_dim: Color,
    /// Line numbers and `~` past the end.
    pub gutter: Color,
    pub orange: Color,
    pub blue: Color,
    pub green: Color,
    pub magenta: Color,
    pub yellow: Color,
    pub red: Color,
    pub cyan: Color,
}

const fn hex(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[rustfmt::skip]
const fn theme(name: &'static str, c: [u32; 15]) -> Theme {
    Theme {
        name, bg: hex(c[0]), bg_dark: hex(c[1]), bg_line: hex(c[2]), bg_visual: hex(c[3]),
        bg_search: hex(c[4]), fg: hex(c[5]), fg_dim: hex(c[6]), gutter: hex(c[7]),
        orange: hex(c[8]), blue: hex(c[9]), green: hex(c[10]), magenta: hex(c[11]),
        yellow: hex(c[12]), red: hex(c[13]), cyan: hex(c[14]),
    }
}

// bg, bg_dark, bg_line, bg_visual, bg_search, fg, fg_dim, gutter,
// orange, blue, green, magenta, yellow, red, cyan
#[rustfmt::skip]
pub const THEMES: [Theme; 6] = [
    theme("tokyonight-night", [0x1a1b26, 0x16161e, 0x292e42, 0x283457, 0x3d59a1, 0xc0caf5, 0x565f89, 0x3b4261,
        0xff9e64, 0x7aa2f7, 0x9ece6a, 0xbb9af7, 0xe0af68, 0xf7768e, 0x7dcfff]),
    theme("tokyonight-storm", [0x24283b, 0x1f2335, 0x292e42, 0x2e3c64, 0x3d59a1, 0xc0caf5, 0x565f89, 0x3b4261,
        0xff9e64, 0x7aa2f7, 0x9ece6a, 0xbb9af7, 0xe0af68, 0xf7768e, 0x7dcfff]),
    theme("tokyonight-moon", [0x222436, 0x1e2030, 0x2f334d, 0x2d3f76, 0x3e68d7, 0xc8d3f5, 0x636da6, 0x3b4261,
        0xff966c, 0x82aaff, 0xc3e88d, 0xc099ff, 0xffc777, 0xff757f, 0x86e1fc]),
    theme("tokyonight-day", [0xe1e2e7, 0xd0d5e3, 0xc4c8da, 0xb7c1e3, 0x7890dd, 0x3760bf, 0x848cb5, 0xa8aecb,
        0xb15c00, 0x2e7de9, 0x587539, 0x9854f1, 0x8c6c3e, 0xf52a65, 0x007197]),
    theme("catppuccin-mocha", [0x1e1e2e, 0x181825, 0x313244, 0x45475a, 0x585b70, 0xcdd6f4, 0x6c7086, 0x45475a,
        0xfab387, 0x89b4fa, 0xa6e3a1, 0xcba6f7, 0xf9e2af, 0xf38ba8, 0x89dceb]),
    theme("gruvbox", [0x282828, 0x1d2021, 0x3c3836, 0x504945, 0x665c54, 0xebdbb2, 0x928374, 0x665c54,
        0xfe8019, 0x83a598, 0xb8bb26, 0xd3869b, 0xfabd2f, 0xfb4934, 0x8ec07c]),
];

thread_local! {
    /// The theme of the frame being drawn (set by [`render`]).
    static CURRENT: Cell<Theme> = const { Cell::new(THEMES[0]) };
}

fn t() -> Theme {
    CURRENT.with(Cell::get)
}

/// Cells a tab takes (LazyVim: `tabstop = 2`).
const TAB_WIDTH: usize = 2;

/// What the UI keeps between frames.
#[derive(Default)]
pub struct View {
    /// First screen column shown (no wrapping: long lines scroll sideways).
    pub left: usize,
}

/// Draws `editor` into `area` with `theme`; returns where the terminal cursor goes.
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    editor: &mut Editor,
    view: &mut View,
    theme: &Theme,
) -> Option<(u16, u16)> {
    CURRENT.with(|c| c.set(*theme));
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
        editor.scroll_to_cursor(usize::from(body.height));
        cursor = draw_text(buf, body, editor, view);
        draw_status(buf, status, editor);
    }
    let command_cursor = draw_command_line(buf, command, editor);
    if let Some(menu) = editor.leader_menu() {
        draw_which_key(buf, body, &menu, &editor.pending_keys());
    }
    if let Some(picker) = &editor.picker {
        return draw_picker(buf, area, picker);
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
        let mut col = 0;
        for (i, c) in rope.slice(start..start + len).chars().enumerate() {
            let pos = start + i;
            let w = cell_width(c, col);
            let visible = col >= view.left && col + w <= view.left + width;
            if visible {
                let x = x0 + (col - view.left) as u16;
                let mut style = Style::new().fg(t().fg);
                if matches.iter().any(|&(a, b)| pos >= a && pos < b) {
                    style = style.bg(t().bg_search);
                }
                if selection.is_some_and(|(a, b)| pos >= a && pos < b) {
                    style = style.bg(t().bg_visual);
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
        Mode::Normal => ("NORMAL", t().blue),
        Mode::Insert => ("INSERT", t().green),
        Mode::Visual => ("VISUAL", t().magenta),
        Mode::VisualLine => ("V-LINE", t().magenta),
        Mode::Command | Mode::Search => ("COMMAND", t().yellow),
    }
}

/// lualine-like: the mode in its color, the file, and on the right pending keys, position, %.
fn draw_status(buf: &mut Buffer, area: Rect, editor: &Editor) {
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
                .fg(t().bg_dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {name}"), Style::new().fg(t().fg)),
        Span::styled(modified, Style::new().fg(t().yellow)),
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
            Style::new().fg(t().fg_dim),
        ),
        Span::styled(
            format!(" {percent} "),
            Style::new().fg(color).bg(t().bg_line),
        ),
        Span::styled(
            format!(" {}:{} ", editor.line() + 1, editor.col() + 1),
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
fn draw_command_line(buf: &mut Buffer, area: Rect, editor: &Editor) -> Option<(u16, u16)> {
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

/// which-key: the keys that can follow, in a box at the bottom right.
fn draw_which_key(buf: &mut Buffer, area: Rect, menu: &[(char, &str)], typed: &str) {
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

/// Nerd Font icons, unless `FENER_ICONS=plain` (LazyVim assumes a Nerd Font too).
fn nerd() -> bool {
    static NERD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *NERD.get_or_init(|| std::env::var("FENER_ICONS").map_or(true, |v| v != "plain"))
}

fn command_icon(command: Command) -> &'static str {
    if !nerd() {
        return "";
    }
    match command {
        Command::FindFiles => "\u{f002} ",
        Command::NewFile => "\u{f15b} ",
        Command::FindText => "\u{f0f6} ",
        Command::RecentFiles => "\u{f1da} ",
        Command::Themes => "\u{f1fc} ",
        Command::Keymaps => "\u{f11c} ",
        Command::Quit => "\u{f08b} ",
        _ => "\u{f101} ",
    }
}

/// A file's icon and its color, by extension.
fn file_icon(path: &str) -> (&'static str, Color) {
    if !nerd() {
        return ("", t().fg_dim);
    }
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "rs" => ("\u{e7a8} ", t().orange),
        "md" => ("\u{e73e} ", t().blue),
        "toml" | "yaml" | "yml" | "ini" | "conf" => ("\u{e615} ", t().fg_dim),
        "json" => ("\u{e60b} ", t().yellow),
        "py" => ("\u{e606} ", t().yellow),
        "js" | "ts" => ("\u{e74e} ", t().yellow),
        "sh" | "fish" | "bash" | "zsh" => ("\u{f489} ", t().green),
        "lock" => ("\u{f023} ", t().fg_dim),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => ("\u{f1c5} ", t().magenta),
        _ => ("\u{f15b} ", t().fg_dim),
    }
}

const LOGO: [&str; 6] = [
    "███████╗███████╗███╗   ██╗███████╗██████╗ ",
    "██╔════╝██╔════╝████╗  ██║██╔════╝██╔══██╗",
    "█████╗  █████╗  ██╔██╗ ██║█████╗  ██████╔╝",
    "██╔══╝  ██╔══╝  ██║╚██╗██║██╔══╝  ██╔══██╗",
    "██║     ███████╗██║ ╚████║███████╗██║  ██║",
    "╚═╝     ╚══════╝╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝",
];

/// The start screen: the logo, a menu with one-letter keys, and a hint (LazyVim's dashboard).
fn draw_dashboard(buf: &mut Buffer, area: Rect, selected: usize) {
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

/// A picker as a floating box: the query on top, the matching items below, the selected one
/// marked. Returns the cursor (at the end of the query).
fn draw_picker(buf: &mut Buffer, area: Rect, picker: &Picker) -> Option<(u16, u16)> {
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
            Kind::Files | Kind::Recent | Kind::Grep => {
                let path = if picker.kind == Kind::Grep {
                    &item.detail
                } else {
                    &item.text
                };
                let (icon, color) = file_icon(path.split(':').next().unwrap_or(path));
                spans.push(Span::styled(icon, Style::new().fg(color)));
                if picker.kind == Kind::Grep {
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

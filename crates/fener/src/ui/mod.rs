//! Turns [`App`] state into a frame. Immediate mode: the whole screen is described on every draw,
//! ratatui then sends only the cells that changed since the previous frame.

use crate::app::{App, Focus, Listing, TermMode, View};
use liman_core::format;
use liman_core::i18n::{tr, trf};
use liman_widgets::theme;
use liman_widgets::{FileList, GridView, ListMode, Sidebar, breadcrumb, file_list, grid, sidebar};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

mod popups;
mod status;
mod terminal;

pub(crate) use terminal::draw_term_screen;

use popups::{
    render_branch_picker, render_dialog, render_git_panel, render_help, render_menu,
    render_theme_picker,
};
use status::render_status_bar;
use terminal::render_terminal;

/// Below this width the sidebar is hidden so the list keeps enough room.
const SIDEBAR_MIN_WIDTH: u16 = 70;

pub fn render(frame: &mut Frame, app: &mut App) {
    render_screen(frame, app);
    if app.code.active.is_some() {
        // liman's popups belong to the folder tabs.
    } else if let Some(dialog) = &app.dialog {
        render_dialog(frame, dialog);
    } else if app.branch_picker.is_some() {
        render_branch_picker(frame, app);
    } else if app.git_panel.is_some() {
        render_git_panel(frame, app);
    } else if app.menu.is_some() {
        render_menu(frame, app);
    } else if app.theme_picker.is_some() {
        render_theme_picker(frame, app);
    } else if app.help_open {
        render_help(frame);
    }
    if !liman_widgets::colors::truecolor() {
        liman_widgets::colors::downsample(frame.buffer_mut());
    }
}

fn render_screen(frame: &mut Frame, app: &mut App) {
    frame.render_widget(
        Block::new().style(Style::new().bg(theme::bg())),
        frame.area(),
    );
    // Tab row (ADR 0008, sketch B): only with two or more tabs.
    let labels = app.tab_labels();
    let screen = if labels.is_empty() {
        app.tabs.chip_areas.clear();
        app.tabs.plus_area = Rect::default();
        frame.area()
    } else {
        let [row, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(frame.area());
        render_tab_row(frame, app, &labels, row);
        rest
    };
    // A code tab: the editor takes everything under the tab row (ADR 0012).
    if app.code.active.is_some() {
        if let Some(cursor) = app.render_code_tab(frame.buffer_mut(), screen) {
            frame.set_cursor_position(cursor);
        }
        return;
    }
    let [top, body, status] = Layout::vertical([
        Constraint::Length(3), // framed title bar with the path chips
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(screen);

    if app.tab.term_mode == TermMode::Fullscreen {
        render_terminal(frame, app, top.union(body));
        render_status_bar(frame, app, status);
        return;
    }
    render_path_bar(frame, app, top);

    // F4 panel: full width under the files, like Dolphin.
    let body = if app.tab.term_mode == TermMode::Panel {
        let height = app
            .term_height
            .unwrap_or(body.height * 2 / 5)
            .max(4)
            .min(body.height.saturating_sub(4));
        let [files, term] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(height)]).areas(body);
        render_terminal(frame, app, term);
        files
    } else {
        app.term_area = Rect::default();
        body
    };

    // `r` / Enter in the preview: the file fills the space of the sidebar and the list.
    if app.preview.reader {
        app.sidebar_area = Rect::default();
        app.list_area = Rect::default();
        let title = tr(" Reader · ↑↓ PgUp/PgDn scroll · Enter back to the panel · Esc close ");
        render_preview(frame, app, body, title, true);
        render_status_bar(frame, app, status);
        return;
    }

    let main = if body.width >= SIDEBAR_MIN_WIDTH {
        let [side, main] =
            Layout::horizontal([Constraint::Length(sidebar::WIDTH + 2), Constraint::Fill(1)])
                .areas(body);
        let places_focused = app.tab.focus == Focus::Places;
        let block = panel(tr(" Places "), places_focused);
        let places = block.inner(side).inner(Margin::new(0, 1));
        frame.render_widget(block, side);
        app.sidebar_area = places;
        app.sidebar_offset = sidebar_offset(app, places.height);
        let mut sidebar =
            Sidebar::new(&app.sidebar, app.tree.rows(), &app.tab.cwd).offset(app.sidebar_offset);
        if places_focused {
            sidebar = sidebar.focused(app.sidebar_selected);
        }
        frame.render_widget(sidebar, places);
        main
    } else {
        app.sidebar_area = Rect::default();
        body
    };
    // F3: preview on the right, if the files keep enough room.
    let (main, preview_area) = if app.preview.shown && main.width >= 60 {
        let [files, preview] =
            Layout::horizontal([Constraint::Percentage(58), Constraint::Fill(1)]).areas(main);
        (files, Some(preview))
    } else {
        (main, None)
    };
    let files_focused = app.tab.focus == Focus::Files;
    let title = match (&app.tab.results, app.entry_count()) {
        (Some(_), Some(n)) => trf(" Results · {} ", &[&format::items(n)]),
        (None, Some(n)) => format!(" {} · {} ", folder_name(app), format::items(n)),
        _ => format!(" {} ", folder_name(app)),
    };
    let block = panel(&title, files_focused);
    let inner = block.inner(main);
    frame.render_widget(block, main);
    let main = inner;
    render_list(frame, app, main.inner(Margin::new(2, 1)));
    match preview_area {
        Some(area) => {
            let focused = app.tab.focus == Focus::Preview;
            render_preview(frame, app, area, tr(" Preview "), focused)
        }
        None => app.preview.area = Rect::default(),
    }
    render_status_bar(frame, app, status);
}

/// Scrolls Places so the keyboard cursor (with focus) or the open folder stays on screen.
fn sidebar_offset(app: &App, height: u16) -> usize {
    let height = usize::from(height).max(1);
    let places = app.sidebar.len();
    let wanted = if app.tab.focus == Focus::Places {
        Some(app.sidebar_selected)
    } else {
        app.tree
            .rows()
            .iter()
            .position(|r| r.path == app.tab.cwd)
            .map(|d| Sidebar::dir_row(places, d))
    };
    let mut offset = app.sidebar_offset;
    if let Some(row) = wanted {
        if row < offset {
            offset = row;
        } else if row >= offset + height {
            offset = row + 1 - height;
        }
    }
    let len = Sidebar::len(places, app.tree.rows().len());
    offset.min(len.saturating_sub(height))
}

fn render_preview(frame: &mut Frame, app: &mut App, area: Rect, title: &str, focused: bool) {
    let block = panel(title, focused);
    let inner = block.inner(area).inner(Margin::new(1, 1));
    frame.render_widget(block, area);
    app.preview.area = inner;
    app.want_preview(
        inner.width,
        inner
            .height
            .saturating_sub(liman_widgets::preview::HEADER_ROWS),
    );
    if let Some(preview) = &app.preview.current {
        let graphic = app.preview.graphic.as_deref().map(|g| &g.0);
        frame.render_widget(
            liman_widgets::preview::PreviewView::new(preview)
                .scroll(app.preview.scroll)
                .raw_markdown(app.preview.raw_markdown)
                .skip_image(graphic.is_some()),
            inner,
        );
        // A real image (Kitty / Sixel / iTerm2) under the header, centered (ADR 0009).
        if let Some(graphic) = graphic {
            let header = liman_widgets::preview::HEADER_ROWS;
            let body = Rect {
                y: inner.y + header.min(inner.height),
                height: inner.height.saturating_sub(header),
                ..inner
            };
            let size = graphic.size();
            let width = size.width.min(body.width);
            let area = Rect {
                x: body.x + (body.width - width) / 2,
                width,
                height: size.height.min(body.height),
                ..body
            };
            frame.render_widget(ratatui_image::Image::new(graphic), area);
        }
    }
}

/// `  1 ⌂ Ev    2 liman ●    3 Downloads    +` — the active tab is a filled chip.
fn render_tab_row(frame: &mut Frame, app: &mut App, labels: &[crate::app::TabLabel], area: Rect) {
    let accent = theme::accent();
    let mut x = area.x + 1;
    app.tabs.chip_areas.clear();
    for (i, label) in labels.iter().enumerate() {
        let shell = if label.has_shell { " ●" } else { "" };
        let text = format!(" {} {}{shell} ", i + 1, label.title);
        let width = (text.chars().count() as u16).min(area.right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        let style = if label.active {
            Style::new().bg(accent).fg(theme::text_on(accent)).bold()
        } else {
            Style::new().fg(theme::dim())
        };
        frame.render_widget(Paragraph::new(text).style(style), rect);
        app.tabs.chip_areas.push(rect);
        x += width + 2;
    }
    app.tabs.plus_area = Rect::default();
    if x + 3 <= area.right() {
        let rect = Rect::new(x, area.y, 3, 1);
        frame.render_widget(Paragraph::new(" + ").fg(theme::dim()), rect);
        app.tabs.plus_area = rect;
    }
}

/// A rounded panel with a title; the focused one gets the accent color (style A, fm-research LOG).
fn panel(title: &str, focused: bool) -> Block<'static> {
    let accent = theme::accent();
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { accent } else { theme::border() }))
        .title(
            Span::raw(title.to_string())
                .fg(if focused { theme::fg() } else { theme::dim() })
                .bold(),
        )
        .style(Style::new().bg(theme::bg()))
}

fn folder_name(app: &App) -> String {
    app.tab
        .cwd
        .file_name()
        .map_or_else(|| "/".to_string(), |n| n.to_string_lossy().into_owned())
}

fn render_path_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = panel(" fener ", false);
    let area = block.inner(area);
    frame.render_widget(
        block,
        Rect::new(area.x - 1, area.y - 1, area.width + 2, area.height + 2),
    );
    app.path_bar_area = area;
    if let Some(text) = &app.path_input {
        let line = Line::from(vec![
            Span::raw(" ➜ ").fg(theme::accent()).bold(),
            Span::raw(text.as_str()).fg(theme::fg()).bold(),
            Span::raw("▏").fg(theme::fg()),
            Span::raw(tr("   Tab complete · Enter go · Esc cancel")).fg(theme::dim()),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(theme::bar_bg())),
            area,
        );
        return;
    }
    if let Some(results) = &app.tab.results {
        let accent = theme::accent();
        let line = Line::from(vec![
            Span::raw(" ⌕ ").fg(accent).bold(),
            Span::raw(results.command.clone()).fg(theme::fg()).bold(),
            Span::raw(trf(
                "  ·  {} found in {}",
                &[&format::items(results.count), &app.tab.cwd.display()],
            ))
            .fg(theme::dim()),
            Span::raw(if results.running {
                tr("  · searching…")
            } else {
                ""
            })
            .fg(theme::accent()),
            Span::raw(tr("   Bksp/Esc back to the folder")).fg(theme::dim()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    frame.render_widget(
        Paragraph::new("").style(Style::new().bg(theme::bar_bg())),
        area,
    );
    // Git branch on the right: "⎇ main ↑1 ↓2" plus a count of changed files.
    // The path gets the rest of the row so the two never overlap.
    let mut path_area = area;
    if let Some(git) = &app.tab.git {
        let changed = git.files.len();
        let text = if changed > 0 {
            trf(" ⎇ {} · {} changed  Ctrl+G ", &[&git.summary(), &changed])
        } else {
            format!(" ⎇ {}  Ctrl+G ", git.summary())
        };
        let width = text.chars().count() as u16;
        if area.width > width + 20 {
            let rect = Rect::new(area.right() - width, area.y, width, 1);
            path_area.width -= width + 1;
            let color = if changed > 0 {
                theme::type_color(liman_core::FileType::Presentation)
            } else {
                theme::type_color(liman_core::FileType::Spreadsheet)
            };
            frame.render_widget(Paragraph::new(Span::raw(text).fg(color).bold()), rect);
        }
    }
    let line = breadcrumb::line(&app.path_segments());
    frame.render_widget(Paragraph::new(line), path_area);
}

fn render_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let message = match &app.tab.listing {
        Listing::Loading => tr("Loading…").to_string(),
        Listing::Failed(err) => trf("Cannot open this folder: {}", &[err]),
        Listing::Ready(entries) if entries.is_empty() => tr("Folder is empty").to_string(),
        Listing::Ready(entries) => {
            app.list_area = area;
            app.drawn_view = fitting_view(app.view, area);
            // Borrow the fields directly (not through a method on `app`) so that `app.tab.table`
            // can be borrowed mutably while `entries` is borrowed immutably.
            let rows = liman_widgets::Rows::ordered(entries, &app.tab.visible);
            let git_marks = app.tab.git.as_ref().map(|g| &g.marks);
            match app.drawn_view.list_mode() {
                Some(mode) => {
                    let mut list = FileList::new(rows, format::now(), mode)
                        .marked(&app.tab.marked)
                        .sort(app.sort);
                    if let Some(marks) = git_marks {
                        list = list.git(marks);
                    }
                    frame.render_stateful_widget(list, area, &mut app.tab.table);
                }
                None => {
                    let wanted = app.grid_level.unwrap_or(grid::DEFAULT_LEVEL);
                    app.drawn_grid_level = grid::fitting_level(wanted, area);
                    let mut grid =
                        GridView::new(rows, app.drawn_grid_level).marked(&app.tab.marked);
                    if let Some(marks) = git_marks {
                        grid = grid.git(marks);
                    }
                    frame.render_stateful_widget(grid, area, &mut app.tab.table);
                }
            }
            return;
        }
    };
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(message).centered().fg(theme::dim()), middle);
}

/// Falls back to a smaller view when the wanted one does not fit: the grid needs one whole tile,
/// boxes need room for two entries.
fn fitting_view(wanted: View, area: Rect) -> View {
    let grid_fits = area.width >= grid::MIN_TILE.0 && area.height >= grid::MIN_TILE.1;
    let boxes_fit = area.height >= file_list::HEADER_HEIGHT + 2 * ListMode::Normal.row_height()
        && area.width >= 50;
    match wanted {
        View::Grid if grid_fits => View::Grid,
        View::Grid | View::Normal if boxes_fit => View::Normal,
        _ => View::Detailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liman_core::{Entry, FileType, Places};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;

    fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn app(cwd: &str) -> App {
        let (tx, _rx) = mpsc::channel();
        App::new(
            PathBuf::from(cwd),
            Places::from_user_dirs("", Path::new("/home/test")),
            tx,
        )
    }

    #[test]
    fn shows_path_loading_and_quit_hint() {
        let mut app = app("/home/test/Projects");
        let mut terminal = Terminal::new(TestBackend::new(140, 14)).unwrap();
        terminal.draw(|f| render(f, &mut app)).unwrap();

        let rows = rows(&terminal);
        assert!(rows[0].contains("fener")); // framed title bar
        assert!(rows[1].contains(" ⌂ Home  ›  Projects ")); // path chips
        assert!(rows.iter().any(|r| r.contains("Loading")));
        assert!(rows[3].contains("Places") && rows[3].contains("Projects")); // panel titles
        assert!(rows[5].contains("QUICK ACCESS")); // sidebar
        assert!(rows[6].contains("⌂ Home"));
        // Status bar: the keys sit on its right, cardea style.
        assert!(rows[13].trim_end().ends_with("[q] Quit"), "{:?}", rows[13]);
    }

    #[test]
    fn normal_view_falls_back_to_detailed_when_too_small() {
        assert_eq!(
            fitting_view(View::Normal, Rect::new(0, 0, 80, 30)),
            View::Normal
        );
        assert_eq!(
            fitting_view(View::Normal, Rect::new(0, 0, 80, 10)),
            View::Detailed
        );
        assert_eq!(
            fitting_view(View::Normal, Rect::new(0, 0, 40, 30)),
            View::Detailed
        );
        assert_eq!(
            fitting_view(View::Grid, Rect::new(0, 0, 80, 30)),
            View::Grid
        );
        assert_eq!(
            fitting_view(View::Grid, Rect::new(0, 0, 80, 6)),
            View::Detailed
        );
        assert_eq!(
            fitting_view(View::Grid, Rect::new(0, 0, 15, 30)),
            View::Detailed
        );
    }

    #[test]
    fn narrow_terminal_hides_the_sidebar() {
        let mut app = app("/home/test");
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        assert_eq!(app.sidebar_area, Rect::default());
    }

    #[test]
    fn shows_entries_count_and_selection_when_ready() {
        let mut app = app("/home/test");
        app.tab.listing = Listing::Ready(vec![Entry {
            name: "main.rs".into(),
            path: PathBuf::from("main.rs"),
            is_dir: false,
            is_symlink: false,
            special: None,
            size: 13_800,
            item_count: None,
            contents: None,
            modified: None,
            file_type: FileType::Code,
        }]);
        app.tab.visible = vec![0];
        app.tab.table.select(Some(0));
        let mut terminal = Terminal::new(TestBackend::new(140, 14)).unwrap();
        terminal.draw(|f| render(f, &mut app)).unwrap();

        let rows = rows(&terminal);
        assert!(
            rows.iter()
                .any(|r| r.contains("RS") && r.contains("main.rs") && r.contains("13.8 kB"))
        );
        assert!(
            rows[13].starts_with(" 1 item · “main.rs”"),
            "{:?}",
            rows[13]
        );
    }
}

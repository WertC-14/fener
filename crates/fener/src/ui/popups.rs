//! Boxes over the file manager: dialogs (rename, conflicts, delete), the git panel and its
//! branch picker, the command palette and right-click menu, the theme picker and the key overview.

use super::panel;
use crate::app::App;
use liman_core::format;
use liman_core::i18n::{tr, trf};
use liman_widgets::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

/// A centered box of `width` × `height` cells with a rounded accent frame; returns the inside.
fn popup(frame: &mut Frame, title: &str, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let (w, h) = (width.min(area.width), height.min(area.height));
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    frame.render_widget(Clear, rect);
    let block = panel(title, true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

pub(super) fn render_theme_picker(frame: &mut Frame, app: &mut App) {
    let themes = &liman_widgets::theme::THEMES;
    let inner = popup(
        frame,
        tr(" Theme · ↑↓ preview · Enter keep · Esc cancel "),
        56,
        themes.len() as u16 + 4,
    );
    let list = inner.inner(Margin::new(1, 1));
    app.picker_area = list;
    let selected = app.theme_picker.map_or(0, |(s, _)| s);
    for (i, t) in themes.iter().enumerate() {
        let mut spans = vec![
            Span::raw(if i == selected { " ▶ " } else { "   " }).fg(theme::accent()),
            Span::raw(format!("{:<12}", t.name)).fg(theme::fg()).bold(),
        ];
        // swatches on the theme's own background
        spans.push(Span::raw(" ").bg(t.bg));
        for c in t.types.iter().take(9) {
            spans.push(Span::raw("■ ").fg(*c).bg(t.bg));
        }
        let mut line = Line::from(spans);
        if i == selected {
            line = line.style(Style::new().bg(theme::selected_bg()));
        }
        let row = Rect::new(list.x, list.y + i as u16, list.width, 1);
        frame.render_widget(Paragraph::new(line), row.intersection(list));
    }
}

pub(super) fn render_dialog(frame: &mut Frame, dialog: &crate::app::Dialog) {
    use crate::app::Dialog;
    let (title, lines): (&str, Vec<Line>) = match dialog {
        Dialog::Conflict { existing } => {
            let mut lines = vec![
                Line::from(trf("{} already here:", &[&format::items(existing.len())]))
                    .fg(theme::fg()),
            ];
            for path in existing.iter().take(5) {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                lines.push(Line::from(format!("  • {name}")).fg(theme::dim()));
            }
            if existing.len() > 5 {
                lines.push(
                    Line::from(trf("  … and {} more", &[&(existing.len() - 5)])).fg(theme::dim()),
                );
            }
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::raw(" b ").bold().fg(theme::accent()),
                Span::raw(tr("keep both (Enter)   ")).fg(theme::fg()),
                Span::raw(" r ").bold().fg(theme::accent()),
                Span::raw(tr("replace (old to trash)   ")).fg(theme::fg()),
                Span::raw(" s ").bold().fg(theme::accent()),
                Span::raw(tr("skip   ")).fg(theme::fg()),
                Span::raw(" Esc ").bold().fg(theme::dim()),
            ]));
            (tr(" Paste "), lines)
        }
        Dialog::ConfirmDiscard { paths } => {
            let question = trf(
                "Throw away the changes in {}?",
                &[&format::items(paths.len())],
            );
            let note = "Tracked files go back to the last commit; new files go to the trash.";
            (
                tr(" Git: discard "),
                confirm_lines(question, note, "discard   "),
            )
        }
        Dialog::ConfirmTrash { paths, inside } => {
            let what = match paths.as_slice() {
                [one] => format!(
                    "“{}”",
                    one.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                many => format::items(many.len()),
            };
            let question = match inside {
                Some(inside) => trf("Move {} and the {} inside to the trash?", &[&what, inside]),
                None => trf("Move {} to the trash?", &[&what]),
            };
            let note = "It can be brought back with Ctrl+Z or from the Trash.";
            (tr(" Trash "), confirm_lines(question, note, "trash   "))
        }
        Dialog::ConfirmDelete { paths } => {
            let what = match paths.as_slice() {
                [one] => format!(
                    "“{}”",
                    one.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                many => format::items(many.len()),
            };
            let question = trf("Delete {} for good?", &[&what]);
            let note = "This cannot be undone (Del moves to the trash instead).";
            (
                tr(" Delete for good "),
                confirm_lines(question, note, "delete   "),
            )
        }
    };
    let height = lines.len() as u16 + 4;
    let inner = popup(frame, title, 72, height);
    frame.render_widget(Paragraph::new(lines), inner.inner(Margin::new(1, 1)));
}

/// A yes/no question: the question, a note under it, then `y <verb>   n / Esc keep`.
fn confirm_lines(question: String, note: &'static str, verb: &'static str) -> Vec<Line<'static>> {
    vec![
        Line::from(question).fg(theme::fg()).bold(),
        Line::from(tr(note)).fg(theme::dim()),
        Line::default(),
        Line::from(vec![
            Span::raw(" y ")
                .bold()
                .fg(theme::type_color(liman_core::FileType::Pdf)),
            Span::raw(tr(verb)).fg(theme::fg()),
            Span::raw(" n / Esc ").bold().fg(theme::accent()),
            Span::raw(tr("keep")).fg(theme::fg()),
        ]),
    ]
}

/// Ctrl+G: changed files on the left (status letters, path in the repository), the selected file's
/// diff on the right. Keys are listed in the frame title.
pub(super) fn render_git_panel(frame: &mut Frame, app: &mut App) {
    app.git_panel_diff();
    let (Some(git), Some(panel)) = (&app.tab.git, &mut app.git_panel) else {
        return;
    };
    let screen = frame.area();
    let (w, h) = (
        screen.width.saturating_sub(6).max(40),
        screen.height.saturating_sub(4).max(10),
    );
    // "⎇ main ↑2 → origin/main · github.com/user/repo": what p pushes and where.
    let target = match (&git.upstream, &panel.remote) {
        (Some(up), Some((_, url))) => {
            format!(" → {up} · {}", liman_core::git::short_url(url))
        }
        (None, Some((name, url))) => format!(
            " → {name} ({}) · {}",
            tr("new branch"),
            liman_core::git::short_url(url)
        ),
        (_, None) if panel.remote_known => format!(" · {}", tr("no remote")),
        (_, None) => String::new(), // still asking git
    };
    let title = format!(" Git · ⎇ {}{target} ", git.summary());
    let inner = popup(frame, &title, w, h);
    let [body, keys] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(
            tr(" Space stage/unstage  a stage all  c commit  d discard  p push  P pull  b branch  Enter show  Esc close"),
        )
        .fg(theme::dim()),
        keys,
    );
    let [list_area, diff_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)])
            .spacing(1)
            .areas(body.inner(Margin::new(1, 1)));

    if git.files.is_empty() {
        frame.render_widget(
            Paragraph::new(tr("Working tree clean")).fg(theme::dim()),
            list_area,
        );
        return;
    }
    panel.selected = panel.selected.min(git.files.len() - 1);
    let visible = usize::from(list_area.height);
    let start = panel.selected.saturating_sub(visible.saturating_sub(1));
    let lines: Vec<Line> = git
        .files
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, (path, mark))| {
            let rel = path
                .strip_prefix(&git.root)
                .unwrap_or(path)
                .display()
                .to_string();
            let mut spans = liman_widgets::gitmark::spans(Some(*mark));
            spans.push(Span::raw(rel).fg(theme::fg()));
            let line = Line::from(spans);
            if i == panel.selected {
                line.style(Style::new().bg(theme::selected_bg()))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);

    let added = theme::type_color(liman_core::FileType::Spreadsheet);
    let removed = theme::type_color(liman_core::FileType::Pdf);
    let hunk = theme::type_color(liman_core::FileType::Document);
    panel.scroll = panel.scroll.min(panel.diff.len().saturating_sub(1));
    let diff: Vec<Line> = panel
        .diff
        .iter()
        .skip(panel.scroll)
        .take(usize::from(diff_area.height))
        .map(|l| {
            let color = match l.chars().next() {
                Some('+') if !l.starts_with("+++") => added,
                Some('-') if !l.starts_with("---") => removed,
                Some('@') => hunk,
                _ => theme::dim(),
            };
            Line::from(Span::raw(l.clone()).fg(color))
        })
        .collect();
    frame.render_widget(Clear, diff_area);
    frame.render_widget(Paragraph::new(diff).bg(theme::bg()), diff_area);
}

pub(super) fn render_branch_picker(frame: &mut Frame, app: &App) {
    let Some((branches, selected)) = &app.branch_picker else {
        return;
    };
    let inner = popup(
        frame,
        tr(" Switch branch · Enter switch · Esc "),
        48,
        branches.len().min(16) as u16 + 4,
    );
    let lines: Vec<Line> = branches
        .iter()
        .enumerate()
        .take(16)
        .map(|(i, b)| {
            let line = Line::from(format!(" ⎇ {b}")).fg(theme::fg());
            if i == *selected {
                line.style(Style::new().bg(theme::selected_bg())).bold()
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner.inner(Margin::new(1, 1)));
}

/// Command palette (centered, with a search line) or right-click menu (at the mouse).
pub(super) fn render_menu(frame: &mut Frame, app: &mut App) {
    let availability: Vec<bool> = app
        .menu
        .as_ref()
        .map(|m| m.items.iter().map(|a| app.action_available(*a)).collect())
        .unwrap_or_default();
    let Some(menu) = &mut app.menu else {
        return;
    };
    const ROWS: usize = 14;
    let list = if let Some(query) = &menu.query {
        let inner = popup(
            frame,
            tr(" Commands · type to search · Enter run · Esc close "),
            60,
            ROWS as u16 + 5,
        );
        let input = Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("> ").fg(theme::accent()).bold(),
                Span::raw(query.clone()).fg(theme::fg()).bold(),
                Span::raw("▏").fg(theme::fg()),
            ])),
            input,
        );
        Rect::new(
            inner.x + 1,
            inner.y + 3,
            inner.width.saturating_sub(2),
            ROWS as u16,
        )
    } else {
        let screen = frame.area();
        let (w, h) = (40, menu.items.len() as u16 + 2);
        let x = menu.at.0.min(screen.right().saturating_sub(w));
        let y = menu.at.1.min(screen.bottom().saturating_sub(h));
        let rect = Rect::new(x, y, w, h);
        frame.render_widget(Clear, rect);
        let block = panel("", true);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        inner
    };
    // Keep the selection in view.
    let visible = usize::from(list.height);
    if menu.selected < menu.offset {
        menu.offset = menu.selected;
    } else if menu.selected >= menu.offset + visible {
        menu.offset = menu.selected + 1 - visible;
    }
    app.menu_area = list;
    for (row, (i, action)) in menu
        .items
        .iter()
        .enumerate()
        .skip(menu.offset)
        .take(visible)
        .enumerate()
    {
        let available = availability.get(i).copied().unwrap_or(true);
        let color = if available { theme::fg() } else { theme::dim() };
        let keys = tr(action.keys());
        let width = usize::from(list.width);
        let label_width = width.saturating_sub(keys.chars().count() + 2);
        let mut line = Line::from(vec![
            Span::raw(format!(" {:<label_width$}", action.label())).fg(color),
            Span::raw(format!("{keys} ")).fg(theme::dim()),
        ]);
        if i == menu.selected {
            line = line.style(Style::new().bg(theme::selected_bg())).bold();
        }
        let rect = Rect::new(list.x, list.y + row as u16, list.width, 1);
        frame.render_widget(Paragraph::new(line), rect);
    }
    if menu.items.is_empty() {
        frame.render_widget(
            Paragraph::new(tr(" No matching command")).fg(theme::dim()),
            list,
        );
    }
}

/// The `?` window, from [`crate::app::HELP`]: one column when it fits the screen's height, else
/// two side by side (descriptions are cut on a narrow screen rather than lines lost at the bottom).
pub(super) fn render_help(frame: &mut Frame) {
    let items: Vec<(&str, &str)> = crate::app::HELP.iter().map(|h| h.texts()).collect();
    let screen = frame.area();
    let columns = if items.len() + 4 <= usize::from(screen.height) {
        1
    } else {
        2
    };
    let rows = items.len().div_ceil(columns);
    let width = |text: &str| text.chars().count();
    // Each column as wide as its longest key and description.
    let chunks: Vec<&[(&str, &str)]> = items.chunks(rows).collect();
    let sizes: Vec<(usize, usize)> = chunks
        .iter()
        .map(|chunk| {
            let keys = chunk.iter().map(|(k, _)| width(k)).max().unwrap_or(0);
            let what = chunk.iter().map(|(_, w)| width(w)).max().unwrap_or(0);
            (keys, what)
        })
        .collect();
    let total: usize = sizes.iter().map(|(k, w)| k + 3 + w).sum::<usize>() + 2 * (columns - 1);
    let inner = popup(
        frame,
        tr(" Keys · any key closes "),
        (total + 4) as u16,
        rows as u16 + 4,
    );
    let body = inner.inner(Margin::new(1, 1));
    let areas = Layout::horizontal(
        sizes
            .iter()
            .map(|(k, w)| Constraint::Length((k + 3 + w) as u16)),
    )
    .spacing(2)
    .split(body);
    for ((chunk, (key_width, _)), area) in chunks.iter().zip(&sizes).zip(areas.iter()) {
        let lines: Vec<Line> = chunk
            .iter()
            .map(|(keys, what)| {
                Line::from(vec![
                    Span::raw(format!(" {keys:<key_width$}  "))
                        .fg(theme::accent())
                        .bold(),
                    Span::raw(*what).fg(theme::fg()),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), *area);
    }
}

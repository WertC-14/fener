//! The status bar: what is selected or happening, and the key hints that fit.

use crate::app::{App, ClipMode, Focus, TermMode};
use liman_core::format;
use liman_core::i18n::{tr, trf};
use liman_widgets::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Status bar key hints (English; shown through `tr`).
pub(super) const HINTS: &[(&str, &str)] = &[
    ("?", "Help"),
    ("Ctrl+F", "Search"),
    ("Ctrl+H", "Hidden"),
    ("Ctrl+P", "Commands"),
    ("q", "Quit"),
];

pub(super) const PREVIEW_HINTS: &[(&str, &str)] = &[
    ("↑↓ PgUp/PgDn", "Scroll"),
    ("g/G", "Start / end"),
    ("Enter", "Full screen"),
    ("m", "Markdown"),
    ("Esc", "Files"),
];

pub(super) const TERMINAL_HINTS: &[(&str, &str)] = &[
    ("Ctrl+O", "files / full screen"),
    ("F6", "focus"),
    ("F4", "panel"),
];

pub(super) fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    if app.tab.term_mode != TermMode::Hidden && app.tab.focus == Focus::Terminal
        || app.tab.term_mode == TermMode::Fullscreen
    {
        spans.push(Span::raw(" ⌂ ").fg(theme::dim()));
        spans.push(Span::raw(app.tab.cwd.display().to_string()).fg(theme::fg()));
        for (key, what) in TERMINAL_HINTS {
            spans.push(Span::raw(format!("  {key} ")).bold());
            spans.push(Span::raw(tr(what)).fg(theme::dim()));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(Style::new().fg(theme::fg()).bg(theme::bar_bg())),
            area,
        );
        return;
    }
    if let Some(text) = &app.commit_input {
        let line = Line::from(vec![
            Span::raw(tr(" ⎇ Commit message: ")).fg(theme::dim()),
            Span::raw(text.clone()).fg(theme::fg()).bold(),
            Span::raw(tr("▏   Enter commit · Esc cancel")).fg(theme::dim()),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(theme::bar_bg())),
            area,
        );
        return;
    }
    if let Some(text) = &app.search_input {
        let line = Line::from(vec![
            Span::raw(tr(" ⌕ Search in this folder and below: ")).fg(theme::dim()),
            Span::raw(text.clone()).fg(theme::fg()).bold(),
            Span::raw(tr("▏   ↑↓ choose · Enter keep the results · Esc back")).fg(theme::dim()),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(theme::bar_bg())),
            area,
        );
        return;
    }
    if let Some(input) = &app.rename {
        spans.push(Span::raw(tr(" Rename: ")).fg(theme::dim()));
        spans.push(Span::raw(input.text.as_str()).fg(theme::fg()).bold());
        spans.push(Span::raw("▏  ").fg(theme::fg()));
        spans.push(Span::raw(" Enter ").bold());
        spans.push(Span::raw(tr("rename ")).fg(theme::dim()));
        spans.push(Span::raw(" Esc ").bold());
        spans.push(Span::raw(tr("cancel")).fg(theme::dim()));
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(Style::new().fg(theme::fg()).bg(theme::bar_bg())),
            area,
        );
        return;
    }
    if let Some(job) = &app.job {
        let text = match (job.done * 100).checked_div(job.total) {
            Some(percent) => format!(" {}… {}%  ", job.label, percent.min(100)),
            None => format!(" {}…  ", job.label), // total still being measured
        };
        spans.push(Span::raw(text).fg(theme::fg()).bold());
    }
    // Left: what you are looking at; right: a few keys (cardea style). On a narrow screen the
    // keys go first, the information stays.
    if let Some(count) = app.entry_count() {
        spans.push(Span::raw(format!(" {}", format::items(count))).fg(theme::fg()));
    }
    if let Some(entry) = app.selected_entry() {
        spans.push(Span::raw(trf(" · “{}”", &[&entry.name])).fg(theme::dim()));
    }
    if !app.tab.marked.is_empty() {
        spans.push(
            Span::raw(trf(" · {} marked", &[&app.tab.marked.len()]))
                .fg(theme::accent())
                .bold(),
        );
    }
    if let Some(clip) = &app.clipboard {
        let template = match clip.mode {
            ClipMode::Copy => " · {} copied",
            ClipMode::Cut => " · {} cut",
        };
        spans.push(Span::raw(trf(template, &[&format::items(clip.paths.len())])).fg(theme::dim()));
    }
    if let Some(message) = &app.message {
        spans.push(Span::raw(format!("  {message}")).fg(theme::fg()));
    }
    let hints = if app.tab.focus == Focus::Preview {
        PREVIEW_HINTS
    } else {
        HINTS
    };
    let mut keys: Vec<Span> = Vec::new();
    for (key, what) in hints {
        keys.push(Span::raw(format!("[{key}]")).fg(theme::accent()).bold());
        keys.push(Span::raw(format!(" {}  ", tr(what))).fg(theme::dim()));
    }
    let left = Line::from(spans);
    let mut right = Line::from(keys);
    // Drop keys from the end until both fit.
    while !right.spans.is_empty() && left.width() + right.width() + 2 > usize::from(area.width) {
        right.spans.truncate(right.spans.len() - 2);
    }
    frame.render_widget(
        Paragraph::new("").style(Style::new().fg(theme::fg()).bg(theme::bar_bg())),
        area,
    );
    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(Paragraph::new(right.right_aligned()), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_hints_have_turkish() {
        use liman_core::i18n::turkish;
        // The English texts (the language is global; other tests may switch it).
        let help: Vec<(&str, &str)> = crate::app::HELP.iter().map(|h| h.english()).collect();
        let all = help
            .iter()
            .chain(HINTS)
            .chain(PREVIEW_HINTS)
            .chain(TERMINAL_HINTS);
        for (key, what) in all {
            assert!(turkish(what).is_some(), "no Turkish for {what:?}");
            // Key names with words in them ("double click", "header click") are translated too.
            let has_word = key
                .split(|c: char| !c.is_alphabetic())
                .any(|w| w.len() > 3 && w.chars().all(|c| c.is_lowercase()));
            assert!(
                !has_word || turkish(key).is_some(),
                "no Turkish for {key:?}"
            );
        }
    }
}

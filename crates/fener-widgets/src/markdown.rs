//! The Markdown reader (`Space u m`): the file shown formatted instead of as source. Headings
//! without `#`, bullets, task boxes, quotes, rules, framed code blocks, inline `code`, **bold**,
//! *italic* and [links](…) without their markers, wrapped at word boundaries. Ported from
//! liman's preview (`liman-widgets/src/markdown.rs`) to fener's theme.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::t;

/// One source line as styled spans (not wrapped yet). `in_code`: inside a fenced code block,
/// updated by fence lines. `width` is only used for horizontal rules.
pub fn line(text: &str, in_code: &mut bool, width: usize) -> Vec<Span<'static>> {
    let t = t();
    let trimmed = text.trim_start();
    let frame = Style::new().fg(t.gutter);
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *in_code = !*in_code;
        return if *in_code {
            let lang = trimmed.trim_start_matches(['`', '~']).trim();
            vec![
                Span::styled("┌─ ", frame),
                Span::styled(lang.to_string(), Style::new().fg(t.fg_dim)),
            ]
        } else {
            vec![Span::styled("└─", frame)]
        };
    }
    if *in_code {
        return vec![
            Span::styled("│ ", frame),
            Span::styled(text.to_string(), Style::new().fg(t.green)),
        ];
    }
    if is_rule(trimmed) {
        return vec![Span::styled("─".repeat(width.clamp(3, 60)), frame)];
    }
    if let Some((level, title)) = heading(trimmed) {
        let color = match level {
            1 => t.blue,
            2 => t.magenta,
            3 => t.cyan,
            _ => t.fg,
        };
        let mut style = Style::new().fg(color).add_modifier(Modifier::BOLD);
        if level == 1 {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        return vec![Span::styled(title.to_string(), style)];
    }
    if let Some(quote) = trimmed.strip_prefix('>') {
        let mut out = vec![Span::styled("▎ ", Style::new().fg(t.blue))];
        out.extend(
            inline(quote.trim_start())
                .into_iter()
                .map(|s| s.patch_style(Style::new().add_modifier(Modifier::ITALIC))),
        );
        return out;
    }
    let indent = &text[..text.len() - trimmed.len()];
    let (marker, rest) = if let Some(rest) = trimmed
        .strip_prefix("- [ ] ")
        .or_else(|| trimmed.strip_prefix("* [ ] "))
    {
        ("☐ ".to_string(), rest)
    } else if let Some(rest) = ["- [x] ", "- [X] ", "* [x] "]
        .iter()
        .find_map(|m| trimmed.strip_prefix(m))
    {
        ("☑ ".to_string(), rest)
    } else if let Some(rest) = ["- ", "* ", "+ "]
        .iter()
        .find_map(|m| trimmed.strip_prefix(m))
    {
        ("• ".to_string(), rest)
    } else {
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        match trimmed[digits..].strip_prefix(". ") {
            Some(rest) if digits > 0 => (format!("{}. ", &trimmed[..digits]), rest),
            _ => (String::new(), trimmed),
        }
    };
    let mut out = Vec::new();
    if !indent.is_empty() {
        out.push(Span::raw(indent.to_string()));
    }
    if !marker.is_empty() {
        out.push(Span::styled(marker, Style::new().fg(t.orange)));
    }
    out.extend(inline(rest));
    out
}

fn is_rule(line: &str) -> bool {
    let line = line.trim_end();
    line.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|c| line.chars().all(|x| x == *c || x == ' '))
}

/// `## Title` → (2, "Title").
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    let title = line[level..].strip_prefix(' ')?;
    (1..=6)
        .contains(&level)
        .then(|| (level, title.trim_end_matches(['#', ' '])))
}

/// Inline markup without its markers: `code`, **bold**, *italic* / _italic_, [text](url),
/// ![alt](image).
fn inline(text: &str) -> Vec<Span<'static>> {
    let t = t();
    let plain = Style::new().fg(t.fg);
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut rest = text;
    let flush = |buf: &mut String, out: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            out.push(Span::styled(std::mem::take(buf), plain));
        }
    };
    while let Some(c) = rest.chars().next() {
        let after_space = buf.is_empty() || buf.ends_with(' ');
        // (marker length, closing marker, style) for the markup starting here.
        let span: Option<(usize, &str, Style)> = if rest.starts_with("**") {
            Some((2, "**", plain.add_modifier(Modifier::BOLD)))
        } else if c == '`' {
            Some((1, "`", Style::new().fg(t.green).bg(t.bg_dark)))
        } else if (c == '*' || c == '_') && after_space {
            Some((
                1,
                if c == '*' { "*" } else { "_" },
                plain.add_modifier(Modifier::ITALIC),
            ))
        } else {
            None
        };
        if let Some((open, close, style)) = span
            && let Some(end) = rest[open..].find(close).filter(|&e| e > 0)
        {
            flush(&mut buf, &mut out);
            out.push(Span::styled(rest[open..open + end].to_string(), style));
            rest = &rest[open + end + close.len()..];
            continue;
        }
        // [text](url) and ![alt](src): the text only.
        let image = rest.starts_with("![");
        if (c == '[' || image)
            && let Some(close) = rest.find("](")
            && let Some(paren) = rest[close..].find(')')
        {
            let label = &rest[if image { 2 } else { 1 }..close];
            flush(&mut buf, &mut out);
            out.push(if image {
                Span::styled(format!("[image: {label}]"), Style::new().fg(t.fg_dim))
            } else {
                Span::styled(
                    label.to_string(),
                    Style::new().fg(t.blue).add_modifier(Modifier::UNDERLINED),
                )
            });
            rest = &rest[close + paren + 1..];
            continue;
        }
        buf.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut buf, &mut out);
    out
}

/// Breaks styled text into rows of at most `width` cells, at spaces when it can (a word
/// longer than a row is cut). Continuation rows keep the line's indent.
pub fn wrap(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let cells: Vec<(char, Style)> = spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    let width_of = |c: char| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    let indent = cells
        .iter()
        .take_while(|(c, _)| *c == ' ')
        .count()
        .min(width / 2);
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut start = 0;
    while start < cells.len() || rows.is_empty() {
        let prefix = if rows.is_empty() { 0 } else { indent };
        let room = width.saturating_sub(prefix).max(1);
        let (mut end, mut used) = (start, 0);
        while end < cells.len() && used + width_of(cells[end].0) <= room {
            used += width_of(cells[end].0);
            end += 1;
        }
        // Cut inside a word: go back to the last space after the row's own leading spaces.
        let lead = cells[start..end]
            .iter()
            .take_while(|(c, _)| *c == ' ')
            .count();
        if end < cells.len()
            && cells[end].0 != ' '
            && let Some(space) = cells[start..end].iter().rposition(|(c, _)| *c == ' ')
            && space >= lead
        {
            end = start + space + 1;
        }
        let end = end.max(start + 1).min(cells.len());
        let mut row = vec![(' ', Style::new()); prefix];
        let mut text = &cells[start..end];
        while let [rest @ .., (' ', _)] = text
            && !rest.is_empty()
        {
            text = rest;
        }
        row.extend_from_slice(text);
        rows.push(row);
        start = end;
        while start < cells.len() && cells[start].0 == ' ' {
            start += 1;
        }
        if cells.is_empty() {
            break;
        }
    }
    rows.into_iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (c, style) in row {
                match spans.last_mut() {
                    Some(last) if last.style == style => last.content.to_mut().push(c),
                    _ => spans.push(Span::styled(c.to_string(), style)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn markers_disappear() {
        let mut code = false;
        let mut show = |l: &str| text(&line(l, &mut code, 20));
        assert_eq!(show("# Title #"), "Title");
        assert_eq!(
            show("Some **bold** and `code` and *it*."),
            "Some bold and code and it."
        );
        assert_eq!(show("- see [docs](a.md)"), "• see docs");
        assert_eq!(show("- [x] done"), "☑ done");
        assert_eq!(show("```rust"), "┌─ rust");
        assert_eq!(show("# not a heading in code"), "│ # not a heading in code");
    }

    #[test]
    fn wraps_at_words_and_keeps_the_indent() {
        let rows: Vec<String> = wrap(vec![Span::raw("  one two three")], 9)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(rows, ["  one two", "  three"]);
        let long: Vec<String> = wrap(vec![Span::raw("abcdefgh")], 3)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(long, ["abc", "def", "gh"]);
    }
}

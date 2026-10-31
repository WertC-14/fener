//! The Markdown reader (`Space r`, Ctrl+E): the file shown formatted instead of as source. Headings
//! without `#`, bullets, task boxes, quotes, rules, framed code blocks, tables, inline `code`,
//! **bold**, *italic* and [links](…) without their markers; paragraphs wrapped at word boundaries. Ported from
//! liman's preview (`liman-widgets/src/markdown.rs`) to fener's theme.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::t;

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
    let (marker, rest) = list_marker(trimmed).unwrap_or((String::new(), trimmed));
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

/// A list item's marker as shown (`• `, `☐ `, `☑ `, `3. `) and the text after it.
fn list_marker(trimmed: &str) -> Option<(String, &str)> {
    if let Some(rest) = trimmed
        .strip_prefix("- [ ] ")
        .or_else(|| trimmed.strip_prefix("* [ ] "))
    {
        Some(("☐ ".to_string(), rest))
    } else if let Some(rest) = ["- [x] ", "- [X] ", "* [x] "]
        .iter()
        .find_map(|m| trimmed.strip_prefix(m))
    {
        Some(("☑ ".to_string(), rest))
    } else if let Some(rest) = ["- ", "* ", "+ "]
        .iter()
        .find_map(|m| trimmed.strip_prefix(m))
    {
        Some(("• ".to_string(), rest))
    } else {
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        match trimmed[digits..].strip_prefix(". ") {
            Some(rest) if digits > 0 => Some((format!("{}. ", &trimmed[..digits]), rest)),
            _ => None,
        }
    }
}

/// What a source line starts, for joining the lines of one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Blank,
    Fence,
    Rule,
    Heading,
    Quote,
    Item,
    Table,
    Text,
}

fn start_of(text: &str) -> Start {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        Start::Blank
    } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        Start::Fence
    } else if is_rule(trimmed) {
        Start::Rule
    } else if heading(trimmed).is_some() {
        Start::Heading
    } else if trimmed.starts_with('>') {
        Start::Quote
    } else if list_marker(trimmed).is_some() {
        Start::Item
    } else if trimmed.starts_with('|') {
        Start::Table
    } else {
        Start::Text
    }
}

/// Two spaces or a `\` at the end: Markdown's line break inside a paragraph.
fn hard_break(text: &str) -> bool {
    text.ends_with("  ") || text.ends_with('\\')
}

/// A line that is only an image (`![alt](src)`): kept on a row of its own, as screenshots in a
/// README are meant to be seen one by one.
fn image_only(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("![") && text.ends_with(')') && text.matches("](").count() == 1
}

/// Source lines formatted for the reader, until `rows` screen rows are filled. Markdown breaks a
/// paragraph only at a blank line (or after two spaces or `\`), so the lines of one paragraph,
/// list item or quote are joined and wrapped together; a run of `|` lines is drawn as a table.
/// `in_code`: the first line is inside a fenced code block.
pub fn layout(
    lines: impl Iterator<Item = String>,
    mut in_code: bool,
    width: usize,
    rows: usize,
) -> Vec<Line<'static>> {
    let mut lines = lines.peekable();
    let mut out = Vec::new();
    while out.len() < rows
        && let Some(first) = lines.next()
    {
        let start = start_of(&first);
        if in_code
            || !matches!(
                start,
                Start::Text | Start::Item | Start::Quote | Start::Table
            )
        {
            out.extend(wrap(line(&first, &mut in_code, width), width));
            continue;
        }
        if start == Start::Table {
            let mut table = vec![first];
            while let Some(next) = lines.next_if(|l| start_of(l) == Start::Table) {
                table.push(next);
            }
            out.extend(self::table(&table, width));
            continue;
        }
        // Four spaces before plain text: an indented code block, kept line by line.
        let indented_code = start == Start::Text && first.len() - first.trim_start().len() >= 4;
        let mut joined = first.trim_end_matches('\\').trim_end().to_string();
        let mut broken = hard_break(&first) || indented_code || image_only(&first);
        while !broken
            && let Some(next) = lines.next_if(|l| {
                let s = start_of(l);
                (s == Start::Text || (start == Start::Quote && s == Start::Quote)) && !image_only(l)
            })
        {
            let mut more = next.trim();
            if start == Start::Quote {
                more = more.strip_prefix('>').unwrap_or(more).trim_start();
            }
            joined.push(' ');
            joined.push_str(more.trim_end_matches('\\').trim_end());
            broken = hard_break(&next);
        }
        let trimmed = joined.trim_start();
        let indent = joined.len() - trimmed.len();
        // Rows after the first start under the text, past the bullet or the quote bar.
        let hang: Vec<(char, Style)> = match start {
            Start::Quote => vec![('▎', Style::new().fg(t().blue)), (' ', Style::new())],
            Start::Item => {
                let marker = list_marker(trimmed).map_or(0, |(m, _)| m.chars().count());
                vec![(' ', Style::new()); indent + marker]
            }
            _ => vec![(' ', Style::new()); indent],
        };
        out.extend(wrap_hanging(
            line(&joined, &mut in_code, width),
            width,
            &hang,
        ));
    }
    out
}

/// A table's cells (outer pipes dropped; `\|` stays a pipe inside a cell).
fn cells(row: &str) -> Vec<String> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    row.replace("\\|", "\u{0}")
        .split('|')
        .map(|c| c.trim().replace('\u{0}', "|"))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alignment {
    Left,
    Center,
    Right,
}

/// The `|---|:--:|` row under a table's header: each column's alignment.
fn alignments(row: &[String]) -> Option<Vec<Alignment>> {
    row.iter()
        .map(|c| {
            let dashes = c.trim_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|ch| ch == '-') {
                return None;
            }
            Some(match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => Alignment::Center,
                (false, true) => Alignment::Right,
                _ => Alignment::Left,
            })
        })
        .collect()
}

type Cell = Vec<(char, Style)>;

fn cell_width(cell: &[(char, Style)]) -> usize {
    cell.iter()
        .map(|(c, _)| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum()
}

/// A cell's text in exactly `w` columns: padded as `align` says, or cut with `…`.
fn fit(cell: &[(char, Style)], w: usize, align: Alignment) -> Cell {
    let width_of = |c: char| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    let mut text: Cell = Vec::new();
    let mut used = 0;
    for &(c, style) in cell {
        if used + width_of(c) > w {
            while used + 1 > w
                && let Some((gone, _)) = text.pop()
            {
                used -= width_of(gone);
            }
            text.push(('…', style));
            used += 1;
            break;
        }
        used += width_of(c);
        text.push((c, style));
    }
    let pad = w.saturating_sub(used);
    let (before, after) = match align {
        Alignment::Left => (0, pad),
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
    };
    let mut out = vec![(' ', Style::new()); before];
    out.extend(text);
    out.extend(vec![(' ', Style::new()); after]);
    out
}

/// A table with its columns lined up and ` │ ` between them, and a bold header with a rule
/// under it (left out when the header cells are all empty). Too wide for `width`: the widest
/// columns shrink and their text ends in `…`.
fn table(rows: &[String], width: usize) -> Vec<Line<'static>> {
    let t = t();
    let frame = Style::new().fg(t.gutter);
    let mut grid: Vec<Vec<String>> = rows.iter().map(|r| cells(r)).collect();
    // The separator is the second row when the table starts here; scrolled into a table, the
    // rows on screen have none.
    let align = grid.get(1).and_then(|r| alignments(r));
    let header = if align.is_some() {
        grid.remove(1);
        Some(grid.remove(0))
    } else {
        None
    };
    let columns = grid
        .iter()
        .chain(header.iter())
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    let align = align.unwrap_or_default();
    let styled = |row: &[String], bold: bool| -> Vec<Cell> {
        (0..columns)
            .map(|i| {
                inline(row.get(i).map_or("", String::as_str))
                    .iter()
                    .flat_map(|s| {
                        let style = if bold {
                            s.style.add_modifier(Modifier::BOLD)
                        } else {
                            s.style
                        };
                        s.content.chars().map(move |c| (c, style))
                    })
                    .collect()
            })
            .collect()
    };
    let header = header
        .filter(|h| h.iter().any(|c| !c.is_empty()))
        .map(|h| styled(&h, true));
    let body: Vec<Vec<Cell>> = grid.iter().map(|r| styled(r, false)).collect();
    let mut widths: Vec<usize> = (0..columns)
        .map(|i| {
            header
                .iter()
                .chain(&body)
                .map(|r| cell_width(&r[i]))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let room = width.saturating_sub(3 * columns.saturating_sub(1));
    while widths.iter().sum::<usize>() > room
        && let Some(widest) = widths.iter_mut().max().filter(|w| **w > 3)
    {
        *widest -= 1;
    }
    let draw = |row: &[Cell]| {
        let mut out = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.extend(" │ ".chars().map(|c| (c, frame)));
            }
            let a = align.get(i).copied().unwrap_or(Alignment::Left);
            out.extend(fit(cell, widths[i], a));
        }
        spans_line(out)
    };
    let mut out = Vec::new();
    if let Some(header) = &header {
        out.push(draw(header));
        let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
        out.push(Line::from(Span::styled(rule.join("─┼─"), frame)));
    }
    out.extend(body.iter().map(|r| draw(r)));
    out
}

/// Styled characters as a line, neighbors with the same style in one span.
fn spans_line(cells: Cell) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (c, style) in cells {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    Line::from(spans)
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
    let indent = spans
        .iter()
        .flat_map(|s| s.content.chars())
        .take_while(|c| *c == ' ')
        .count();
    wrap_hanging(spans, width, &vec![(' ', Style::new()); indent])
}

/// [`wrap`] with `hang` in front of every row after the first (at most half the width).
fn wrap_hanging(
    spans: Vec<Span<'static>>,
    width: usize,
    hang: &[(char, Style)],
) -> Vec<Line<'static>> {
    let cells: Vec<(char, Style)> = spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    let width_of = |c: char| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    let hang = &hang[..hang.len().min(width / 2)];
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut start = 0;
    while start < cells.len() || rows.is_empty() {
        let prefix: &[(char, Style)] = if rows.is_empty() { &[] } else { hang };
        let room = width.saturating_sub(cell_width(prefix)).max(1);
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
        let mut row = prefix.to_vec();
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
    rows.into_iter().map(spans_line).collect()
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

    fn layout_of(source: &str, width: usize) -> Vec<String> {
        layout(source.lines().map(String::from), false, width, 100)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn lines_of_a_paragraph_are_joined_and_wrapped_together() {
        let rows = layout_of("one two\nthree four\n\nnext", 40);
        assert_eq!(rows, ["one two three four", "", "next"]);
        // Two spaces or a backslash at the end keep the break.
        assert_eq!(layout_of("a  \nb\\\nc", 40), ["a", "b", "c"]);
        // Images on lines of their own stay apart.
        assert_eq!(
            layout_of("![one](a.png)\n![two](b.png)", 40),
            ["[image: one]", "[image: two]"]
        );
    }

    #[test]
    fn list_items_and_quotes_continue_under_their_text() {
        let rows = layout_of("- first item that\n  goes on\n- second", 14);
        assert_eq!(rows, ["• first item", "  that goes on", "• second"]);
        let rows = layout_of("> said one\n> two three", 12);
        assert_eq!(rows, ["▎ said one", "▎ two three"]);
    }

    #[test]
    fn code_stays_line_by_line() {
        let rows = layout_of("```\nlet a;\nlet b;\n```\n\n    indented\n    code", 40);
        assert_eq!(
            rows,
            [
                "┌─",
                "│ let a;",
                "│ let b;",
                "└─",
                "",
                "    indented",
                "    code"
            ]
        );
    }

    #[test]
    fn tables_line_up_their_columns() {
        let rows = layout_of(
            "| Key | What |\n|:---|---:|\n| `w` | save |\n| Space | menu |",
            40,
        );
        assert_eq!(
            rows,
            [
                "Key   │ What",
                "──────┼─────",
                "w     │ save",
                "Space │ menu",
            ]
        );
        // An empty header (a grid of keys) is left out; a narrow screen cuts the widest column.
        let rows = layout_of("| | |\n|---|---|\n| abcdefghij | x |", 9);
        assert_eq!(rows, ["abcd… │ x"]);
    }
}

//! Markdown shown formatted in the preview, not as source (like cardea's reader): headings
//! without `#`, bullets, task boxes, quotes, rules, framed code blocks, and inline `code`,
//! **bold**, *italic* and [links](…) without their markers, and tables. The lines of a paragraph
//! are joined and wrapped at words ([`layout`]); a block can start at any line, and only a code
//! fence carries over from the lines above (see `highlight::state_after`).

use liman_core::FileType;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;

/// One source line as styled spans (not wrapped yet). `in_code`: inside a fenced code block,
/// updated by fence lines. `width` is only used for horizontal rules.
pub fn line(text: &str, in_code: &mut bool, width: usize) -> Vec<Span<'static>> {
    let trimmed = text.trim_start();
    let frame = Style::new().fg(theme::border());
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *in_code = !*in_code;
        return if *in_code {
            let lang = trimmed.trim_start_matches(['`', '~']).trim();
            vec![
                Span::styled("┌─ ", frame),
                Span::styled(lang.to_string(), Style::new().fg(theme::dim())),
            ]
        } else {
            vec![Span::styled("└─", frame)]
        };
    }
    if *in_code {
        return vec![
            Span::styled("│ ", frame),
            Span::styled(text.to_string(), code_style()),
        ];
    }
    if is_rule(trimmed) {
        return vec![Span::styled("─".repeat(width.clamp(3, 60)), frame)];
    }
    if let Some((level, title)) = heading(trimmed) {
        let color = match level {
            1 => theme::accent(),
            2 => theme::type_color(FileType::Code),
            _ => theme::fg(),
        };
        let mut style = Style::new().fg(color).add_modifier(Modifier::BOLD);
        if level == 1 {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        return vec![Span::styled(title.to_string(), style)];
    }
    if let Some(quote) = trimmed.strip_prefix('>') {
        let mut out = vec![Span::styled("▎ ", Style::new().fg(theme::accent()))];
        out.extend(
            inline(quote.trim_start())
                .into_iter()
                .map(|s| s.patch_style(Style::new().add_modifier(Modifier::ITALIC))),
        );
        return out;
    }
    let indent = &text[..text.len() - trimmed.len()];
    let bullet = Style::new().fg(theme::type_color(FileType::Archive));
    let (marker, rest) = list_marker(trimmed).unwrap_or((String::new(), trimmed));
    let mut out = Vec::new();
    if !indent.is_empty() {
        out.push(Span::raw(indent.to_string()));
    }
    if !marker.is_empty() {
        out.push(Span::styled(marker, bullet));
    }
    out.extend(inline(rest));
    out
}

fn code_style() -> Style {
    Style::new().fg(theme::type_color(FileType::Spreadsheet))
}

/// A list item's marker as shown (`• `, `☐ `, `☑ `, `3. `) and the text after it.
fn list_marker(trimmed: &str) -> Option<(String, &str)> {
    if let Some(rest) = trimmed
        .strip_prefix("- [ ] ")
        .or_else(|| trimmed.strip_prefix("* [ ] "))
    {
        Some(("[ ] ".to_string(), rest))
    } else if let Some(rest) = ["- [x] ", "- [X] ", "* [x] "]
        .iter()
        .find_map(|m| trimmed.strip_prefix(m))
    {
        Some(("[✓] ".to_string(), rest))
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

/// Source lines formatted for the preview, until `rows` screen rows are filled. Markdown breaks a
/// paragraph only at a blank line (or after two spaces or `\`), so the lines of one paragraph,
/// list item or quote are joined and wrapped together; a run of `|` lines is drawn as a table.
/// `in_code`: the first line is inside a fenced code block. Also returns how many source lines
/// the rows show.
pub fn layout(
    lines: &[String],
    mut in_code: bool,
    width: usize,
    rows: usize,
) -> (Vec<Line<'static>>, usize) {
    let total = lines.len();
    let mut lines = lines.iter().cloned().peekable();
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
            Start::Quote => vec![('▎', Style::new().fg(theme::accent())), (' ', Style::new())],
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
    (out, total - lines.len())
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

/// Cells a character takes on screen (wide characters like emoji take two), measured by ratatui.
fn char_width(c: char) -> usize {
    Span::raw(&*c.encode_utf8(&mut [0; 4])).width()
}

fn cell_width(cell: &[(char, Style)]) -> usize {
    cell.iter().map(|(c, _)| char_width(*c)).sum()
}

/// A cell's text in exactly `w` columns: padded as `align` says, or cut with `…`.
fn fit(cell: &[(char, Style)], w: usize, align: Alignment) -> Cell {
    let width_of = char_width;
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
    let frame = Style::new().fg(theme::border());
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
    let width_of = char_width;
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

/// Breaks styled text into rows of at most `width` cells, at spaces when it can; continuation
/// rows keep the line's indent.
fn wrap(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let indent = spans
        .iter()
        .flat_map(|s| s.content.chars())
        .take_while(|c| *c == ' ')
        .count();
    wrap_hanging(spans, width, &vec![(' ', Style::new()); indent])
}

fn is_rule(line: &str) -> bool {
    let line = line.trim_end();
    line.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|c| line.chars().all(|x| x.to_string() == *c || x == ' '))
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
    let plain = Style::new().fg(theme::fg());
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
            Some((1, "`", code_style()))
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
            if image {
                out.push(Span::styled(
                    format!("[{}: {label}]", liman_core::i18n::tr("image")),
                    Style::new().fg(theme::dim()),
                ));
            } else {
                out.push(Span::styled(
                    label.to_string(),
                    Style::new()
                        .fg(theme::accent())
                        .add_modifier(Modifier::UNDERLINED),
                ));
            }
            rest = &rest[close + paren + 1..];
            continue;
        }
        buf.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut buf, &mut out);
    out
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
        assert_eq!(show("  - [x] done"), "  [✓] done");
        assert_eq!(show("2. second"), "2. second");
        assert_eq!(show("> quoted"), "▎ quoted");
        assert_eq!(show("---"), "─".repeat(20));
        assert_eq!(show("snake_case_name stays"), "snake_case_name stays");
        assert_eq!(show("![logo](logo.png)"), "[image: logo]");
    }

    #[test]
    fn code_blocks_are_framed_and_kept_as_is() {
        let mut code = false;
        assert_eq!(text(&line("```rust", &mut code, 20)), "┌─ rust");
        assert!(code);
        assert_eq!(
            text(&line("# not a heading", &mut code, 20)),
            "│ # not a heading"
        );
        assert_eq!(text(&line("```", &mut code, 20)), "└─");
        assert!(!code);
    }

    #[test]
    fn layout_joins_paragraphs_draws_tables_and_counts_lines() {
        let source: Vec<String> = [
            "one two",
            "three",
            "",
            "| a | b |",
            "|---|---|",
            "| 1 | 2 |",
            "rest",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (rows, used) = layout(&source, false, 20, 4);
        let rows: Vec<String> = rows
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(rows, ["one two three", "", "a │ b", "──┼──", "1 │ 2"]);
        // The table was taken whole: six source lines are on screen.
        assert_eq!(used, 6);
    }
}

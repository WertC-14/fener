//! Syntax colors from tree-sitter (fm-research ADR 0015) for 12 languages; other files keep the
//! scanner in `syntax.rs`. Each grammar's own `highlights.scm` names what it finds
//! (`keyword`, `function.method`, `string`, ...); those names map to fener's [`Kind`]s, so the
//! drawing and the themes stay as they are. Injections are followed (code in Markdown fences,
//! Markdown in Rust doc comments, inline Markdown).

use std::path::Path;
use std::sync::OnceLock;

use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::syntax::Kind;

/// Capture names fener knows, and their kinds. tree-sitter-highlight picks the most specific
/// listed name for a capture (`function.method` → `function`).
const NAMES: &[(&str, Kind)] = &[
    ("attribute", Kind::Marker),
    ("boolean", Kind::Number),
    ("character", Kind::Str),
    ("comment", Kind::Comment),
    ("conditional", Kind::Keyword),
    ("constant", Kind::Number),
    ("constant.builtin", Kind::Number),
    ("constructor", Kind::Type),
    ("escape", Kind::Number),
    ("exception", Kind::Keyword),
    ("float", Kind::Number),
    ("function", Kind::Function),
    ("include", Kind::Keyword),
    ("keyword", Kind::Keyword),
    ("label", Kind::Type),
    ("markup.heading", Kind::Heading),
    ("markup.link", Kind::Function),
    ("markup.list", Kind::Marker),
    ("markup.quote", Kind::Comment),
    ("markup.raw", Kind::Str),
    ("module", Kind::Type),
    ("namespace", Kind::Type),
    ("number", Kind::Number),
    ("operator", Kind::Plain),
    ("property", Kind::Plain),
    ("punctuation", Kind::Plain),
    ("punctuation.special", Kind::Marker),
    ("repeat", Kind::Keyword),
    ("storageclass", Kind::Keyword),
    ("string", Kind::Str),
    ("tag", Kind::Keyword),
    ("text.literal", Kind::Str),
    ("text.reference", Kind::Function),
    ("text.title", Kind::Heading),
    ("text.uri", Kind::Function),
    ("type", Kind::Type),
    ("variable", Kind::Plain),
    ("variable.builtin", Kind::Keyword),
    ("variable.parameter", Kind::Plain),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grammar {
    Rust,
    C,
    Cpp,
    CSharp,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Bash,
    Toml,
    Json,
    Markdown,
    MarkdownInline,
    Lua,
}

const ALL: [Grammar; 14] = [
    Grammar::Rust,
    Grammar::C,
    Grammar::Cpp,
    Grammar::CSharp,
    Grammar::Python,
    Grammar::JavaScript,
    Grammar::TypeScript,
    Grammar::Tsx,
    Grammar::Bash,
    Grammar::Toml,
    Grammar::Json,
    Grammar::Markdown,
    Grammar::MarkdownInline,
    Grammar::Lua,
];

impl Grammar {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::from_name(&ext)
    }

    /// By file extension or by the name an injection uses (```` ```rust ````).
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "rs" | "rust" => Self::Rust,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "c++" => Self::Cpp,
            "cs" | "csharp" | "c_sharp" => Self::CSharp,
            "py" | "python" => Self::Python,
            "js" | "mjs" | "cjs" | "jsx" | "javascript" => Self::JavaScript,
            "ts" | "typescript" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "sh" | "bash" | "zsh" => Self::Bash,
            "toml" => Self::Toml,
            "json" | "jsonc" => Self::Json,
            "md" | "markdown" => Self::Markdown,
            "markdown_inline" => Self::MarkdownInline,
            "lua" => Self::Lua,
            _ => return None,
        })
    }

    fn index(self) -> usize {
        ALL.iter().position(|g| *g == self).unwrap_or(0)
    }

    fn build(self) -> Option<HighlightConfiguration> {
        let js = || {
            (
                Language::new(tree_sitter_javascript::LANGUAGE),
                tree_sitter_javascript::HIGHLIGHT_QUERY.to_string(),
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            )
        };
        let (language, highlights, injections, locals): (Language, String, &str, &str) = match self
        {
            Self::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
                tree_sitter_rust::INJECTIONS_QUERY,
                "",
            ),
            Self::C => (
                tree_sitter_c::LANGUAGE.into(),
                tree_sitter_c::HIGHLIGHT_QUERY.into(),
                "",
                "",
            ),
            // C++ adds to the C query (its own file only has what C lacks).
            Self::Cpp => (
                tree_sitter_cpp::LANGUAGE.into(),
                format!(
                    "{}\n{}",
                    tree_sitter_cpp::HIGHLIGHT_QUERY,
                    tree_sitter_c::HIGHLIGHT_QUERY
                ),
                "",
                "",
            ),
            Self::CSharp => (
                tree_sitter_c_sharp::LANGUAGE.into(),
                tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
                "",
                "",
            ),
            Self::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY.into(),
                "",
                "",
            ),
            Self::JavaScript => {
                let (l, h, i, lo) = js();
                (
                    l,
                    format!("{h}\n{}", tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
                    i,
                    lo,
                )
            }
            // TypeScript's query adds to JavaScript's.
            Self::TypeScript | Self::Tsx => {
                let (_, h, _, lo) = js();
                let language = if self == Self::Tsx {
                    tree_sitter_typescript::LANGUAGE_TSX
                } else {
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT
                };
                (
                    language.into(),
                    format!("{}\n{h}", tree_sitter_typescript::HIGHLIGHTS_QUERY),
                    "",
                    lo,
                )
            }
            Self::Bash => (
                tree_sitter_bash::LANGUAGE.into(),
                tree_sitter_bash::HIGHLIGHT_QUERY.into(),
                "",
                "",
            ),
            Self::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
                "",
                "",
            ),
            Self::Json => (
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY.into(),
                "",
                "",
            ),
            Self::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
                tree_sitter_md::INJECTION_QUERY_BLOCK,
                "",
            ),
            Self::MarkdownInline => (
                tree_sitter_md::INLINE_LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into(),
                tree_sitter_md::INJECTION_QUERY_INLINE,
                "",
            ),
            Self::Lua => (
                tree_sitter_lua::LANGUAGE.into(),
                tree_sitter_lua::HIGHLIGHTS_QUERY.into(),
                tree_sitter_lua::INJECTIONS_QUERY,
                tree_sitter_lua::LOCALS_QUERY,
            ),
        };
        let mut config = HighlightConfiguration::new(
            language,
            format!("{self:?}"),
            &highlights,
            injections,
            locals,
        )
        .ok()?;
        let names: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
        config.configure(&names);
        Some(config)
    }
}

/// The configurations, built on first use (a query compiles in a few milliseconds).
fn config(grammar: Grammar) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; 14] =
        [const { OnceLock::new() }; 14];
    CONFIGS[grammar.index()]
        .get_or_init(|| grammar.build())
        .as_ref()
}

/// A colored stretch of the text, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// The colored stretches of `source` (sorted, not overlapping; plain text is left out), or
/// `None` when the grammar could not run.
pub fn highlight(grammar: Grammar, source: &str) -> Option<Vec<Span>> {
    let config = config(grammar)?;
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, source.as_bytes(), None, None, |name| {
            Grammar::from_name(name).and_then(self::config)
        })
        .ok()?;
    let mut spans: Vec<Span> = Vec::new();
    let mut stack: Vec<Kind> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(h) => stack.push(NAMES[h.0].1),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                // The innermost capture wins (a keyword inside a macro call, ...).
                let Some(&kind) = stack.last() else { continue };
                if kind == Kind::Plain {
                    continue;
                }
                match spans.last_mut() {
                    Some(last) if last.end == start && last.kind == kind => last.end = end,
                    _ => spans.push(Span { start, end, kind }),
                }
            }
        }
    }
    Some(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(grammar: Grammar, source: &str) -> Vec<(String, Kind)> {
        highlight(grammar, source)
            .unwrap()
            .into_iter()
            .map(|s| (source[s.start..s.end].to_string(), s.kind))
            .collect()
    }

    #[test]
    fn every_grammar_builds() {
        for g in ALL {
            assert!(config(g).is_some(), "{g:?}");
        }
    }

    #[test]
    fn rust_parts() {
        let k = kinds(Grammar::Rust, "fn main() { let x: u8 = 1; // hi\n}\n");
        assert!(k.contains(&("fn".into(), Kind::Keyword)));
        assert!(k.contains(&("main".into(), Kind::Function)));
        assert!(k.contains(&("u8".into(), Kind::Type)));
        assert!(k.contains(&("1".into(), Kind::Number)));
        assert!(k.contains(&("// hi".into(), Kind::Comment)));
    }

    #[test]
    fn python_and_markdown_injection() {
        let k = kinds(Grammar::Python, "def f():\n    return \"x\"\n");
        assert!(k.contains(&("def".into(), Kind::Keyword)));
        assert!(k.contains(&("\"x\"".into(), Kind::Str)));
        // A fenced Rust block inside Markdown gets Rust colors.
        let k = kinds(Grammar::Markdown, "# T\n\n```rust\nfn a() {}\n```\n");
        assert!(
            k.iter()
                .any(|(t, kind)| t == "fn" && *kind == Kind::Keyword),
            "{k:?}"
        );
    }
}

#[cfg(test)]
mod timing {
    /// `cargo test --release -p fener-core timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn big_file() {
        let text = include_str!("editor.rs");
        for round in 0..3 {
            let start = std::time::Instant::now();
            let spans = super::highlight(super::Grammar::Rust, text).unwrap();
            println!(
                "round {round}: {} lines, {} spans, {:?}",
                text.lines().count(),
                spans.len(),
                start.elapsed()
            );
        }
    }
}

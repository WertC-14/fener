//! Syntax highlighting without a parser: what each character of a line is (comment, string,
//! keyword, ...). The UI turns the kinds into theme colors. Ported from liman's preview
//! colorizer (`liman-widgets/src/highlight.rs`), returning kinds instead of styles, plus function
//! and macro names (a word followed by `(` or `!`). fm-research `notes/dil-destegi.md`: a
//! tree-sitter highlighter can later produce the same kinds.
//!
//! Lines are independent except for `/* */` comments and Markdown code fences, which carry over
//! in [`State`]; [`state_after`] finds the state at any line without highlighting what is above.

use std::path::Path;

use ropey::Rope;

/// A family of languages that share comment syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// `//` and `/* */`: Rust, C, C++, Go, Java, JS/TS, Swift, Kotlin, C#, Zig, CSS, PHP, JSON...
    CLike,
    /// `#`: Python, shell, Ruby, TOML, YAML, Makefile, config files.
    Hash,
    /// `--`: SQL, Lua, Haskell.
    Dash,
    Markdown,
    Plain,
}

impl Lang {
    pub fn from_path(path: &Path) -> Self {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(
            name,
            "Makefile" | "makefile" | "Dockerfile" | ".gitignore" | ".env"
        ) {
            return Self::Hash;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext.to_ascii_lowercase().as_str() {
            "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "go" | "java" | "kt" | "swift" | "js"
            | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "cs" | "zig" | "css" | "scss" | "php"
            | "json" | "jsonc" | "dart" | "scala" | "proto" | "glsl" | "wgsl" => Self::CLike,
            "py" | "sh" | "bash" | "zsh" | "fish" | "rb" | "toml" | "yaml" | "yml" | "conf"
            | "cfg" | "ini" | "env" | "mk" | "r" | "pl" | "nix" | "ex" | "exs" => Self::Hash,
            "sql" | "lua" | "hs" => Self::Dash,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Plain,
        }
    }
}

/// What a character is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Comment,
    Str,
    Number,
    Keyword,
    /// A capitalized name (types, constants, enum variants).
    Type,
    /// A name before `(` or `!` (calls, definitions, macros).
    Function,
    /// Markdown: headings.
    Heading,
    /// Markdown: list markers, `>` quotes.
    Marker,
}

/// Carried from line to line: inside a `/* */` comment or a Markdown code fence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    block_comment: bool,
    fence: bool,
}

impl State {
    /// Inside a Markdown code fence (for the formatted reader).
    pub fn in_fence(self) -> bool {
        self.fence
    }
}

const KEYWORDS: &[&str] = &[
    // shared by many languages
    "if",
    "else",
    "for",
    "while",
    "loop",
    "do",
    "break",
    "continue",
    "return",
    "match",
    "switch",
    "case",
    "default",
    "in",
    "of",
    "as",
    "is",
    "not",
    "and",
    "or",
    "true",
    "false",
    "null",
    "nil",
    "None",
    "True",
    "False",
    "self",
    "Self",
    "this",
    "super",
    "new",
    "try",
    "catch",
    "except",
    "finally",
    "throw",
    "raise",
    "with",
    "yield",
    "await",
    "async",
    "import",
    "from",
    "export",
    "package",
    "use",
    "mod",
    "pub",
    "fn",
    "func",
    "function",
    "def",
    "class",
    "struct",
    "enum",
    "trait",
    "impl",
    "interface",
    "type",
    "let",
    "const",
    "var",
    "val",
    "static",
    "mut",
    "ref",
    "where",
    "public",
    "private",
    "protected",
    "void",
    "int",
    "bool",
    "char",
    "float",
    "double",
    "string",
    "lambda",
    "pass",
    "then",
    "fi",
    "done",
    "esac",
    "elif",
    "end",
    "local",
    "select",
    "insert",
    "update",
    "delete",
    "create",
    "table",
    "into",
    "values",
    "unsafe",
    "extern",
    "crate",
    "dyn",
    "move",
    "go",
    "defer",
    "chan",
    "map",
    "echo",
    "unless",
    "begin",
    "module",
    "require",
];

/// The state at the start of line `line` (everything above scanned, nothing highlighted).
pub fn state_after(lang: Lang, rope: &Rope, line: usize) -> State {
    let mut state = State::default();
    match lang {
        Lang::CLike => {
            let mut kinds = Vec::new();
            for l in rope.lines().take(line) {
                let text = String::from(l);
                // Only a `/*` can open a comment and only a `*/` can close it.
                let marker = if state.block_comment { "*/" } else { "/*" };
                if text.contains(marker) {
                    kinds.clear();
                    code(lang, &text, &mut state, &mut kinds);
                }
            }
        }
        Lang::Markdown => {
            for l in rope.lines().take(line) {
                if is_fence(&String::from(l)) {
                    state.fence = !state.fence;
                }
            }
        }
        Lang::Hash | Lang::Dash | Lang::Plain => {}
    }
    state
}

/// The kind of each character of `line` (one per char); `state` is updated for the next line.
pub fn line_kinds(lang: Lang, line: &str, state: &mut State) -> Vec<Kind> {
    let mut kinds = Vec::with_capacity(line.len());
    match lang {
        Lang::Plain => kinds.resize(line.chars().count(), Kind::Plain),
        Lang::Markdown => markdown(line, state, &mut kinds),
        _ => code(lang, line, state, &mut kinds),
    }
    kinds
}

fn code(lang: Lang, line: &str, state: &mut State, kinds: &mut Vec<Kind>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let put = |kinds: &mut Vec<Kind>, kind: Kind, end: usize| {
        kinds.resize(end.max(kinds.len()), kind);
    };
    while i < chars.len() {
        // Inside a block comment: until */.
        if state.block_comment {
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            if i < chars.len() {
                i += 2;
                state.block_comment = false;
            }
            put(kinds, Kind::Comment, i.min(chars.len()));
            continue;
        }
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let line_comment = match lang {
            Lang::CLike => c == '/' && next == Some('/'),
            Lang::Hash => c == '#',
            Lang::Dash => c == '-' && next == Some('-'),
            _ => false,
        };
        if line_comment {
            put(kinds, Kind::Comment, chars.len());
            return;
        }
        if lang == Lang::CLike && c == '/' && next == Some('*') {
            state.block_comment = true;
            continue;
        }
        if c == '"' || c == '\'' || c == '`' {
            // A Rust lifetime ('a) is not a string: only take ' when it closes soon.
            let close = chars[i + 1..]
                .iter()
                .position(|&x| x == c)
                .map(|p| i + 1 + p);
            let is_string = match (c, close) {
                ('\'', Some(end)) => lang != Lang::CLike || end - i <= 3 || chars[i + 1] == '\\',
                (_, Some(_)) => true,
                (_, None) => c == '"',
            };
            if is_string {
                let mut end = i + 1;
                while end < chars.len() && chars[end] != c {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                i = (end + 1).min(chars.len());
                put(kinds, Kind::Str, i);
                continue;
            }
        }
        let after_word = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        if c.is_ascii_digit() && !after_word {
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            put(kinds, Kind::Number, i);
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if KEYWORDS.contains(&word.as_str()) {
                Kind::Keyword
            } else if matches!(chars.get(i), Some('(' | '!')) {
                Kind::Function
            } else if word.chars().next().is_some_and(char::is_uppercase) && word.len() > 1 {
                Kind::Type
            } else {
                Kind::Plain
            };
            // A macro's `!` belongs to its name.
            let end = if kind == Kind::Function && chars.get(i) == Some(&'!') {
                i + 1
            } else {
                i
            };
            put(kinds, kind, end);
            i = end;
            continue;
        }
        i += 1;
        put(kinds, Kind::Plain, i);
    }
}

/// A Markdown code fence line (```` ``` ```` or `~~~`), which opens or closes a code block.
fn is_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

/// Markdown source: headings, fences and code, quotes, list markers, inline `code`.
fn markdown(line: &str, state: &mut State, kinds: &mut Vec<Kind>) {
    let n = line.chars().count();
    let trimmed = line.trim_start();
    let indent = n - trimmed.chars().count();
    if is_fence(line) {
        state.fence = !state.fence;
        kinds.resize(n, Kind::Comment);
        return;
    }
    if state.fence {
        kinds.resize(n, Kind::Str);
        return;
    }
    if trimmed.starts_with('#') {
        kinds.resize(n, Kind::Heading);
        return;
    }
    if trimmed.starts_with('>') {
        kinds.resize(n, Kind::Comment);
        return;
    }
    kinds.resize(indent, Kind::Plain);
    let marker = ["- [ ] ", "- [x] ", "- ", "* ", "+ "]
        .iter()
        .find(|m| trimmed.starts_with(*m))
        .map(|m| m.len())
        .or_else(|| {
            let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && trimmed[digits..].starts_with(". ")).then_some(digits + 2)
        })
        .unwrap_or(0);
    kinds.resize(indent + marker, Kind::Marker);
    let mut in_code = false;
    for c in trimmed.chars().skip(marker) {
        if c == '`' {
            in_code = !in_code;
            kinds.push(Kind::Str);
        } else {
            kinds.push(if in_code { Kind::Str } else { Kind::Plain });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds of a line as a string, one letter per char: p c s n k t f h m.
    fn show(lang: Lang, line: &str, state: &mut State) -> String {
        line_kinds(lang, line, state)
            .iter()
            .map(|k| match k {
                Kind::Plain => 'p',
                Kind::Comment => 'c',
                Kind::Str => 's',
                Kind::Number => 'n',
                Kind::Keyword => 'k',
                Kind::Type => 't',
                Kind::Function => 'f',
                Kind::Heading => 'h',
                Kind::Marker => 'm',
            })
            .collect()
    }

    #[test]
    fn rust_line() {
        let mut s = State::default();
        assert_eq!(
            show(Lang::CLike, r#"let x = f("a", 1); // hi"#, &mut s),
            "kkkpppppfpsssppnpppccccc"
        );
        assert_eq!(show(Lang::CLike, "println!(Foo)", &mut s), "ffffffffptttp");
        // A lifetime is not a string.
        assert_eq!(show(Lang::CLike, "&'a str", &mut s), "ppppppp");
    }

    #[test]
    fn block_comments_carry_over() {
        let rope = Rope::from_str("a /* one\ntwo\nthree */ b\nc\n");
        let mut s = state_after(Lang::CLike, &rope, 1);
        assert_eq!(show(Lang::CLike, "two", &mut s), "ccc");
        assert_eq!(show(Lang::CLike, "three */ b", &mut s), "ccccccccpp");
        assert_eq!(state_after(Lang::CLike, &rope, 3), State::default());
    }

    #[test]
    fn markdown_source() {
        let mut s = State::default();
        assert_eq!(show(Lang::Markdown, "# Hi", &mut s), "hhhh");
        assert_eq!(show(Lang::Markdown, "- a `b`", &mut s), "mmppsss");
        assert_eq!(show(Lang::Markdown, "```rs", &mut s), "ccccc");
        assert_eq!(show(Lang::Markdown, "x", &mut s), "s");
    }
}

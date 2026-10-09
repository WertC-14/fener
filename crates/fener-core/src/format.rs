//! `Space c f`: formatting by the language's own tool (LazyVim does it with conform.nvim). The
//! text goes to the tool's stdin and comes back on its stdout; the app runs it on a worker and
//! hands the result to [`crate::Editor::formatted`], which replaces the text as one undo step.
//! `[format]` in the config file overrides a command (`{file}` is the file's name).

use std::path::Path;

/// The shell command that formats `file` from stdin to stdout, if fener knows one.
pub fn command_for(file: &Path) -> Option<String> {
    let name = quote(file.file_name()?.to_str()?);
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    Some(match ext {
        "rs" => "rustfmt --edition 2024 --emit stdout".into(),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "cs" | "java" => {
            format!("clang-format --assume-filename={name}")
        }
        "py" => format!("ruff format --stdin-filename {name} -"),
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "json" | "css" | "scss" | "html" | "md"
        | "yaml" | "yml" => format!("prettier --stdin-filepath {name}"),
        "go" => "gofmt".into(),
        "lua" => "stylua -".into(),
        "sh" | "bash" => "shfmt".into(),
        "toml" => "taplo fmt -".into(),
        _ => return None,
    })
}

/// Fills `{file}` in a command from the config file.
pub fn fill(template: &str, file: &Path) -> String {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .map(quote)
        .unwrap_or_default();
    template.replace("{file}", &name)
}

fn quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '+'))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_by_language() {
        assert_eq!(
            command_for(Path::new("/x/a.rs")).unwrap(),
            "rustfmt --edition 2024 --emit stdout"
        );
        assert_eq!(
            command_for(Path::new("/x/my lab.c")).unwrap(),
            "clang-format --assume-filename='my lab.c'"
        );
        assert!(command_for(Path::new("/x/notes.txt")).is_none());
        assert_eq!(
            fill("black -q --stdin-filename {file} -", Path::new("/a/b.py")),
            "black -q --stdin-filename b.py -"
        );
    }
}

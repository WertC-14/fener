//! F5: how to build and run the open file. A project file (Cargo, Go module, Makefile) runs the
//! project; a lone file is compiled and run by its language's tool, the way one would type it
//! (`rustc lab-1.rs && ./lab-1`). The app types the command into the code tab's terminal, so the
//! program's output stays visible and it can read input.

use std::path::{Path, PathBuf};

/// The folder to run in and the shell command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub dir: PathBuf,
    pub command: String,
}

/// How to run `file`, or `None` for a file type fener does not know how to run.
pub fn command_for(file: &Path) -> Option<Run> {
    let dir = file.parent()?.to_path_buf();
    let name = file.file_name()?.to_str()?;
    let stem = file.file_stem()?.to_str()?;
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    let run = |dir: &Path, command: String| {
        Some(Run {
            dir: dir.to_path_buf(),
            command,
        })
    };

    if ext == "rs"
        && let Some(cargo) = cargo_project(file)
    {
        let rel = file.strip_prefix(&cargo).ok()?;
        let parts: Vec<&str> = rel.iter().filter_map(|p| p.to_str()).collect();
        let command = match parts.as_slice() {
            ["src", "bin", _] => format!("cargo run --bin {}", quote(stem)),
            ["src", "bin", bin, "main.rs"] => format!("cargo run --bin {}", quote(bin)),
            ["examples", _] => format!("cargo run --example {}", quote(stem)),
            ["tests", _] => format!("cargo test --test {}", quote(stem)),
            _ => "cargo run".to_string(),
        };
        return run(&cargo, command);
    }
    if ext == "go"
        && let Some(module) = ancestor_with(&dir, "go.mod")
    {
        return run(&module, "go run .".into());
    }
    if dir.join("Makefile").exists() && matches!(ext, "c" | "cpp" | "cc" | "h" | "hpp") {
        return run(&dir, "make".into());
    }
    let (file, bin) = (quote(name), quote(&format!("./{stem}")));
    let command = match ext {
        "rs" => format!("rustc {file} && {bin}"),
        "c" => format!("cc {file} -o {} && {bin}", quote(stem)),
        "cpp" | "cc" | "cxx" => format!("c++ {file} -o {} && {bin}", quote(stem)),
        "go" => format!("go run {file}"),
        "py" => format!("python3 {file}"),
        "js" | "mjs" | "cjs" => format!("node {file}"),
        "ts" => format!("npx tsx {file}"),
        "sh" | "bash" => format!("bash {file}"),
        "zsh" => format!("zsh {file}"),
        "fish" => format!("fish {file}"),
        "lua" => format!("lua {file}"),
        "rb" => format!("ruby {file}"),
        "java" => format!("java {file}"),
        "php" => format!("php {file}"),
        _ => return None,
    };
    run(&dir, command)
}

/// The Cargo package `file` belongs to: the nearest folder above it with a `Cargo.toml` that
/// has `file` under `src/`, `examples/`, `tests/` or `benches/` (a stray `.rs` elsewhere is
/// compiled on its own).
fn cargo_project(file: &Path) -> Option<PathBuf> {
    let package = file
        .ancestors()
        .skip(1)
        .find(|d| d.join("Cargo.toml").exists())?;
    let rel = file.strip_prefix(package).ok()?;
    let top = rel.iter().next()?.to_str()?;
    (matches!(top, "src" | "examples" | "tests" | "benches") || rel == Path::new("build.rs"))
        .then(|| package.to_path_buf())
}

fn ancestor_with(dir: &Path, marker: &str) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(marker).exists())
        .map(Path::to_path_buf)
}

/// Quotes for the shell when needed (`lab-1.rs` stays as it is, `my file.rs` gets quotes).
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
    use std::fs;

    #[test]
    fn single_files_and_cargo_projects() {
        let base = std::env::temp_dir().join(format!("fener-run-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let lab = base.join("1/lab");
        let pkg = base.join("pkg");
        fs::create_dir_all(&lab).unwrap();
        fs::create_dir_all(pkg.join("src/bin")).unwrap();
        fs::write(base.join("Cargo.toml"), "").unwrap(); // a workspace file above the lab
        fs::write(pkg.join("Cargo.toml"), "").unwrap();

        let cmd = |p: &Path| command_for(p).map(|r| (r.dir, r.command));
        // A lab file outside any src/ is compiled on its own, next to itself.
        assert_eq!(
            cmd(&lab.join("lab-1.rs")),
            Some((lab.clone(), "rustc lab-1.rs && ./lab-1".into()))
        );
        assert_eq!(
            cmd(&pkg.join("src/main.rs")),
            Some((pkg.clone(), "cargo run".into()))
        );
        assert_eq!(
            cmd(&pkg.join("src/bin/tool.rs")),
            Some((pkg.clone(), "cargo run --bin tool".into()))
        );
        assert_eq!(
            cmd(&lab.join("my file.py")).unwrap().1,
            "python3 'my file.py'"
        );
        assert_eq!(cmd(&lab.join("notes.md")), None);
        fs::remove_dir_all(&base).unwrap();
    }
}

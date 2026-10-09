//! A language server client (fm-research ADR 0016): JSON-RPC over the server's stdin / stdout,
//! with `Content-Length` frames. No async runtime: a thread reads the server and hands every
//! answer to a callback (the app turns it into an event); writing happens on the caller's
//! thread. Messages sent before the server answered `initialize` wait in a queue.
//!
//! Positions stay in the server's units (UTF-8 bytes or UTF-16 code units, whichever it chose;
//! LSP's default is UTF-16); [`to_char`] and [`from_char`] convert with the document's text.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use ropey::Rope;
use serde_json::{Value, json};

/// How the server counts columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A position in the server's units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub character: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: Pos,
    pub end: Pos,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// What goes in (snippet placeholders removed).
    pub insert: String,
    /// Type or signature, shown dimmed.
    pub detail: String,
    pub kind: &'static str,
}

/// What a request was, so its answer can be routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Initialize,
    Hover(PathBuf),
    Definition(PathBuf),
    References(PathBuf),
    Completion(PathBuf, u64),
}

/// What the server said, for the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `initialize` answered; columns are counted in `Encoding`.
    Ready(Encoding),
    Diagnostics {
        path: PathBuf,
        items: Vec<Diagnostic>,
    },
    Hover {
        path: PathBuf,
        text: String,
    },
    Definition {
        path: PathBuf,
        items: Vec<Location>,
    },
    References {
        path: PathBuf,
        items: Vec<Location>,
    },
    /// `version`: the document version the completion was asked for.
    Completion {
        path: PathBuf,
        version: u64,
        items: Vec<Completion>,
    },
    /// The server could not answer a question about `path` (still loading, ...).
    Notice {
        path: PathBuf,
        text: String,
    },
    /// The server could not start or stopped.
    Failed(String),
}

pub struct Client {
    stdin: Arc<Mutex<ChildStdin>>,
    child: Child,
    next_id: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, Asked>>>,
    /// Messages waiting for the `initialize` answer; `None` once it came.
    queue: Arc<Mutex<Option<Vec<Value>>>>,
    encoding: Arc<Mutex<Encoding>>,
}

impl Client {
    /// Starts `command` for the project at `root`; every answer goes to `on_event`.
    pub fn start(
        command: &[String],
        root: &Path,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> io::Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| io::Error::other("empty server command"))?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        let stdout = child.stdout.take().expect("piped");
        let client = Self {
            stdin,
            child,
            next_id: AtomicI64::new(1),
            pending: Arc::default(),
            queue: Arc::new(Mutex::new(Some(Vec::new()))),
            encoding: Arc::default(),
        };
        let reader = Reader {
            stdin: client.stdin.clone(),
            pending: client.pending.clone(),
            queue: client.queue.clone(),
            encoding: client.encoding.clone(),
        };
        std::thread::spawn(move || reader.run(BufReader::new(stdout), on_event));

        let root_uri = uri(root);
        let id = client.next_id.fetch_add(1, Ordering::Relaxed);
        client
            .pending
            .lock()
            .expect("lock")
            .insert(id, Asked::Initialize);
        // Sent directly: everything else waits behind it in the queue.
        write_message(
            &client.stdin,
            &json!({
                "jsonrpc": "2.0", "id": id, "method": "initialize",
                "params": {
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "workspaceFolders": [{"uri": root_uri, "name": root.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()}],
                    "capabilities": {
                        "general": {"positionEncodings": ["utf-8", "utf-16"]},
                        "textDocument": {
                            "synchronization": {"didSave": true},
                            "publishDiagnostics": {"relatedInformation": false},
                            "hover": {"contentFormat": ["markdown", "plaintext"]},
                            "definition": {"linkSupport": false},
                            "references": {},
                            "completion": {"completionItem": {"snippetSupport": true, "documentationFormat": ["plaintext"]}}
                        }
                    }
                }
            }),
        )?;
        Ok(client)
    }

    pub fn encoding(&self) -> Encoding {
        *self.encoding.lock().expect("lock")
    }

    pub fn open(&self, path: &Path, version: u64, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri(path), "languageId": language_id(path), "version": version, "text": text}}),
        );
    }

    /// The whole text again (full sync: simple, and fast enough for source files).
    pub fn change(&self, path: &Path, version: u64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri(path), "version": version}, "contentChanges": [{"text": text}]}),
        );
    }

    pub fn save(&self, path: &Path) {
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri(path)}}),
        );
    }

    pub fn close(&self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri(path)}}),
        );
    }

    pub fn hover(&self, path: &Path, pos: Pos) {
        self.request(
            "textDocument/hover",
            at(path, pos),
            Asked::Hover(path.into()),
        );
    }

    pub fn definition(&self, path: &Path, pos: Pos) {
        self.request(
            "textDocument/definition",
            at(path, pos),
            Asked::Definition(path.into()),
        );
    }

    pub fn references(&self, path: &Path, pos: Pos) {
        let mut params = at(path, pos);
        params["context"] = json!({"includeDeclaration": true});
        self.request(
            "textDocument/references",
            params,
            Asked::References(path.into()),
        );
    }

    pub fn completion(&self, path: &Path, pos: Pos, version: u64) {
        self.request(
            "textDocument/completion",
            at(path, pos),
            Asked::Completion(path.into(), version),
        );
    }

    fn request(&self, method: &str, params: Value, asked: Asked) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().expect("lock").insert(id, asked);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn send(&self, message: Value) {
        if let Some(queue) = self.queue.lock().expect("lock").as_mut() {
            queue.push(message);
            return;
        }
        let _ = write_message(&self.stdin, &message);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = write_message(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "id": 0, "method": "shutdown"}),
        );
        let _ = write_message(&self.stdin, &json!({"jsonrpc": "2.0", "method": "exit"}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The reading side, on its own thread.
struct Reader {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<i64, Asked>>>,
    queue: Arc<Mutex<Option<Vec<Value>>>>,
    encoding: Arc<Mutex<Encoding>>,
}

impl Reader {
    fn run(self, mut out: impl BufRead, on_event: impl Fn(Event)) {
        loop {
            let message = match read_message(&mut out) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    on_event(Event::Failed("the language server stopped".into()));
                    return;
                }
                Err(e) => {
                    on_event(Event::Failed(e.to_string()));
                    return;
                }
            };
            // A request from the server (configuration, progress, ...): answer "nothing".
            if message.get("method").is_some() && message.get("id").is_some() {
                let reply = json!({"jsonrpc": "2.0", "id": message["id"], "result": null});
                let _ = write_message(&self.stdin, &reply);
                continue;
            }
            if message["method"] == "textDocument/publishDiagnostics" {
                let params = &message["params"];
                if let Some(path) = params["uri"].as_str().and_then(path_of) {
                    let items = params["diagnostics"]
                        .as_array()
                        .map(|a| a.iter().filter_map(diagnostic).collect())
                        .unwrap_or_default();
                    on_event(Event::Diagnostics { path, items });
                }
                continue;
            }
            let Some(id) = message["id"].as_i64() else {
                continue;
            };
            let Some(asked) = self.pending.lock().expect("lock").remove(&id) else {
                continue;
            };
            // An error answer: said plainly instead of looking like "nothing found".
            if let Some(error) = message.get("error") {
                let path = match &asked {
                    Asked::Hover(p) | Asked::Definition(p) | Asked::References(p) => {
                        Some(p.clone())
                    }
                    Asked::Completion(..) | Asked::Initialize => None,
                };
                if let Some(path) = path {
                    let text = if error["code"] == -32801 || error["code"] == -32800 {
                        "The language server is still loading: try again in a moment".to_string()
                    } else {
                        format!(
                            "Language server: {}",
                            error["message"].as_str().unwrap_or("error")
                        )
                    };
                    on_event(Event::Notice { path, text });
                }
                continue;
            }
            let result = &message["result"];
            if std::env::var_os("FENER_LSP_DEBUG").is_some() {
                eprintln!("<- {message}");
            }
            let event = match asked {
                Asked::Initialize => {
                    let encoding = match result["capabilities"]["positionEncoding"].as_str() {
                        Some("utf-8") => Encoding::Utf8,
                        _ => Encoding::Utf16,
                    };
                    *self.encoding.lock().expect("lock") = encoding;
                    let _ = write_message(
                        &self.stdin,
                        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
                    );
                    // What waited for the handshake goes out now, in order.
                    let waiting = self.queue.lock().expect("lock").take().unwrap_or_default();
                    for m in waiting {
                        let _ = write_message(&self.stdin, &m);
                    }
                    Event::Ready(encoding)
                }
                Asked::Hover(path) => Event::Hover {
                    path,
                    text: hover_text(&result["contents"]),
                },
                Asked::Definition(path) => Event::Definition {
                    path,
                    items: locations(result),
                },
                Asked::References(path) => Event::References {
                    path,
                    items: locations(result),
                },
                Asked::Completion(path, version) => {
                    let list = result.get("items").unwrap_or(result);
                    let items = list
                        .as_array()
                        .map(|a| a.iter().filter_map(completion).collect())
                        .unwrap_or_default();
                    Event::Completion {
                        path,
                        version,
                        items,
                    }
                }
            };
            on_event(event);
        }
    }
}

fn write_message(stdin: &Mutex<ChildStdin>, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    let mut stdin = stdin.lock().map_err(|_| io::Error::other("poisoned"))?;
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    stdin.flush()
}

/// One framed message; `None` at the end of the stream.
fn read_message(out: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if out.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(n) = line.strip_prefix("Content-Length:") {
            length = n.trim().parse::<usize>().ok();
        }
    }
    let length = length.ok_or_else(|| io::Error::other("message without Content-Length"))?;
    let mut body = vec![0; length];
    out.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

fn at(path: &Path, pos: Pos) -> Value {
    json!({"textDocument": {"uri": uri(path)}, "position": {"line": pos.line, "character": pos.character}})
}

fn pos(value: &Value) -> Option<Pos> {
    Some(Pos {
        line: value["line"].as_u64()? as usize,
        character: value["character"].as_u64()? as usize,
    })
}

fn diagnostic(value: &Value) -> Option<Diagnostic> {
    Some(Diagnostic {
        start: pos(&value["range"]["start"])?,
        end: pos(&value["range"]["end"])?,
        severity: match value["severity"].as_u64() {
            Some(2) => Severity::Warning,
            Some(3) => Severity::Info,
            Some(4) => Severity::Hint,
            _ => Severity::Error,
        },
        message: value["message"].as_str()?.to_string(),
    })
}

/// `Location`, `Location[]` or `LocationLink[]`.
fn locations(result: &Value) -> Vec<Location> {
    let one = |v: &Value| {
        let uri = v["uri"].as_str().or_else(|| v["targetUri"].as_str())?;
        let range = if v["range"].is_object() {
            &v["range"]
        } else {
            &v["targetSelectionRange"]
        };
        Some(Location {
            path: path_of(uri)?,
            pos: pos(&range["start"])?,
        })
    };
    match result {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Hover contents as Markdown text (`MarkupContent`, `MarkedString` or a list of them).
fn hover_text(contents: &Value) -> String {
    match contents {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(hover_text)
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(o) => match (o.get("language"), o.get("value")) {
            (Some(lang), Some(Value::String(v))) => {
                format!("```{}\n{v}\n```", lang.as_str().unwrap_or(""))
            }
            (_, Some(Value::String(v))) => v.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn completion(value: &Value) -> Option<Completion> {
    let label = value["label"].as_str()?.trim().to_string();
    let raw = value["textEdit"]["newText"]
        .as_str()
        .or_else(|| value["insertText"].as_str())
        .unwrap_or(&label);
    let insert = if value["insertTextFormat"].as_u64() == Some(2) {
        strip_snippet(raw)
    } else {
        raw.to_string()
    };
    Some(Completion {
        insert,
        detail: value["detail"].as_str().unwrap_or("").trim().to_string(),
        kind: match value["kind"].as_u64() {
            Some(2..=4) => "fn",
            Some(5 | 10) => "field",
            Some(6) => "var",
            Some(7 | 8 | 22) => "type",
            Some(9) => "mod",
            Some(13 | 20) => "enum",
            Some(14) => "kw",
            Some(15) => "snip",
            Some(21) => "const",
            _ => "",
        },
        label,
    })
}

/// `foo(${1:a}, $2)$0` → `foo(a, )`: placeholders keep their default text.
fn strip_snippet(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    chars.next();
                }
                if chars.peek() == Some(&':') {
                    chars.next();
                }
                let mut depth = 1;
                for c in chars.by_ref() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    if depth > 0 {
                        out.push(c);
                    }
                }
            }
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            c => out.push(c),
        }
    }
    out
}

pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(rest.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

fn language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "lua" => "lua",
        "sh" | "bash" => "shellscript",
        "toml" => "toml",
        "go" => "go",
        _ => "plaintext",
    }
}

/// The language server for `path` (fener's defaults; `[lsp]` in the config overrides).
pub fn server_for(path: &Path) -> Option<Vec<String>> {
    let words: &[&str] = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => &["rust-analyzer"],
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" => &["clangd"],
        "py" => &["pyright-langserver", "--stdio"],
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => &["typescript-language-server", "--stdio"],
        "lua" => &["lua-language-server"],
        "sh" | "bash" => &["bash-language-server", "start"],
        "go" => &["gopls"],
        "toml" => &["taplo", "lsp", "stdio"],
        _ => return None,
    };
    Some(words.iter().map(|w| (*w).to_string()).collect())
}

/// The folder a server works in: the nearest `Cargo.toml` / `go.mod` / `package.json` /
/// `pyproject.toml` / `compile_commands.json` above the file, else its git repository, else its
/// folder.
pub fn root_for(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(path);
    const MARKERS: &[&str] = &[
        "Cargo.toml",
        "go.mod",
        "package.json",
        "pyproject.toml",
        "compile_commands.json",
    ];
    // The outermost Cargo.toml is the workspace.
    let cargo = dir
        .ancestors()
        .filter(|d| d.join("Cargo.toml").exists())
        .last();
    cargo
        .or_else(|| {
            dir.ancestors()
                .find(|d| MARKERS.iter().any(|m| d.join(m).exists()))
        })
        .map_or_else(|| crate::tree::project_root(path), Path::to_path_buf)
}

/// A server position as a char index of `rope`.
pub fn to_char(rope: &Rope, pos: Pos, encoding: Encoding) -> usize {
    let lines = rope.len_lines();
    if pos.line >= lines {
        return rope.len_chars();
    }
    let start = rope.line_to_char(pos.line);
    let line = rope.line(pos.line);
    let mut units = 0;
    for (i, c) in line.chars().enumerate() {
        if units >= pos.character || c == '\n' {
            return start + i;
        }
        units += match encoding {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
        };
    }
    start + line.len_chars()
}

/// A char index of `rope` as a server position.
pub fn from_char(rope: &Rope, char: usize, encoding: Encoding) -> Pos {
    let char = char.min(rope.len_chars());
    let line = rope.char_to_line(char);
    let start = rope.line_to_char(line);
    let character = rope
        .slice(start..char)
        .chars()
        .map(|c| match encoding {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
        })
        .sum();
    Pos { line, character }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut data = Vec::new();
        let body = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        write!(data, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut reader = BufReader::new(&data[..]);
        assert_eq!(read_message(&mut reader).unwrap().unwrap()["id"], 1);
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn positions_in_utf16_and_utf8() {
        let rope = Rope::from_str("aé😀b\nx\n");
        // 😀 is two UTF-16 units and four UTF-8 bytes.
        assert_eq!(
            to_char(
                &rope,
                Pos {
                    line: 0,
                    character: 4
                },
                Encoding::Utf16
            ),
            3
        );
        assert_eq!(
            to_char(
                &rope,
                Pos {
                    line: 0,
                    character: 7
                },
                Encoding::Utf8
            ),
            3
        );
        assert_eq!(
            from_char(&rope, 3, Encoding::Utf16),
            Pos {
                line: 0,
                character: 4
            }
        );
        assert_eq!(
            from_char(&rope, 5, Encoding::Utf8),
            Pos {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            to_char(
                &rope,
                Pos {
                    line: 9,
                    character: 0
                },
                Encoding::Utf16
            ),
            rope.len_chars()
        );
    }

    #[test]
    fn answers_are_read() {
        assert_eq!(strip_snippet("foo(${1:a}, $2)$0"), "foo(a, )");
        assert_eq!(strip_snippet("vec![${1}]"), "vec![]");
        let locs = locations(
            &json!([{"uri": "file:///a%20b.rs", "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 5}}}]),
        );
        assert_eq!(locs[0].path, PathBuf::from("/a b.rs"));
        assert_eq!(
            locs[0].pos,
            Pos {
                line: 2,
                character: 4
            }
        );
        assert_eq!(
            hover_text(&json!({"kind": "markdown", "value": "**x**"})),
            "**x**"
        );
        let c = completion(&json!({"label": "push", "kind": 2, "detail": "fn(&mut self, T)", "insertTextFormat": 2, "insertText": "push(${1:value})"})).unwrap();
        assert_eq!((c.insert.as_str(), c.kind), ("push(value)", "fn"));
    }

    /// Talks to a real rust-analyzer when it is installed:
    /// `cargo test -p fener-core lsp -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn rust_analyzer_reports_an_error() {
        let dir = std::env::temp_dir().join(format!("fener-lsp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let file = dir.join("src/main.rs");
        let text = "fn main() { let x: u8 = \"no\"; }\n";
        std::fs::write(&file, text).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let client = Client::start(&["rust-analyzer".into()], &dir, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        client.open(&file, 1, text);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Event::Diagnostics { items, .. }) =
                rx.recv_timeout(std::time::Duration::from_secs(1))
                && !items.is_empty()
            {
                println!("{items:?}");
                found = true;
                break;
            }
        }
        drop(client);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(found, "no diagnostics from rust-analyzer");
    }

    /// `cargo test -p fener-core hover_from -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn hover_from_rust_analyzer() {
        let dir = std::env::temp_dir().join(format!("fener-hover-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let file = dir.join("src/main.rs");
        let text =
            "fn main() {\n    println!(\"{}\", topla(1));\n}\nfn topla(a: i32) -> i32 { a }\n";
        std::fs::write(&file, text).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let client = Client::start(&["rust-analyzer".into()], &dir, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        client.open(&file, 1, text);
        for round in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            client.hover(
                &file,
                Pos {
                    line: 3,
                    character: 5,
                },
            );
            while let Ok(e) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
                if let Event::Hover { text, .. } = e {
                    println!("round {round}: {text:?}");
                    if !text.is_empty() {
                        drop(client);
                        std::fs::remove_dir_all(&dir).unwrap();
                        return;
                    }
                }
            }
        }
        panic!("hover stayed empty");
    }
}

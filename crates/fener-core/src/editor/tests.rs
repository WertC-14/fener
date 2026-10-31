use super::*;
use crate::picker::{Item, Kind, Pick};

/// Keys from a Vim-like string: `<Esc>`, `<CR>`, `<BS>`, `<C-r>`, `<Space>`; other characters
/// are themselves.
fn keys(s: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            let name = &rest[1..end];
            let key = match name {
                "Esc" => Some(Key::Esc),
                "CR" => Some(Key::Enter),
                "BS" => Some(Key::Backspace),
                "Space" => Some(Key::Char(' ')),
                "Tab" => Some(Key::Tab),
                n if n.starts_with("C-") => n[2..].chars().next().map(Key::Ctrl),
                _ => None,
            };
            if let Some(key) = key {
                out.push(key);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push(Key::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// An editor on `text` with the cursor on the first `|` (which is removed).
fn ed(text: &str) -> Editor {
    let at = text.find('|').map(|i| text[..i].chars().count());
    let mut e = Editor::new(Document::new(&text.replace('|', "")));
    e.cursor = at.unwrap_or(0);
    e
}

/// The text with `|` where the cursor is.
fn show(e: &Editor) -> String {
    let mut s = e.doc.rope.to_string();
    let byte = s.char_indices().nth(e.cursor).map_or(s.len(), |(i, _)| i);
    s.insert(byte, '|');
    s
}

fn run(text: &str, input: &str) -> String {
    let mut e = ed(text);
    for k in keys(input) {
        e.handle_key(k);
    }
    show(&e)
}

#[test]
fn motions() {
    assert_eq!(run("|one two three", "w"), "one |two three");
    assert_eq!(run("|one two three", "2w"), "one two |three");
    assert_eq!(run("one two |three", "b"), "one |two three");
    assert_eq!(run("|one two", "e"), "on|e two");
    assert_eq!(run("|one two", "$"), "one tw|o");
    assert_eq!(run("  on|e", "0"), "|  one");
    assert_eq!(run("  on|e", "^"), "  |one");
    assert_eq!(run("|a(b)c", "fc"), "a(b)|c");
    assert_eq!(run("|a(b)c", "tc"), "a(b|)c");
    assert_eq!(run("|a,b,c", "f,;"), "a,b|,c");
    assert_eq!(run("|a(b)c", "%"), "a(b|)c");
    assert_eq!(run("|1\n2\n3\n", "G"), "1\n2\n|3\n");
    assert_eq!(run("1\n2\n|3\n", "gg"), "|1\n2\n3\n");
    assert_eq!(run("|1\n2\n3\n", "2G"), "1\n|2\n3\n");
    // Up and down keep the column across a short line.
    assert_eq!(run("abc|d\nx\nabcd\n", "jj"), "abcd\nx\nabc|d\n");
}

#[test]
fn delete_change_yank_with_counts_and_objects() {
    assert_eq!(run("|one two three", "dw"), "|two three");
    assert_eq!(run("|one two three", "d2w"), "|three");
    assert_eq!(run("|one two three", "2dw"), "|three");
    assert_eq!(run("one |two\nnext", "dw"), "one| \nnext"); // stops at the line end
    assert_eq!(run("|one two", "cwONE<Esc>"), "ON|E two"); // cw = ce
    assert_eq!(
        run("say \"hi |there\" ok", "ci\"bye<Esc>"),
        "say \"by|e\" ok"
    );
    assert_eq!(run("f(a, |b)", "da("), "|f");
    assert_eq!(run("|foo bar", "diw"), "| bar");
    assert_eq!(run("|foo bar", "daw"), "|bar");
    assert_eq!(run("|abc", "x"), "|bc");
    assert_eq!(run("a|bc", "D"), "|a"); // cursor back on the last character
    assert_eq!(run("|a(b)c", "dt)"), "|)c");
}

#[test]
fn whole_lines() {
    assert_eq!(run("|1\n2\n3\n", "dd"), "|2\n3\n");
    assert_eq!(run("1\n2\n|3\n", "dd"), "1\n|2\n");
    assert_eq!(run("|1\n2\n3\n", "2dd"), "|3\n");
    assert_eq!(run("|1\n2\n3\n", "dj"), "|3\n");
    assert_eq!(run("|1\n2\n", "yyp"), "1\n|1\n2\n");
    assert_eq!(run("1\n|2\n", "yyP"), "1\n|2\n2\n");
    assert_eq!(run("|1\n2", "ddp"), "2\n|1"); // after a last line without a line break
    assert_eq!(run("  |a\n", "ccb<Esc>"), "  |b\n"); // keeps the indentation
    assert_eq!(run("|a\nb\n", ">j"), "  |a\n  b\n");
    assert_eq!(run("  |a\n", "<<"), "|a\n");
}

#[test]
fn insert_open_join_replace() {
    assert_eq!(run("|ab", "ix<Esc>"), "|xab");
    assert_eq!(run("|ab", "ax<Esc>"), "a|xb");
    assert_eq!(run("|ab", "Ax<Esc>"), "ab|x");
    assert_eq!(run("  |a", "Ix<Esc>"), "  |xa");
    assert_eq!(run("  |a\n", "ob<Esc>"), "  a\n  |b\n"); // autoindent
    assert_eq!(run("|a\n", "Ob<Esc>"), "|b\na\n");
    assert_eq!(run("|a\n    b\n", "J"), "a| b\n");
    assert_eq!(run("|abc", "2rx"), "x|xc");
    assert_eq!(run("|ab", "~"), "A|b");
    assert_eq!(run("a|b", "i<CR><Esc>"), "a\n|b");
    assert_eq!(run("a|b", "a<BS><BS><Esc>"), "|"); // backspace in insert
    assert_eq!(run("|", "ifoo bar<C-w><Esc>"), "foo| ");
}

#[test]
fn undo_redo_and_dot() {
    assert_eq!(run("|one two", "dwu"), "|one two");
    assert_eq!(run("|one two", "dwu<C-r>"), "|two");
    assert_eq!(run("|a b c d", "dw."), "|c d");
    assert_eq!(run("|a b c d", "dw2."), "|d");
    assert_eq!(run("|x\ny\n", "Ahi<Esc>j."), "xhi\nyh|i\n"); // repeats the insert too
    assert_eq!(run("|one two", "cwX<Esc>w."), "X |X");
    // A whole insert session is one undo step.
    assert_eq!(run("|", "iabc<Esc>u"), "|");
}

#[test]
fn visual_mode() {
    assert_eq!(run("|one two", "vwd"), "|wo");
    assert_eq!(run("|one two", "veyP"), "on|eone two");
    assert_eq!(run("|1\n2\n3\n", "Vjd"), "|3\n");
    assert_eq!(run("a \"|bc\" d", "vi\"d"), "a \"|\" d");
    assert_eq!(run("|1\n2\n", "Vj>"), "  |1\n  2\n");
}

#[test]
fn search_and_command_line() {
    assert_eq!(run("|foo bar foo", "/foo<CR>"), "foo bar |foo");
    assert_eq!(run("|foo bar foo", "/foo<CR>n"), "|foo bar foo"); // wraps
    assert_eq!(run("foo bar |foo", "?bar<CR>"), "foo |bar foo");
    assert_eq!(run("|foo bar foo", "*"), "foo bar |foo");
    assert_eq!(run("|1\n2\n3\n", ":3<CR>"), "1\n2\n|3\n");
    let mut e = ed("|a");
    for k in keys("x:q<CR>") {
        e.handle_key(k);
    }
    assert!(!e.quit);
    assert!(e.message.as_deref().unwrap().contains("No write"));
    for k in keys(":q!<CR>") {
        e.handle_key(k);
    }
    assert!(e.quit);
}

#[test]
fn leader_menu_like_which_key() {
    let mut e = ed("|a");
    e.handle_key(Key::Char(' '));
    assert_eq!(
        e.leader_menu(),
        Some(vec![
            ('↵', "Run (F5)"),
            (' ', "Find Files"),
            ('1', "1…9  Go to Tab (1: files)"),
            ('-', "Split Window Below"),
            ('/', "Find Text (Grep)"),
            ('a', "Split Window Below"),
            ('b', "Build / Check (no run)"),
            ('c', "+code"),
            ('e', "Explorer (folder tree; Ctrl+B)"),
            ('f', "+file/find"),
            ('g', "+git"),
            ('o', "Find in File (Ctrl+F)"),
            ('q', "+quit/session"),
            ('s', "+search"),
            ('t', "Terminal open / close (Ctrl+/)"),
            ('u', "+ui"),
            ('v', "Split Window Right (side by side)"),
            ('w', "Save (Ctrl+S)"),
            ('|', "Split Window Right (side by side)")
        ])
    );
    e.handle_key(Key::Char('u'));
    assert_eq!(
        e.leader_menu(),
        Some(vec![
            ('C', "Colorscheme with Preview"),
            ('L', "Toggle Relative Numbers"),
            ('l', "Toggle Line Numbers"),
            ('m', "Toggle Markdown Reader"),
            ('r', "Clear Search Highlight")
        ])
    );
    e.handle_key(Key::Char('l'));
    assert!(!e.number);
    assert_eq!(e.leader_menu(), None);
    // Space q q quits (nothing changed).
    for k in keys("<Space>qq") {
        e.handle_key(k);
    }
    assert!(e.quit);
}

#[test]
fn tree_opens_files_and_hands_focus_back() {
    let dir = std::env::temp_dir().join(format!("fener-ed-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join("a.txt"), "aaa\n").unwrap();
    std::fs::write(dir.join("b.txt"), "bbb\n").unwrap();
    let mut e = Editor::new(Document::open(&dir.join("a.txt")).unwrap());
    for k in keys("<Space>e") {
        e.handle_key(k);
    }
    assert!(e.tree_focus);
    assert_eq!(
        e.tree.as_ref().unwrap().selected_row().unwrap().name,
        "a.txt"
    );
    for k in keys("j<CR>") {
        e.handle_key(k);
    }
    assert_eq!(e.doc.rope.to_string(), "bbb\n");
    assert!(!e.tree_focus && e.tree.is_some());
    // Ctrl+H back to the tree, q closes it.
    e.handle_key(Key::Ctrl('h'));
    e.handle_key(Key::Char('q'));
    assert!(e.tree.is_none() && !e.tree_focus);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn markdown_opens_in_the_reader_and_i_edits_where_it_was() {
    let dir = std::env::temp_dir().join(format!("fener-reader-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.md");
    std::fs::write(&file, "# A\n\none\ntwo\nthree\n").unwrap();
    let mut e = Editor::new(Document::open(&file).unwrap());
    assert!(e.reader);
    for k in keys("jjj") {
        e.handle_key(k);
    }
    assert_eq!(e.reader_top, 3);
    // x does nothing in the reader (no edit by accident).
    e.handle_key(Key::Char('x'));
    assert!(!e.doc.is_modified());
    e.handle_key(Key::Char('i'));
    assert!(!e.reader);
    assert_eq!((e.line(), e.mode), (3, Mode::Normal));
    for k in keys("<Space>um") {
        e.handle_key(k);
    }
    assert!(e.reader);
    // The Space menu of a Markdown file: no run / build, but reader and headings.
    e.handle_key(Key::Char(' '));
    let menu = e.leader_menu().unwrap();
    assert!(menu.contains(&('r', "Reader / Source (Ctrl+E)")));
    assert!(menu.contains(&('h', "Go to Heading")));
    assert!(!menu.iter().any(|(k, _)| *k == '↵' || *k == 'b'));
    // Space h: the headings; picking one scrolls the reader there.
    e.handle_key(Key::Char('h'));
    assert_eq!(e.picker.as_ref().unwrap().items[0].text, "A");
    e.handle_key(Key::Enter);
    assert_eq!(e.reader_top, 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ctrl_f_finds_in_the_file_and_n_goes_on() {
    let mut e = ed("|alpha\nbeta\ngamma beta\n");
    e.handle_key(Key::Ctrl('f'));
    for k in keys("beta") {
        e.handle_key(k);
    }
    let p = e.picker.as_ref().unwrap();
    assert_eq!(p.matches.len(), 2);
    assert_eq!(p.matches[1].positions, vec![6, 7, 8, 9]);
    e.handle_key(Key::Down);
    e.handle_key(Key::Enter);
    assert_eq!(e.line(), 2);
    assert_eq!(e.last_search.as_deref(), Some("beta"));
}

#[test]
fn f5_saves_and_asks_to_run_and_ctrl_slash_asks_for_the_terminal() {
    let dir = std::env::temp_dir().join(format!("fener-f5-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hi.py");
    std::fs::write(&file, "print(1)\n").unwrap();
    let mut e = Editor::new(Document::open(&file).unwrap());
    e.handle_key(Key::Char('x'));
    e.handle_key(Key::F(5));
    assert!(!e.doc.is_modified(), "saved before running");
    let run = crate::run::Run {
        dir: dir.clone(),
        command: "python3 hi.py".into(),
    };
    assert!(e.requests.contains(&Request::Run(run)));
    e.handle_key(Key::Ctrl('/'));
    assert_eq!(e.requests.last(), Some(&Request::ToggleTerminal));
    // Space as a super key: Enter runs, b builds, t terminal, 3 tab three.
    e.requests.clear();
    for k in keys("<Space><CR><Space>b<Space>t<Space>3") {
        e.handle_key(k);
    }
    let build = crate::run::Run {
        dir: dir.clone(),
        command: "python3 -m py_compile hi.py".into(),
    };
    assert_eq!(e.requests.len(), 4);
    assert_eq!(e.requests[1], Request::Run(build));
    assert_eq!(
        e.requests[2..],
        [Request::ToggleTerminal, Request::SwitchTab(2)]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn dashboard_menu_and_new_file() {
    let mut e = ed("|");
    e.dashboard = Some(0);
    e.handle_key(Key::Char('j'));
    assert_eq!(e.dashboard, Some(1));
    // `?` opens the keymaps picker over the start screen; Esc goes back to it.
    e.handle_key(Key::Char('?'));
    assert_eq!(e.picker.as_ref().unwrap().kind, Kind::Keymaps);
    e.handle_key(Key::Esc);
    assert!(e.picker.is_none() && e.dashboard.is_some());
    e.handle_key(Key::Char('n'));
    assert_eq!((e.dashboard, e.mode), (None, Mode::Insert));
}

#[test]
fn keymaps_picker_runs_commands() {
    let mut e = ed("|a");
    for k in keys("<Space>sk") {
        e.handle_key(k);
    }
    for k in keys("line num") {
        e.handle_key(k);
    }
    let p = e.picker.as_ref().unwrap();
    assert_eq!(p.current().unwrap().text, "Toggle Line Numbers");
    e.handle_key(Key::Enter);
    assert!(e.picker.is_none() && !e.number);
    // Keys can be searched too: a cheat-sheet row is shown, not run.
    for k in keys("<Space>skdd<CR>") {
        e.handle_key(k);
    }
    assert_eq!(e.message.as_deref(), Some("dd  Delete a line"));
}

#[test]
fn themes_preview_and_escape_restores() {
    let mut e = ed("|a");
    e.themes = vec!["night", "storm", "moon"];
    e.run_command(Command::Themes);
    e.handle_key(Key::Down);
    assert_eq!(e.theme, 1);
    e.handle_key(Key::Esc);
    assert_eq!(e.theme, 0);
    e.run_command(Command::Themes);
    for k in keys("moon<CR>") {
        e.handle_key(k);
    }
    assert_eq!((e.theme, e.state.theme.as_deref()), (2, Some("moon")));
    assert!(e.requests.contains(&Request::SaveState));
}

#[test]
fn grep_results_for_an_old_query_are_dropped() {
    let mut e = ed("|a");
    e.run_command(Command::FindText);
    for k in keys("fo") {
        e.handle_key(k);
    }
    assert_eq!(
        e.requests,
        [Request::Grep("f".into()), Request::Grep("fo".into())]
    );
    let hit = |t: &str| Item {
        text: t.into(),
        detail: String::new(),
        pick: Pick::Info,
    };
    e.receive(Kind::Grep, "f", vec![hit("old")]);
    assert!(e.picker.as_ref().unwrap().items.is_empty());
    e.receive(Kind::Grep, "fo", vec![hit("food")]);
    let p = e.picker.as_ref().unwrap();
    assert_eq!(
        (p.items.len(), p.matches[0].positions.clone()),
        (1, vec![0, 1])
    );
}

#[test]
fn open_refuses_to_drop_changes() {
    let dir = std::env::temp_dir().join(format!("fener-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("b.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let mut e = ed("|a");
    e.handle_key(Key::Char('x'));
    e.open(&file, Some(2), false);
    assert!(e.message.as_deref().unwrap().starts_with("No write"));
    e.open(&file, Some(2), true);
    assert_eq!(
        (e.line(), e.doc.rope.to_string().as_str()),
        (2, "one\ntwo\nthree\n")
    );
    assert_eq!(e.state.recent[0], file);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ctrl_z_and_ctrl_y_in_every_mode() {
    let mut e = ed("|abc");
    // Normal mode.
    e.handle_key(Key::Char('x'));
    e.handle_key(Key::Ctrl('z'));
    assert_eq!(e.doc.rope.to_string(), "abc");
    e.handle_key(Key::Ctrl('y'));
    assert_eq!(e.doc.rope.to_string(), "bc");
    // Insert mode: what was typed goes, typing goes on.
    for k in keys("ixy") {
        e.handle_key(k);
    }
    e.handle_key(Key::Ctrl('z'));
    assert_eq!(
        (e.doc.rope.to_string().as_str(), e.mode),
        ("bc", Mode::Insert)
    );
    e.handle_key(Key::Char('q'));
    assert_eq!(e.doc.rope.to_string(), "qbc");
}

#[test]
fn ctrl_e_switches_markdown_between_reader_and_editing() {
    let dir = std::env::temp_dir().join(format!("fener-ctrl-e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("n.md");
    std::fs::write(&file, "# T\n").unwrap();
    let mut e = Editor::new(Document::open(&file).unwrap());
    assert!(e.reader);
    e.handle_key(Key::Ctrl('e'));
    assert!(!e.reader);
    e.handle_key(Key::Char('i'));
    e.handle_key(Key::Ctrl('e'));
    assert!(e.reader && e.mode == Mode::Normal);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An editor on a scratch file with this name and text (the extension picks the language).
fn ed_file(name: &str, text: &str) -> (Editor, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("fener-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, text).unwrap();
    (Editor::new(Document::open(&file).unwrap()), dir)
}

#[test]
fn gcc_and_gc_comment_in_and_out() {
    let (mut e, dir) = ed_file("a.rs", "fn a() {\n    x();\n    y();\n}\n");
    for k in keys("jgcc") {
        e.handle_key(k);
    }
    assert_eq!(
        e.doc.rope.to_string(),
        "fn a() {\n    // x();\n    y();\n}\n"
    );
    // gcj: this line and the next; one is commented already, so both get one.
    for k in keys("gcj") {
        e.handle_key(k);
    }
    assert_eq!(
        e.doc.rope.to_string(),
        "fn a() {\n    // // x();\n    // y();\n}\n"
    );
    e.handle_key(Key::Ctrl('z'));
    // Visual gc on both lines: not all commented → each gets a mark; again → they go.
    for k in keys("Vjgc") {
        e.handle_key(k);
    }
    assert_eq!(
        e.doc.rope.to_string(),
        "fn a() {\n    // // x();\n    // y();\n}\n"
    );
    for k in keys("Vjgc") {
        e.handle_key(k);
    }
    assert_eq!(
        e.doc.rope.to_string(),
        "fn a() {\n    // x();\n    y();\n}\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn alt_j_and_alt_k_move_lines() {
    let mut e = ed("|one\ntwo\nthree\n");
    e.handle_key(Key::Alt('j'));
    assert_eq!(
        (e.doc.rope.to_string().as_str(), e.line()),
        ("two\none\nthree\n", 1)
    );
    e.handle_key(Key::Alt('k'));
    assert_eq!(e.doc.rope.to_string(), "one\ntwo\nthree\n");
    // Visual: two lines move together and stay selected.
    for k in keys("Vj") {
        e.handle_key(k);
    }
    e.handle_key(Key::Alt('j'));
    assert_eq!(e.doc.rope.to_string(), "three\none\ntwo\n");
    assert_eq!(e.mode, Mode::VisualLine);
    e.handle_key(Key::Ctrl('z'));
    assert_eq!(e.doc.rope.to_string(), "one\ntwo\nthree\n");
}

#[test]
fn brackets_and_quotes_close_themselves() {
    let (mut e, dir) = ed_file("p.py", "\n");
    for k in keys("ifoo(\"a") {
        e.handle_key(k);
    }
    assert_eq!(e.doc.rope.to_string(), "foo(\"a\")\n");
    // Typing the closers steps over them.
    for k in keys("\")") {
        e.handle_key(k);
    }
    assert_eq!(e.doc.rope.to_string(), "foo(\"a\")\n");
    // Backspace in an empty pair takes both; Enter inside {} opens it.
    for k in keys("[<BS>{<CR>") {
        e.handle_key(k);
    }
    assert_eq!(e.doc.rope.to_string(), "foo(\"a\"){\n  \n}\n");
    // Inside a word, no pair: it's.
    for k in keys("<Esc>oit's") {
        e.handle_key(k);
    }
    assert!(e.doc.rope.to_string().contains("it's\n"));
    std::fs::remove_dir_all(&dir).unwrap();
    // Rust: no pair for ' (lifetimes).
    let (mut r, dir) = ed_file("l.rs", "\n");
    for k in keys("i&'a") {
        r.handle_key(k);
    }
    assert_eq!(r.doc.rope.to_string(), "&'a\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn shift_h_l_and_space_g_g_and_space_q_s_ask_the_app() {
    let mut e = ed("|a");
    for k in keys("HL<Space>gg<Space>qs") {
        e.handle_key(k);
    }
    assert_eq!(
        e.requests,
        [
            Request::CycleTab(-1),
            Request::CycleTab(1),
            Request::Lazygit,
            Request::RestoreSession
        ]
    );
}

#[test]
fn formatted_text_is_one_undo_step_and_stale_answers_are_dropped() {
    let mut e = ed("a\n|b\n");
    let v = e.doc.version();
    e.formatted(v, Ok("a\n  b\n".into()));
    assert_eq!((e.doc.rope.to_string().as_str(), e.line()), ("a\n  b\n", 1));
    e.handle_key(Key::Char('u'));
    assert_eq!(e.doc.rope.to_string(), "a\nb\n");
    // The text changed after the request: the answer is ignored.
    let old = e.doc.version();
    e.handle_key(Key::Char('x'));
    e.formatted(old, Ok("zzz\n".into()));
    assert_ne!(e.doc.rope.to_string(), "zzz\n");
}

#[test]
fn lsp_keys_ask_the_server_with_its_positions() {
    let mut e = ed("aé|b\n");
    e.handle_key(Key::Char('K'));
    assert!(
        e.message
            .as_deref()
            .unwrap()
            .starts_with("No language server")
    );
    e.lsp.active = true;
    e.lsp.encoding = lsp::Encoding::Utf8;
    for k in keys("Kgdgr") {
        e.handle_key(k);
    }
    let at = lsp::Pos {
        line: 0,
        character: 3,
    }; // é is two bytes
    assert_eq!(
        e.requests,
        [
            Request::Lsp(LspAsk::Hover(at)),
            Request::Lsp(LspAsk::Definition(at)),
            Request::Lsp(LspAsk::References(at))
        ]
    );
    // The hover box shows until the next key.
    e.show_hover("**u8**".into());
    assert!(e.lsp.hover.is_some());
    e.handle_key(Key::Esc);
    assert!(e.lsp.hover.is_none());
}

#[test]
fn completion_menu_filters_and_accepts() {
    let mut e = ed("|\n");
    e.lsp.active = true;
    for k in keys("ipu") {
        e.handle_key(k);
    }
    assert!(matches!(
        e.requests.last(),
        Some(Request::Lsp(LspAsk::Completion(..)))
    ));
    let item = |label: &str, insert: &str| lsp::Completion {
        label: label.into(),
        insert: insert.into(),
        detail: String::new(),
        kind: "fn",
    };
    e.show_completion(vec![item("pop", "pop()"), item("push", "push(value)")]);
    let menu = e.lsp.completion.as_ref().unwrap();
    assert_eq!(menu.shown.len(), 1, "only push fits \"pu\"");
    e.handle_key(Key::Tab);
    assert_eq!(e.doc.rope.to_string(), "push(value)\n");
    assert!(e.lsp.completion.is_none() && e.mode == Mode::Insert);
}

#[test]
fn diagnostics_are_walked_with_brackets() {
    let mut e = ed("|one\ntwo\nthree\n");
    let diag = |line, message: &str| lsp::Diagnostic {
        start: lsp::Pos { line, character: 1 },
        end: lsp::Pos { line, character: 2 },
        severity: lsp::Severity::Error,
        message: message.into(),
    };
    e.lsp.diagnostics = vec![diag(2, "third"), diag(1, "second")];
    for k in keys("]d") {
        e.handle_key(k);
    }
    assert_eq!((e.line(), e.message.as_deref()), (1, Some("second")));
    for k in keys("]d]d") {
        e.handle_key(k);
    }
    assert_eq!(e.line(), 1, "wraps around");
    for k in keys("[d") {
        e.handle_key(k);
    }
    assert_eq!(e.line(), 2);
}

#[test]
fn references_open_a_picker() {
    let mut e = ed("|a\n");
    e.show_references(vec![(
        PathBuf::from("/p/src/a.rs"),
        4,
        "  let x = 1;".into(),
    )]);
    let p = e.picker.as_ref().unwrap();
    assert_eq!(
        (p.kind, p.items[0].text.as_str()),
        (Kind::Locations, "let x = 1;")
    );
    assert!(p.items[0].detail.ends_with("a.rs:5"));
}

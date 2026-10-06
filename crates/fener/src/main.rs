//! fener: a modal terminal editor in the spirit of LazyVim.
//!
//! The terminal setup and the event loop follow liman's (fm-research ADR 0010): an input thread
//! sends events over a channel, the main loop draws only when something changed, inside a
//! synchronized update so the terminal never shows half a frame. Slow work (listing and
//! searching files for the pickers) runs on worker threads and comes back on the same channel.

use std::io::{self, stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;

use fener_core::picker::{self, Item, Kind};
use fener_core::{Document, Editor, Key, Mode, Request, State, files};
use fener_widgets::{THEMES, View};
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

const HELP: &str = "fener: a modal terminal editor in the spirit of LazyVim

Usage: fener             the start screen (find, recent and new files, themes, keys)
       fener FILE        edit FILE (a new file is created on :w)
       fener --version

Space opens the command menu; Space s k (or ? on the start screen) lists every key.
FENER_ICONS=plain turns off the Nerd Font icons.";

/// Most files listed for Find Files, and most lines Find Text shows.
const MAX_FILES: usize = 100_000;
const MAX_HITS: usize = 2_000;

enum Msg {
    Term(Event),
    /// A picker's list from a worker: the picker kind, the query it is for, the items.
    Items(Kind, String, Vec<Item>),
}

fn main() -> io::Result<()> {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version" | "-V") => {
            println!("fener {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!("{HELP}");
            return Ok(());
        }
        _ => {}
    }
    let doc = match &arg {
        Some(path) => Document::open(&PathBuf::from(path))?,
        None => Document::new(""),
    };
    let mut editor = Editor::new(doc);
    editor.root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    editor.state = State::load();
    editor.themes = THEMES.iter().map(|t| t.name).collect();
    if let Some(name) = &editor.state.theme {
        editor.theme = THEMES.iter().position(|t| t.name == name).unwrap_or(0);
    }
    match &arg {
        Some(path) => editor.remember(Path::new(path)),
        None => editor.dashboard = Some(0),
    }
    let mut view = View::default();

    let mut terminal = ratatui::try_init()?;
    let (tx, rx) = mpsc::channel();
    let input = tx.clone();
    thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input.send(Msg::Term(ev)).is_err() {
                break;
            }
        }
    });
    // Bumped by each new worker job; a job that sees it change stops (its result is stale).
    let generation = Arc::new(AtomicU64::new(0));

    let mut dirty = true;
    let mut shape = None;
    let result = loop {
        if dirty {
            execute!(stdout(), BeginSynchronizedUpdate)?;
            // A bar cursor in Insert mode and on the command line, a block elsewhere. Sent
            // before the frame, so a terminal that prints the sequence has it drawn over.
            let bar = matches!(editor.mode, Mode::Insert | Mode::Command | Mode::Search);
            if shape != Some(bar) {
                let style = if bar {
                    SetCursorStyle::SteadyBar
                } else {
                    SetCursorStyle::SteadyBlock
                };
                execute!(stdout(), style)?;
                shape = Some(bar);
            }
            let drawn = terminal.draw(|frame| {
                let area = frame.area();
                let theme = &THEMES[editor.theme.min(THEMES.len() - 1)];
                if let Some((x, y)) =
                    fener_widgets::render(frame.buffer_mut(), area, &mut editor, &mut view, theme)
                {
                    frame.set_cursor_position((x, y));
                }
            });
            execute!(stdout(), EndSynchronizedUpdate)?;
            drawn?;
            dirty = false;
        }
        let Ok(first) = rx.recv() else {
            break Ok(());
        };
        // Drain what queued up meanwhile: a paste or key repeat costs one frame.
        for msg in std::iter::once(first).chain(rx.try_iter()) {
            match msg {
                Msg::Term(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if let Some(key) = convert(key) {
                        editor.handle_key(key);
                        dirty = true;
                    }
                }
                Msg::Term(Event::Paste(text)) => {
                    for c in text.chars() {
                        editor.handle_key(if c == '\n' { Key::Enter } else { Key::Char(c) });
                    }
                    dirty = true;
                }
                Msg::Term(Event::Resize(..)) => dirty = true,
                Msg::Term(_) => {}
                Msg::Items(kind, query, items) => {
                    editor.receive(kind, &query, items);
                    dirty = true;
                }
            }
            for request in std::mem::take(&mut editor.requests) {
                start(request, &editor, &tx, &generation);
            }
        }
        if editor.quit {
            break Ok(());
        }
    };
    let _ = execute!(stdout(), SetCursorStyle::DefaultUserShape);
    ratatui::restore();
    result
}

/// Does what the editor asked for: state is written here, lists are made on a worker.
fn start(request: Request, editor: &Editor, tx: &Sender<Msg>, generation: &Arc<AtomicU64>) {
    if request == Request::SaveState {
        // Remembering is a convenience: a read-only home folder must not stop editing.
        let _ = editor.state.save();
        return;
    }
    let root = editor.root.clone();
    let tx = tx.clone();
    let mine = generation.fetch_add(1, Ordering::Relaxed) + 1;
    let generation = Arc::clone(generation);
    let cancel = move || generation.load(Ordering::Relaxed) != mine;
    match request {
        Request::SaveState => {}
        Request::ListFiles => {
            thread::spawn(move || {
                let paths = files::list(&root, MAX_FILES, &cancel);
                let items = picker::file_items(&root, paths);
                let _ = tx.send(Msg::Items(Kind::Files, String::new(), items));
            });
        }
        Request::Grep(query) => {
            thread::spawn(move || {
                let hits = files::grep(&root, &query, MAX_HITS, &cancel);
                if !cancel() {
                    let items = picker::grep_items(&root, hits);
                    let _ = tx.send(Msg::Items(Kind::Grep, query, items));
                }
            });
        }
    }
}

/// A terminal key event as fener's key (`None` for keys fener does not use).
fn convert(key: KeyEvent) -> Option<Key> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Esc => Key::Esc,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Tab => Key::Tab,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => return None,
    })
}

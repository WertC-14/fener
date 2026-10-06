//! fener: a modal terminal editor in the spirit of LazyVim.
//!
//! The terminal setup and the event loop follow liman's (fm-research ADR 0010): an input thread
//! sends events over a channel, the main loop draws only when something changed, inside a
//! synchronized update so the terminal never shows half a frame.

mod ui;

use std::io::{self, stdout};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use fener_core::{Document, Editor, Key, Mode};
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

const HELP: &str = "fener: a modal terminal editor in the spirit of LazyVim

Usage: fener [FILE]      edit FILE (a new file is created on :w)
       fener --version";

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
    let mut view = ui::View::default();

    let mut terminal = ratatui::try_init()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });

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
                if let Some((x, y)) = ui::render(frame, &mut editor, &mut view) {
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
        for ev in std::iter::once(first).chain(rx.try_iter()) {
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(key) = convert(key) {
                        editor.handle_key(key);
                        dirty = true;
                    }
                }
                Event::Paste(text) => {
                    for c in text.chars() {
                        editor.handle_key(if c == '\n' { Key::Enter } else { Key::Char(c) });
                    }
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
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

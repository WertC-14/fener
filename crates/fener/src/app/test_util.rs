//! Test helpers for the app on a real temp folder, with the workers running: shared by the
//! tests of the file operations and of the code tabs.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use liman_core::Places;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
};

use super::{App, Listing};
use crate::event::AppEvent;

pub(super) fn key(code: KeyCode, modifiers: KeyModifiers) -> AppEvent {
    AppEvent::Input(Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }))
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    app.handle(key(code, KeyModifiers::NONE));
}

pub(super) fn ctrl(app: &mut App, c: char) {
    app.handle(key(KeyCode::Char(c), KeyModifiers::CONTROL));
}

/// Types `text` key by key; `\x1b` is Esc.
pub(super) fn keys(app: &mut App, text: &str) {
    for c in text.chars() {
        let code = if c == '\x1b' {
            KeyCode::Esc
        } else {
            KeyCode::Char(c)
        };
        press(app, code);
    }
}

/// Feeds worker events to the app until `done` holds (listing loaded, job finished...).
pub(super) fn pump(app: &mut App, rx: &Receiver<AppEvent>, done: impl Fn(&App) -> bool) {
    while !done(app) {
        let ev = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("worker event");
        app.handle(ev);
    }
}

pub(super) fn loaded(app: &App) -> bool {
    matches!(app.tab.listing, Listing::Ready(_)) && app.job.is_none() && app.loading_path.is_none()
}

/// Until the first code tab's picker has its list (Find Files reads the folder on a worker).
pub(super) fn wait_picker(app: &mut App, rx: &Receiver<AppEvent>) {
    pump(app, rx, |a| {
        a.code.tabs[0]
            .editor
            .picker
            .as_ref()
            .is_some_and(|p| !p.loading)
    });
}

/// Temp home with a.txt, b.txt and dest/; the app shows it with a private trash.
pub(super) fn setup(tag: &str) -> (PathBuf, App, Receiver<AppEvent>) {
    let dir = std::env::temp_dir().join(format!("liman-app-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("dest")).unwrap();
    fs::write(dir.join("a.txt"), "a").unwrap();
    fs::write(dir.join("b.txt"), "b").unwrap();
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(dir.clone(), Places::from_user_dirs("", &dir), tx);
    app.trash_dir = dir.join(".trash");
    pump(&mut app, &rx, loaded);
    (dir, app, rx)
}

pub(super) fn select(app: &mut App, name: &str) {
    let row = app.visible_entries().position(|e| e.name == name).unwrap();
    app.tab.table.select(Some(row));
}

//! Nerd Font icons for files and commands, as LazyVim; none with `FENER_ICONS=plain` or when
//! the embedding app turns them off ([`set_nerd`]).

use std::sync::atomic::{AtomicU8, Ordering};

use fener_core::Command;
use ratatui::style::Color;

use crate::theme::t;

/// 0: not decided yet (read `FENER_ICONS`), 1: Nerd Font icons, 2: none.
static NERD: AtomicU8 = AtomicU8::new(0);

/// Turns the Nerd Font icons on or off (an embedding app follows its own setting).
pub fn set_nerd(on: bool) {
    NERD.store(if on { 1 } else { 2 }, Ordering::Relaxed);
}

/// Nerd Font icons, unless `FENER_ICONS=plain` or [`set_nerd`] (LazyVim assumes a Nerd Font too).
pub fn nerd() -> bool {
    match NERD.load(Ordering::Relaxed) {
        0 => {
            let on = std::env::var("FENER_ICONS").map_or(true, |v| v != "plain");
            set_nerd(on);
            on
        }
        n => n == 1,
    }
}

pub fn command_icon(command: Command) -> &'static str {
    if !nerd() {
        return "";
    }
    match command {
        Command::FindFiles => "\u{f002} ",
        Command::NewFile => "\u{f15b} ",
        Command::FindText => "\u{f0f6} ",
        Command::RecentFiles => "\u{f1da} ",
        Command::Themes => "\u{f1fc} ",
        Command::Keymaps => "\u{f11c} ",
        Command::Quit => "\u{f08b} ",
        Command::Explorer => "\u{f07c} ",
        _ => "\u{f101} ",
    }
}

/// A file's icon and its color, by extension.
pub fn file_icon(path: &str) -> (&'static str, Color) {
    if !nerd() {
        return ("", t().fg_dim);
    }
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "rs" => ("\u{e7a8} ", t().orange),
        "md" => ("\u{e73e} ", t().blue),
        "toml" | "yaml" | "yml" | "ini" | "conf" => ("\u{e615} ", t().fg_dim),
        "json" => ("\u{e60b} ", t().yellow),
        "py" => ("\u{e606} ", t().yellow),
        "js" | "ts" => ("\u{e74e} ", t().yellow),
        "sh" | "fish" | "bash" | "zsh" => ("\u{f489} ", t().green),
        "lock" => ("\u{f023} ", t().fg_dim),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => ("\u{f1c5} ", t().magenta),
        _ => ("\u{f15b} ", t().fg_dim),
    }
}

//! UI-independent core of fener: the document and its undo history, Vim motions and text
//! objects, and the editor state machine (modes, the command language, registers).
//!
//! No terminal library here; the `fener` crate draws and feeds keys.

pub mod command;
pub mod document;
pub mod editor;
pub mod files;
pub mod fuzzy;
pub mod picker;
pub mod state;
pub mod text;
pub mod tree;

pub use command::Command;
pub use document::{Document, Edit};
pub use editor::{Editor, Key, Mode, Request};
pub use state::State;

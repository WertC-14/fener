//! A text file being edited: the text (a rope), where it lives, and its history of changes.
//!
//! Every edit is a [`Edit`] — "replace the characters `from..to` with `text`" — and is stored with
//! its inverse, so undo and redo replay edits instead of copying the whole text (fm-research
//! `notes/helix.md`: a change record, not snapshots). Edits made by one command (`dw`, or a whole
//! insert session after `cw`) are grouped into one undo step.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ropey::Rope;

/// Replace the characters `from..to` (char indices) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

impl Edit {
    /// The edit that undoes `self` once it has been applied to `before`.
    fn inverse(&self, before: &Rope) -> Edit {
        Edit {
            from: self.from,
            to: self.from + self.text.chars().count(),
            text: before.slice(self.from..self.to).to_string(),
        }
    }

    fn apply(&self, rope: &mut Rope) {
        if self.to > self.from {
            rope.remove(self.from..self.to);
        }
        if !self.text.is_empty() {
            rope.insert(self.from, &self.text);
        }
    }

    /// Where position `pos` ends up after this edit (positions after it shift).
    pub fn map(&self, pos: usize) -> usize {
        let inserted = self.text.chars().count();
        if pos <= self.from {
            pos
        } else if pos >= self.to {
            pos - (self.to - self.from) + inserted
        } else {
            self.from + inserted.min(pos - self.from)
        }
    }
}

/// One undo step: the edits in the order they were made, and where the cursor was around them.
#[derive(Debug, Clone, Default)]
struct Step {
    edits: Vec<Edit>,
    inverses: Vec<Edit>,
    cursor_before: usize,
    cursor_after: usize,
}

/// Undo goes back at most this many steps.
const MAX_UNDO: usize = 1000;

pub struct Document {
    pub rope: Rope,
    path: Option<PathBuf>,
    done: Vec<Step>,
    undone: Vec<Step>,
    /// The step being built while a command (or an insert session) runs.
    open: Option<Step>,
    /// Bumped by every change of the text, undo and redo included.
    version: u64,
    saved_version: u64,
    /// Every change applied to the text since the last [`Document::take_changes`], undo and redo
    /// included, while another window shows this document (it moves its cursor along).
    log: Option<Vec<Edit>>,
}

impl Document {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            path: None,
            done: Vec::new(),
            undone: Vec::new(),
            open: None,
            version: 0,
            saved_version: 0,
            log: None,
        }
    }

    /// Opens `path`; a file that does not exist yet is an empty document that saves there.
    pub fn open(path: &Path) -> io::Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let mut doc = Self::new(&text);
        doc.path = Some(path.to_path_buf());
        Ok(doc)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    /// Writes the text to its file.
    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no file name"));
        };
        let mut file = io::BufWriter::new(fs::File::create(path)?);
        self.rope.write_to(&mut file)?;
        io::Write::flush(&mut file)?;
        self.saved_version = self.version;
        Ok(())
    }

    pub fn is_modified(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Starts or stops keeping the changes for [`Document::take_changes`].
    pub fn track_changes(&mut self, on: bool) {
        self.log = on.then(Vec::new);
    }

    /// The changes since the last call, in the order they were applied.
    pub fn take_changes(&mut self) -> Vec<Edit> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    fn logged(&mut self, edit: &Edit) {
        if let Some(log) = &mut self.log {
            log.push(edit.clone());
        }
    }

    /// Starts an undo step: every edit until [`Document::end_step`] is undone together.
    pub fn begin_step(&mut self, cursor: usize) {
        if self.open.is_none() {
            self.open = Some(Step {
                cursor_before: cursor,
                ..Step::default()
            });
        }
    }

    /// Closes the open undo step (an empty one is dropped).
    pub fn end_step(&mut self, cursor: usize) {
        if let Some(mut step) = self.open.take()
            && !step.edits.is_empty()
        {
            step.cursor_after = cursor;
            self.done.push(step);
            if self.done.len() > MAX_UNDO {
                self.done.remove(0);
            }
            self.undone.clear();
        }
    }

    /// Replaces `from..to` with `text`. Outside an open step the edit is a step of its own.
    pub fn edit(&mut self, from: usize, to: usize, text: &str) {
        let len = self.rope.len_chars();
        let (from, to) = (from.min(len), to.min(len).max(from.min(len)));
        if from == to && text.is_empty() {
            return;
        }
        let edit = Edit {
            from,
            to,
            text: text.to_string(),
        };
        let inverse = edit.inverse(&self.rope);
        edit.apply(&mut self.rope);
        self.logged(&edit);
        self.version += 1;
        let single = self.open.is_none();
        if single {
            self.begin_step(from);
        }
        let step = self.open.as_mut().expect("a step is open");
        step.edits.push(edit);
        step.inverses.push(inverse);
        if single {
            self.end_step(from);
        }
    }

    /// Undoes the last step; returns where the cursor goes.
    pub fn undo(&mut self) -> Option<usize> {
        self.end_step(0);
        let step = self.done.pop()?;
        for inverse in step.inverses.iter().rev() {
            inverse.apply(&mut self.rope);
            self.logged(inverse);
        }
        self.version += 1;
        let cursor = step.cursor_before;
        self.undone.push(step);
        Some(cursor)
    }

    /// Redoes the last undone step; returns where the cursor goes.
    pub fn redo(&mut self) -> Option<usize> {
        let step = self.undone.pop()?;
        for edit in &step.edits {
            edit.apply(&mut self.rope);
            self.logged(edit);
        }
        self.version += 1;
        let cursor = step.cursor_after;
        self.done.push(step);
        Some(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(doc: &Document) -> String {
        doc.rope.to_string()
    }

    #[test]
    fn edits_undo_and_redo_in_steps() {
        let mut doc = Document::new("hello world\n");
        doc.edit(0, 5, "goodbye"); // a step of its own
        assert_eq!(text(&doc), "goodbye world\n");
        doc.begin_step(8);
        doc.edit(8, 13, "there");
        doc.edit(13, 13, "!");
        doc.end_step(14);
        assert_eq!(text(&doc), "goodbye there!\n");
        assert_eq!(doc.undo(), Some(8)); // both edits of the step at once
        assert_eq!(text(&doc), "goodbye world\n");
        assert_eq!(doc.undo(), Some(0));
        assert_eq!(text(&doc), "hello world\n");
        assert_eq!(doc.undo(), None);
        assert_eq!(doc.redo(), Some(0));
        assert_eq!(doc.redo(), Some(14));
        assert_eq!(text(&doc), "goodbye there!\n");
        // A new edit forgets what was undone.
        doc.undo();
        doc.edit(0, 0, ">");
        assert_eq!(doc.redo(), None);
    }

    #[test]
    fn positions_move_with_edits() {
        let e = Edit {
            from: 2,
            to: 5,
            text: "xy".into(),
        };
        assert_eq!(e.map(1), 1);
        assert_eq!(e.map(6), 5); // 3 removed, 2 inserted
        assert_eq!(e.map(3), 3); // inside the replaced range: clamps into the new text
    }

    #[test]
    fn changes_are_kept_while_tracked() {
        let mut doc = Document::new("abc");
        doc.edit(0, 0, "x");
        assert!(doc.take_changes().is_empty(), "not tracked");
        doc.track_changes(true);
        doc.edit(1, 2, "");
        doc.undo();
        let changes = doc.take_changes();
        assert_eq!(changes.len(), 2);
        // A cursor on "c" (index 3 in "xabc") follows the delete and its undo.
        let pos = changes.iter().fold(3, |pos, e| e.map(pos));
        assert_eq!(pos, 3);
        assert!(doc.take_changes().is_empty());
    }

    #[test]
    fn modified_until_saved() {
        let dir = std::env::temp_dir().join(format!("fener-doc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("new.txt");
        let mut doc = Document::open(&path).unwrap(); // does not exist yet
        assert!(!doc.is_modified());
        doc.edit(0, 0, "çay\n");
        assert!(doc.is_modified());
        doc.save().unwrap();
        assert!(!doc.is_modified());
        assert_eq!(fs::read_to_string(&path).unwrap(), "çay\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}

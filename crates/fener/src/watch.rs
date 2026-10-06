//! Watching the open folder (inotify through `notify`). Changes from anywhere — the embedded shell,
//! other programs, our own jobs — refresh the list. Bursts are merged: one refresh after the folder
//! has been quiet for `SETTLE`.
//!
//! Every event reports the watched folder itself, not a folder guessed from the event's path
//! (cardea does the same, `research/repos/cardea/src/fs/watcher.rs:41`). The app also checks the
//! folder's modification time every few seconds and when the terminal window gets the focus
//! back, so a change inotify did not report (FUSE and network folders, a full watch table)
//! still shows up.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::event::AppEvent;

/// A copy of many files sends hundreds of events; wait until they stop.
const SETTLE: Duration = Duration::from_millis(150);

pub struct FolderWatch {
    watcher: Option<RecommendedWatcher>,
    current: Option<PathBuf>,
    /// The folder being watched, for the watcher's callback.
    shared: Arc<Mutex<Option<PathBuf>>>,
}

impl FolderWatch {
    pub fn start(tx: Sender<AppEvent>) -> Self {
        let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
        // Debounce thread: collect events, report each folder once it has been quiet.
        thread::spawn(move || {
            // The folder and when its newest change was seen.
            let mut pending: Option<(PathBuf, Instant)> = None;
            loop {
                match raw_rx.recv_timeout(SETTLE) {
                    Ok(dir) => pending = Some((dir, Instant::now())),
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some((dir, at)) = pending.take()
                            && tx.send(AppEvent::FolderChanged { dir, at }).is_err()
                        {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        let shared: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let watched = Arc::clone(&shared);
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && !matches!(event.kind, notify::EventKind::Access(_))
                && let Some(dir) = watched.lock().ok().and_then(|d| d.clone())
            {
                let _ = raw_tx.send(dir);
            }
        })
        .ok();
        Self {
            watcher,
            current: None,
            shared,
        }
    }

    /// Watches `dir` (not its subfolders) instead of the previous folder.
    pub fn watch(&mut self, dir: &Path) {
        if self.current.as_deref() == Some(dir) {
            return;
        }
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        if let Some(old) = self.current.take() {
            let _ = watcher.unwatch(&old);
        }
        if watcher.watch(dir, RecursiveMode::NonRecursive).is_ok() {
            self.current = Some(dir.to_path_buf());
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.clone_from(&self.current);
        }
    }
}

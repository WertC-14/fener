//! Application state. Rendering reads it, events change it.

mod actions;
mod code_tabs;
mod dnd;
mod file_ops;
mod git_ops;
mod menu;
mod mouse;
mod navigation;
mod places;
mod preview;
mod search;
mod settings;
mod tabs;
mod terminal_mode;

pub use actions::{Action, HELP, Menu};
pub use code_tabs::CodeTabs;

pub use file_ops::{ClipMode, Clipboard, Dialog, JobStatus, RenameInput};
pub use git_ops::GitPanel;
pub use preview::{Graphic, PreviewKey, PreviewPane};
pub use tabs::{TabLabel, TabState, Tabs};
pub use terminal_mode::{Focus, TermMode};

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use liman_core::i18n::trf;
use liman_core::job::Done;
use liman_core::{Entry, ListOptions, Place, Places, trash};
use liman_widgets::ListMode;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::widgets::TableState;

use crate::event::AppEvent;
use crate::worker;

/// Two clicks on the same row within this time count as a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// While folder counts arrive, a size sort is redone at most this often.
const COUNT_SORT_INTERVAL: Duration = Duration::from_secs(1);
/// Rows moved per mouse wheel step.
const WHEEL_STEP: isize = 3;

/// A file a browser is still downloading.
fn is_partial_download(name: &str) -> bool {
    [".part", ".crdownload", ".download", ".tmp"]
        .iter()
        .any(|ext| name.ends_with(ext))
}

/// A folder's modification time (changes when an entry is added, removed or renamed).
fn folder_time(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The three views of ADR 0003, smallest first. `+` / `-` step through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Detailed,
    Normal,
    Grid,
}

impl View {
    /// The file list mode for the two list views; `None` for the grid.
    pub const fn list_mode(self) -> Option<ListMode> {
        match self {
            Self::Detailed => Some(ListMode::Detailed),
            Self::Normal => Some(ListMode::Normal),
            Self::Grid => None,
        }
    }
}

/// A key that types a character into a text field (not Ctrl+… or Alt+…, which are commands).
fn is_typing(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Mouse button held on an entry.
struct Drag {
    from: usize,
    start: (u16, u16),
    /// Moved far enough to count as dragging.
    active: bool,
}

/// A listing of files found by a shell command (`find`, `fd`, `grep -l`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Results {
    pub command: String,
    pub count: usize,
    /// A live search (Ctrl+F) is still finding more.
    pub running: bool,
}

/// What the body of the window shows.
pub enum Listing {
    Loading,
    Ready(Vec<Entry>),
    Failed(String),
}

pub struct App {
    /// The active tab: its folder, listing, selection, history, shell... (ADR 0008: other tabs
    /// are parked as `TabState` and swapped in here).
    pub tab: TabState,
    pub running: bool,
    /// Set when the screen must be redrawn.
    pub dirty: bool,
    /// The listing on its way comes from back/forward (do not touch the stacks).
    moving_in_history: bool,
    /// Order of folder listings (`s` / `S` / header click); saved in the config.
    pub sort: liman_core::sort::SortOrder,
    /// One-line message for the status bar (cleared on the next key press).
    pub message: Option<String>,
    /// A terminal program the main loop should run in the foreground (set by Enter on a file over SSH).
    pub external: Option<(String, PathBuf)>,
    /// View chosen by the user (`+` / `-`, Ctrl+wheel).
    pub view: View,
    /// View actually drawn last frame; smaller than `view` when the terminal is too small. Written by the UI.
    pub drawn_view: View,
    /// Where the list was drawn last frame, for mouse hit-testing. Written by the UI.
    pub list_area: Rect,
    /// Where the sidebar was drawn last frame (empty when hidden). Written by the UI.
    pub sidebar_area: Rect,
    /// Highlighted row of the Places panel when it has focus (see `Sidebar::item`).
    pub sidebar_selected: usize,
    /// First row of the Places panel on screen. Written by the UI.
    pub sidebar_offset: usize,
    /// The FOLDERS tree of the Places panel.
    pub tree: liman_core::tree::DirTree,
    /// Where the path bar was drawn last frame. Written by the UI.
    pub path_bar_area: Rect,
    pub places: Places,
    pub sidebar: Vec<Place>,
    /// Folders bookmarked with Ctrl+D (shown in Places with ★).
    pub bookmarks: Vec<PathBuf>,
    pub clipboard: Option<Clipboard>,
    /// The file operation running on the worker, if any.
    pub job: Option<JobStatus>,
    /// Finished operations, newest last; undo (Ctrl+Z) walks back through them.
    pub history: Vec<Done>,
    /// F2 rename in progress.
    pub rename: Option<RenameInput>,
    /// Box size level chosen with `+` / `-` in the grid; `None` = the largest that fits.
    pub grid_level: Option<usize>,
    /// Box size level drawn last frame. Written by the UI.
    pub drawn_grid_level: usize,
    /// Where the terminal was drawn last frame. Written by the UI.
    pub term_area: Rect,
    /// Panel height chosen with Ctrl+↑/↓ or by dragging its top border; `None` = 40% of the window.
    pub term_height: Option<u16>,
    dragging_panel: bool,
    /// Entry index of the last plain or Ctrl click (start of a Shift+click range).
    click_anchor: Option<usize>,
    /// Left button held on an entry (possibly the start of a drag-and-drop).
    drag: Option<Drag>,
    /// The large view `v` returns to from the compact list.
    last_large_view: View,
    /// `g`: the git panel.
    pub git_panel: Option<GitPanel>,
    /// Commit message being typed.
    pub commit_input: Option<String>,
    /// `b` in the git panel: branches and the highlighted one.
    pub branch_picker: Option<(Vec<String>, usize)>,
    /// A `git status` is running; `git_again` asks for one more when it is done.
    git_busy: bool,
    git_again: bool,
    /// Watches the open folder for changes made by anyone.
    watch: crate::watch::FolderWatch,
    /// F3: preview of the selected item.
    pub preview: PreviewPane,
    /// Ctrl+T: the tab row (ADR 0008); the active tab's state is in the fields above.
    pub tabs: Tabs,
    /// Text files opened in the fener editor, right of the folder tabs (ADR 0012).
    pub code: CodeTabs,
    /// Command palette (Ctrl+P) or right-click menu.
    pub menu: Option<Menu>,
    /// Where the menu was drawn (for clicks). Written by the UI.
    pub menu_area: Rect,
    /// After the next listing, start renaming the selected entry (a folder just created).
    rename_after_load: bool,
    /// A question in the middle of the screen (paste conflicts, permanent delete).
    pub dialog: Option<Dialog>,
    /// What liman last put on the system clipboard (to tell it from another app's copy).
    system_clip: Option<String>,
    /// Ctrl+F: the search text being typed.
    pub search_input: Option<String>,
    /// Ctrl+L: a path being typed into the path bar.
    pub path_input: Option<String>,
    /// Newest listing/search id, shared with search workers so old searches stop early.
    current_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// `t`: the theme list (selected row, theme to go back to on Esc).
    pub theme_picker: Option<(usize, usize)>,
    /// `?`: the shortcut overview.
    pub help_open: bool,
    /// Where the theme list was drawn (for clicks). Written by the UI.
    pub picker_area: Rect,
    /// Results on their way from the worker (becomes `results` when the listing arrives).
    results_pending: Option<Results>,
    /// When the shell's folder was last read (throttles it during long output).
    cwd_checked: Instant,
    /// Folder of the listing on its way, if any.
    loading_path: Option<PathBuf>,
    /// When a size sort last took the arriving folder counts into account.
    counts_sorted: Instant,
    /// When the newest listing was requested (it shows every change made before).
    listing_started: Instant,
    /// The open folder's modification time when its listing was asked for; a different one
    /// later means something was added, removed or renamed in it.
    folder_time: Option<std::time::SystemTime>,
    /// The last "New: …" message we showed, so a later reload may replace it (other messages
    /// are left alone).
    new_message: Option<String>,
    /// The listing on its way was started by following the shell (do not send `cd` back).
    load_from_shell: bool,
    trash_dir: PathBuf,
    /// Incremented on every listing request; older results are ignored.
    generation: u64,
    last_click: Option<(Instant, usize)>,
    options: ListOptions,
    tx: Sender<AppEvent>,
}

impl App {
    /// Creates the app and starts listing `cwd` in the background.
    pub fn new(cwd: PathBuf, places: Places, tx: Sender<AppEvent>) -> Self {
        let mut app = Self {
            tab: TabState::fresh(cwd.clone()),
            running: true,
            dirty: true,
            moving_in_history: false,
            sort: liman_core::sort::SortOrder::default(),
            message: None,
            external: None,
            // The grid of type-colored boxes is what sets liman apart, so it is the first thing you see.
            view: View::Grid,
            drawn_view: View::Detailed,
            grid_level: None,
            drawn_grid_level: 0,
            term_area: Rect::default(),
            term_height: None,
            dragging_panel: false,
            last_large_view: View::Grid,
            click_anchor: None,
            drag: None,
            theme_picker: None,
            search_input: None,
            path_input: None,
            dialog: None,
            system_clip: None,
            watch: crate::watch::FolderWatch::start(tx.clone()),
            git_busy: false,
            git_panel: None,
            commit_input: None,
            branch_picker: None,
            git_again: false,
            preview: PreviewPane::new(),
            tabs: Tabs::default(),
            code: CodeTabs::default(),
            menu: None,
            menu_area: Rect::default(),
            rename_after_load: false,
            current_generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            help_open: false,
            picker_area: Rect::default(),
            results_pending: None,
            cwd_checked: Instant::now(),
            loading_path: None,
            listing_started: Instant::now(),
            folder_time: None,
            new_message: None,
            counts_sorted: Instant::now(),
            load_from_shell: false,
            list_area: Rect::default(),
            sidebar_area: Rect::default(),
            sidebar_selected: 1,
            sidebar_offset: 0,
            tree: liman_core::tree::DirTree::new(if places.home == std::path::Path::new("/") {
                vec![PathBuf::from("/")]
            } else {
                vec![places.home.clone(), PathBuf::from("/")]
            }),
            path_bar_area: Rect::default(),
            sidebar: places.sidebar_with(&[]),
            bookmarks: Vec::new(),
            clipboard: None,
            job: None,
            history: Vec::new(),
            rename: None,
            trash_dir: trash::home_trash(&places.home),
            places,
            generation: 0,
            last_click: None,
            options: ListOptions::default(),
            tx,
        };
        app.load(cwd);
        app
    }

    /// Requests a listing of `path`. The view switches when the result arrives.
    pub fn load(&mut self, path: PathBuf) {
        self.results_pending = None;
        self.start_loading(&path);
        worker::spawn_listing(self.tx.clone(), self.generation, path, self.options);
    }

    /// Asks for `git status` of the open folder (at most one at a time; asks again when it ends).
    pub fn request_git(&mut self) {
        if self.git_busy {
            self.git_again = true;
            return;
        }
        self.git_busy = true;
        worker::spawn_git_status(self.tx.clone(), self.tab.cwd.clone());
    }

    /// Reads the open folder again without blanking the view: the old list stays until the new one
    /// arrives; selection, marks and filter are kept.
    pub fn refresh(&mut self) {
        self.invalidate_preview();
        if self.tab.select_after_load.is_none() {
            self.tab.select_after_load = self.selected_entry().map(|e| e.name.clone());
        }
        self.generation += 1;
        self.current_generation
            .store(self.generation, std::sync::atomic::Ordering::Relaxed);
        self.loading_path = Some(self.tab.cwd.clone());
        self.listing_started = Instant::now();
        self.folder_time = folder_time(&self.tab.cwd);
        worker::spawn_listing(
            self.tx.clone(),
            self.generation,
            self.tab.cwd.clone(),
            self.options,
        );
    }

    fn start_loading(&mut self, path: &std::path::Path) {
        // Remember what was selected here, for when the user comes back.
        if self.tab.results.is_none()
            && let Some(entry) = self.selected_entry()
        {
            self.tab
                .remembered
                .insert(self.tab.cwd.clone(), entry.name.clone());
        }
        self.generation += 1;
        self.current_generation
            .store(self.generation, std::sync::atomic::Ordering::Relaxed);
        self.tab.listing = Listing::Loading;
        self.tab.visible.clear();
        self.tab.marked.clear();
        self.dirty = true;
        self.loading_path = Some(path.to_path_buf());
        self.listing_started = Instant::now();
        self.folder_time = folder_time(path);
    }

    /// The safety net under the watcher: reads the folder again when its time changed (or
    /// always, with `force`: the window got the focus back and sizes may have changed too).
    fn check_folder(&mut self, force: bool) {
        if self.loading_path.is_some() || self.tab.results.is_some() {
            return;
        }
        if force || folder_time(&self.tab.cwd) != self.folder_time {
            self.refresh();
        }
    }

    pub fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Tick => self.check_folder(false),
            AppEvent::Redraw => self.dirty = true,
            AppEvent::Lsp { server, event } => self.on_lsp(server, *event),
            AppEvent::CodeFormatted {
                id,
                version,
                result,
            } => self.on_code_formatted(id, version, result),
            AppEvent::Input(Event::FocusGained) => self.check_folder(true),
            AppEvent::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if self.code.active.is_some() {
                    self.on_code_key(key);
                } else {
                    self.on_key(key);
                }
            }
            AppEvent::Input(Event::Paste(text)) if self.code.active.is_some() => {
                self.on_code_paste(&text);
            }
            // In a code tab only the tab row takes clicks.
            AppEvent::Input(Event::Mouse(mouse)) if self.code.active.is_some() => {
                if let MouseEventKind::Down(button) = mouse.kind {
                    self.tab_row_click(mouse.column, mouse.row, button);
                }
            }
            AppEvent::Input(Event::Mouse(mouse)) => self.on_mouse(mouse),
            AppEvent::Input(Event::Paste(text)) => self.on_paste(text),
            AppEvent::CodeItems {
                id,
                kind,
                query,
                items,
            } => self.on_code_items(id, kind, &query, items),
            AppEvent::Input(Event::Resize(..)) => self.dirty = true,
            AppEvent::Input(_) => {}
            AppEvent::Listing {
                generation,
                path,
                result,
            } => self.on_listing(generation, path, result),
            AppEvent::JobProgress { done, total } => self.on_job_progress(done, total),
            AppEvent::JobFinished(outcome) => self.on_job_finished(outcome),
            // The shells under code tabs.
            AppEvent::TermOutput { id, bytes } if self.code_tab_with_terminal(id).is_some() => {
                if let Some(tab) = self.code_tab_with_terminal(id)
                    && let Some(term) = &mut tab.terminal
                {
                    term.process(&bytes);
                }
                self.dirty = true;
            }
            AppEvent::TermQuiet(id) if self.code_tab_with_terminal(id).is_some() => {
                if let Some(tab) = self.code_tab_with_terminal(id)
                    && let Some(term) = &mut tab.terminal
                {
                    term.on_quiet();
                }
                self.dirty = true;
            }
            AppEvent::TermExited(id) if self.code_tab_with_terminal(id).is_some() => {
                if let Some(tab) = self.code_tab_with_terminal(id) {
                    tab.terminal = None;
                    tab.term_open = false;
                    tab.term_focus = false;
                }
                self.dirty = true;
            }
            AppEvent::TermOutput { id, bytes } if self.is_active_terminal(id) => {
                self.on_term_output(&bytes)
            }
            AppEvent::TermOutput { id, bytes } => {
                self.on_background_terminal(id, tabs::BackgroundTerm::Output(bytes))
            }
            AppEvent::TermExited(id) if self.is_active_terminal(id) => self.on_term_exited(),
            AppEvent::TermExited(id) => {
                self.on_background_terminal(id, tabs::BackgroundTerm::Exited)
            }
            AppEvent::TermQuiet(id) if !self.is_active_terminal(id) => {
                self.on_background_terminal(id, tabs::BackgroundTerm::Quiet)
            }
            // Our own jobs refresh the folder as soon as they finish; their changes reach the
            // watcher too, a moment later. A listing started after the change already shows it.
            AppEvent::FolderChanged { dir, at } => {
                let moving_away = self
                    .loading_path
                    .as_ref()
                    .is_some_and(|p| *p != self.tab.cwd);
                if dir == self.tab.cwd
                    && self.tab.results.is_none()
                    && !moving_away
                    && at > self.listing_started
                {
                    self.refresh();
                }
            }
            AppEvent::Git { dir, status } => {
                self.git_busy = false;
                if dir == self.tab.cwd {
                    self.tab.git = status;
                    self.dirty = true;
                }
                if std::mem::take(&mut self.git_again) {
                    self.request_git();
                }
            }
            AppEvent::SearchFound {
                generation,
                entries,
                done,
            } => self.on_search_found(generation, entries, done),
            AppEvent::Counts {
                generation,
                counts,
                done,
            } => self.on_counts(generation, counts, done),
            AppEvent::GitDone { label, result } => self.on_git_done(label, result),
            AppEvent::GitDiff { path, lines } => self.on_git_diff(path, lines),
            AppEvent::TreeChildren { dir, children } => {
                self.tree.set_children(dir, children);
                self.dirty = true;
            }
            AppEvent::GitRemote { root, remote } => self.on_git_remote(&root, remote),
            AppEvent::GitBranches(result) => self.on_git_branches(result),
            AppEvent::Preview {
                key,
                preview,
                graphic,
            } => self.on_preview(key, *preview, graphic),
            AppEvent::TermQuiet(_) => {
                let Some(term) = &mut self.tab.terminal else {
                    return;
                };
                term.on_quiet();
                let finished = term.take_finished_command();
                self.follow_shell_cwd();
                if let Some(out) = finished {
                    self.on_command_finished(out);
                }
            }
        }
    }

    fn is_active_terminal(&self, id: u64) -> bool {
        self.tab.terminal.as_ref().is_some_and(|t| t.id == id)
    }

    /// Entries currently shown, in display order.
    pub fn visible_entries(&self) -> impl Iterator<Item = &Entry> + '_ {
        let entries: &[Entry] = match &self.tab.listing {
            Listing::Ready(entries) => entries,
            _ => &[],
        };
        self.tab.visible.iter().filter_map(|&i| entries.get(i))
    }

    /// The entry on display row `row`.
    pub fn visible_entry(&self, row: usize) -> Option<&Entry> {
        let Listing::Ready(entries) = &self.tab.listing else {
            return None;
        };
        self.tab.visible.get(row).map(|&i| &entries[i])
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        let Listing::Ready(entries) = &self.tab.listing else {
            return None;
        };
        let row = self.tab.table.selected()?;
        self.tab.visible.get(row).map(|&i| &entries[i])
    }

    fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        self.message = None;
        // Terminal mode keys work everywhere, whichever side has focus.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::F(4) => return self.toggle_panel(),
            // Alt+1…9 picks a tab from anywhere, the terminal included.
            KeyCode::Char(c @ '1'..='9') if key.modifiers.contains(KeyModifiers::ALT) => {
                return self.switch_any_tab(c as usize - '1' as usize);
            }
            KeyCode::F(3) => return self.toggle_preview(),
            KeyCode::Char('o') if ctrl => return self.toggle_fullscreen(),
            KeyCode::F(6) if self.tab.term_mode == TermMode::Panel => return self.switch_focus(),
            KeyCode::Up if ctrl && self.tab.term_mode == TermMode::Panel => {
                return self.resize_panel(2);
            }
            KeyCode::Down if ctrl && self.tab.term_mode == TermMode::Panel => {
                return self.resize_panel(-2);
            }
            // Tab / Shift+Tab move between Places, Files and the terminal panel. In the terminal,
            // Tab completes while there is text on the command line.
            // While a path is typed, Tab completes it instead.
            KeyCode::Tab
                if self.path_input.is_none()
                    && (!self.terminal_has_focus()
                        || (self.tab.term_mode == TermMode::Panel
                            && self.terminal_line_empty())) =>
            {
                return self.focus_next();
            }
            KeyCode::BackTab if self.tab.term_mode != TermMode::Fullscreen => {
                return self.focus_prev();
            }
            _ => {}
        }
        if self.terminal_has_focus() {
            return self.on_term_key(key);
        }
        if self.tab.focus == Focus::Places && self.on_places_key(key) {
            return;
        }
        if self.dialog.is_some() {
            return self.on_dialog_key(key);
        }
        if self.menu.is_some() {
            return self.on_menu_key(key);
        }
        if self.tab.focus == Focus::Preview {
            return self.on_preview_key(key);
        }
        if self.commit_input.is_some() {
            return self.on_commit_key(key);
        }
        if self.branch_picker.is_some() {
            return self.on_branch_key(key);
        }
        if self.git_panel.is_some() {
            return self.on_git_panel_key(key);
        }
        if self.theme_picker.is_some() {
            return self.on_picker_key(key);
        }
        if self.help_open {
            self.help_open = false; // any key closes the overview
            return;
        }
        if self.rename.is_some() {
            self.on_rename_key(key);
            return;
        }
        if self.search_input.is_some() {
            return self.on_search_key(key);
        }
        if self.path_input.is_some() {
            return self.on_path_key(key);
        }
        // Keys that are actions run exactly what the palette and the menus run.
        if let Some(action) = actions::for_key(key) {
            return self.run_action(action);
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Alt+arrows move in space: ← / ↑ parent (an action), → enters the selected folder,
            // ↓ opens the selection. History is Ctrl+← / Ctrl+→ (fm-research LOG 2026-10-05).
            KeyCode::Right if alt => {
                if self.selected_entry().is_some_and(|e| e.is_dir) {
                    self.activate_selected();
                }
            }
            KeyCode::Down if alt => self.activate_selected(),
            KeyCode::Char('p') if ctrl => self.open_palette(),
            KeyCode::Char(' ') if ctrl => self.toggle_mark_here(),
            KeyCode::Char(' ') => self.toggle_mark(),
            KeyCode::Esc if self.tab.results.is_some() => self.go_up(),
            KeyCode::Esc => self.tab.marked.clear(),
            // Shift+arrows / Home / End / PgUp / PgDn: mark everything from where Shift started.
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
                if key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.extend_selection(key.code)
            }
            // In the grid, left/right move between tiles and up/down jump a row (like Nautilus).
            KeyCode::Right | KeyCode::Char('l') if self.drawn_view == View::Grid => {
                self.move_selection(1)
            }
            KeyCode::Left | KeyCode::Char('h') if self.drawn_view == View::Grid => {
                self.move_selection(-1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.click_anchor = None;
                self.move_selection(self.vertical_step())
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.click_anchor = None;
                self.move_selection(-self.vertical_step())
            }
            KeyCode::PageDown => self.move_selection(self.page()),
            KeyCode::PageUp => self.move_selection(-self.page()),
            KeyCode::Home | KeyCode::Char('g') => self.select_row(0),
            KeyCode::End | KeyCode::Char('G') => self.select_row(usize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.activate_selected(),
            KeyCode::Left | KeyCode::Char('h') => self.go_up(),
            _ => self.dirty = false,
        }
    }

    /// Folder child counts arrived (after the listing): fill them in; a size sort uses them.
    fn on_counts(
        &mut self,
        generation: u64,
        counts: Vec<(PathBuf, Option<usize>, Option<liman_core::FileType>)>,
        done: bool,
    ) {
        if generation != self.generation {
            return;
        }
        let Listing::Ready(entries) = &mut self.tab.listing else {
            return;
        };
        let counts: std::collections::HashMap<PathBuf, _> =
            counts.into_iter().map(|(p, n, t)| (p, (n, t))).collect();
        for entry in entries.iter_mut().filter(|e| e.is_dir) {
            if let Some((count, contents)) = counts.get(&entry.path) {
                entry.item_count = *count;
                entry.contents = *contents;
            }
        }
        // A size sort uses the counts. Sorting again moves the selection around, so it happens
        // once at the end, and at most once a second while a long count is still running.
        let due = done || self.counts_sorted.elapsed() >= COUNT_SORT_INTERVAL;
        if self.sort.key == liman_core::sort::SortKey::Size && self.tab.results.is_none() && due {
            self.counts_sorted = Instant::now();
            self.set_sort_quietly(self.sort);
        }
        self.dirty = true;
    }

    fn on_listing(&mut self, generation: u64, path: PathBuf, result: Result<Vec<Entry>, String>) {
        if generation != self.generation {
            return; // stale: a newer request is on its way
        }
        self.loading_path = None;
        self.tab.results = self.results_pending.take();
        if let (Some(results), Ok(entries)) = (&mut self.tab.results, &result) {
            results.count = entries.len();
        }
        let from_shell = std::mem::take(&mut self.load_from_shell);
        if result.is_ok() {
            self.sync_shell_to(&path, from_shell);
            if self.results_pending.is_none() && self.tab.results.is_none() {
                self.watch.watch(&path);
            }
        }
        // History: a move to another folder (not back/forward, not a reload) is a new step.
        let from_history = std::mem::take(&mut self.moving_in_history);
        if path != self.tab.cwd {
            if !from_history {
                self.tab.back_stack.push(self.tab.cwd.clone());
                self.tab.forward_stack.clear();
            }
            if self.tab.select_after_load.is_none() {
                self.tab.select_after_load = self.tab.remembered.get(&path).cloned();
            }
        }
        // A reload of the same folder keeps the counts it had until the new ones arrive (no flicker).
        // Counts of subfolders whose modification time is unchanged are still right: a folder's
        // time changes whenever something inside it is added, removed or renamed.
        type Known = (
            usize,
            Option<liman_core::FileType>,
            Option<std::time::SystemTime>,
        );
        let old_counts: std::collections::HashMap<PathBuf, Known> = match &self.tab.listing {
            Listing::Ready(old) if path == self.tab.cwd => old
                .iter()
                .filter_map(|e| Some((e.path.clone(), (e.item_count?, e.contents, e.modified))))
                .collect(),
            _ => Default::default(),
        };
        // A reload of the same folder: what is new gets named in the status line, so a file that
        // arrives (a download) is seen even when the sort puts it in the middle.
        let old_names: Option<std::collections::HashSet<String>> = match &self.tab.listing {
            Listing::Ready(old) if path == self.tab.cwd && self.tab.results.is_none() => {
                Some(old.iter().map(|e| e.name.clone()).collect())
            }
            _ => None,
        };
        let mut to_count = Vec::new();
        self.tab.cwd = path;
        self.tab.listing = match result {
            Ok(mut entries) => {
                for entry in entries.iter_mut().filter(|e| e.is_dir) {
                    entry.special = self.places.kind_of(&entry.path);
                    let old = old_counts.get(&entry.path);
                    if let Some((count, contents, _)) = old {
                        entry.item_count = Some(*count);
                        entry.contents = *contents;
                    }
                    let unchanged = old.is_some_and(|(_, _, modified)| {
                        modified.is_some() && *modified == entry.modified
                    });
                    if !unchanged {
                        to_count.push(entry.path.clone());
                    }
                }
                // Folders come sorted by name; results keep the command's order.
                if self.results_pending.is_none()
                    && self.sort != liman_core::sort::SortOrder::default()
                {
                    liman_core::sort::sort_with(&mut entries, self.sort);
                }
                Listing::Ready(entries)
            }
            Err(err) => Listing::Failed(err),
        };
        let ours = self.message.is_none() || self.message == self.new_message;
        if let (Some(old), Listing::Ready(entries), true) = (&old_names, &self.tab.listing, ours) {
            // A download in progress (`.part`, `.crdownload`) is not news yet; its file is.
            let new: Vec<&str> = entries
                .iter()
                .filter(|e| !old.contains(&e.name) && !is_partial_download(&e.name))
                .map(|e| e.name.as_str())
                .collect();
            let note = match new.len() {
                0 => None,
                1..=3 => Some(trf("New: {}", &[&new.join(", ")])),
                n => Some(trf("{} new items", &[&n])),
            };
            if note.is_some() {
                self.message.clone_from(&note);
                self.new_message = note;
            }
        }
        if let (Listing::Ready(entries), None) = (&self.tab.listing, &self.tab.results) {
            let dirs = entries
                .iter()
                .filter(|e| e.is_dir)
                .map(|e| (e.name.clone(), e.path.clone()))
                .collect();
            self.update_tree(dirs);
        }
        if !to_count.is_empty() {
            self.counts_sorted = Instant::now();
            worker::spawn_counts(
                self.tx.clone(),
                self.generation,
                self.current_generation.clone(),
                to_count,
                self.options,
            );
        }
        self.refresh_visible();
        self.tab.table = TableState::default();
        let came_from = self.tab.select_after_load.take();
        let row = came_from
            .and_then(|name| self.visible_entries().position(|e| e.name == name))
            .unwrap_or(0);
        self.select_row(row);
        if self
            .tab
            .git
            .as_ref()
            .is_none_or(|g| !self.tab.cwd.starts_with(&g.root))
        {
            self.tab.git = None; // left the repository; the new status arrives soon
        }
        self.request_git();
        if std::mem::take(&mut self.rename_after_load) {
            self.begin_rename();
        }
        self.dirty = true;
    }

    pub fn entry_count(&self) -> Option<usize> {
        match &self.tab.listing {
            Listing::Ready(_) => Some(self.tab.visible.len()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;

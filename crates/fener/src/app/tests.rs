use super::*;
use liman_core::FileType;
use liman_widgets::Sidebar;
use ratatui::crossterm::event::{KeyEventState, MouseButton, MouseEvent};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

fn key(code: KeyCode) -> AppEvent {
    AppEvent::Input(Event::Key(KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }))
}

fn click(column: u16, row: u16) -> AppEvent {
    AppEvent::Input(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }))
}

fn entry(name: &str, is_dir: bool) -> Entry {
    let path = PathBuf::from("/data").join(name);
    Entry {
        name: name.into(),
        file_type: FileType::from_path(&path, is_dir),
        path,
        is_dir,
        is_symlink: false,
        special: None,
        size: 0,
        item_count: None,
        contents: None,
        modified: None,
    }
}

/// Home is /data, Music is the XDG music dir.
fn places() -> Places {
    Places::from_user_dirs("XDG_MUSIC_DIR=\"$HOME/Music\"", Path::new("/data"))
}

/// App showing /data with: Music/, Projects/, a.txt, b.pdf, c.rs
fn app() -> (App, Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(PathBuf::from("/data"), places(), tx);
    let generation = app.generation;
    app.handle(AppEvent::Listing {
        generation,
        path: PathBuf::from("/data"),
        result: Ok(vec![
            entry("Music", true),
            entry("Projects", true),
            entry("a.txt", false),
            entry("b.pdf", false),
            entry("c.rs", false),
        ]),
    });
    app.list_area = Rect::new(24, 2, 60, 10); // rows start at y = 4
    app.sidebar_area = Rect::new(0, 1, 22, 10);
    app.sidebar = vec![Place {
        name: "Home".into(),
        path: PathBuf::from("/data"),
        kind: liman_core::SpecialDir::Home,
    }];
    app.path_bar_area = Rect::new(0, 0, 80, 1);
    app.dirty = false;
    (app, rx)
}

fn selected_name(app: &App) -> String {
    app.selected_entry().unwrap().name.clone()
}

#[test]
fn q_and_ctrl_q_quit_esc_and_ctrl_c_do_not() {
    let (mut app, _rx) = app();
    app.handle(key(KeyCode::Esc));
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    ))));
    assert!(app.running);
    app.handle(key(KeyCode::Char('q')));
    assert!(!app.running);

    let (mut app, _rx) = self::app();
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
    ))));
    assert!(!app.running);
}

#[test]
fn moving_the_selection_is_clamped() {
    let (mut app, _rx) = app();
    assert_eq!(selected_name(&app), "Music");
    app.handle(key(KeyCode::Char('j')));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected_name(&app), "a.txt");
    app.handle(key(KeyCode::End));
    assert_eq!(selected_name(&app), "c.rs");
    app.handle(key(KeyCode::Down));
    assert_eq!(selected_name(&app), "c.rs");
    app.handle(key(KeyCode::Char('g')));
    app.handle(key(KeyCode::Up));
    assert_eq!(selected_name(&app), "Music");
}

#[test]
fn enter_on_a_folder_loads_it() {
    let (mut app, _rx) = app();
    app.handle(key(KeyCode::Char('j'))); // Projects
    app.handle(key(KeyCode::Enter));
    assert!(matches!(app.tab.listing, Listing::Loading));
}

#[test]
fn enter_on_a_file_keeps_the_listing() {
    let (mut app, _rx) = app();
    app.handle(key(KeyCode::Char('G')));
    // Do not actually launch anything from a test: only check the folder did not change.
    let entry = app.selected_entry().unwrap().clone();
    assert!(!entry.is_dir);
    assert!(matches!(app.tab.listing, Listing::Ready(_)));
}

#[test]
fn going_up_selects_the_folder_we_came_from() {
    let (tx, _rx) = mpsc::channel();
    let mut app = App::new(PathBuf::from("/data/Projects"), places(), tx);
    app.handle(key(KeyCode::Backspace));
    let generation = app.generation;
    app.handle(AppEvent::Listing {
        generation,
        path: PathBuf::from("/data"),
        result: Ok(vec![
            entry("Music", true),
            entry("Projects", true),
            entry("a.txt", false),
        ]),
    });
    assert_eq!(app.tab.cwd, PathBuf::from("/data"));
    assert_eq!(selected_name(&app), "Projects");
}

#[test]
fn click_selects_and_double_click_opens() {
    let (mut app, _rx) = app();
    app.handle(click(30, 5)); // second row: Projects
    assert_eq!(selected_name(&app), "Projects");
    assert!(matches!(app.tab.listing, Listing::Ready(_)));
    app.handle(click(30, 5));
    assert!(matches!(app.tab.listing, Listing::Loading));
}

#[test]
fn clicks_outside_the_rows_are_ignored() {
    let (mut app, _rx) = app();
    app.handle(click(30, 2)); // header
    app.handle(click(30, 11)); // below the last entry
    assert_eq!(selected_name(&app), "Music");
}

#[test]
fn special_folders_are_marked_after_loading() {
    let (app, _rx) = app();
    let music = app.visible_entry(0).unwrap();
    assert_eq!(music.name, "Music");
    assert_eq!(music.special, Some(liman_core::SpecialDir::Music));
    assert_eq!(app.visible_entry(1).unwrap().special, None);
}

#[test]
fn clicking_the_sidebar_or_path_bar_navigates() {
    let (mut app, _rx) = app();
    app.handle(click(5, 1)); // the QUICK ACCESS title: nothing
    assert!(!matches!(app.tab.listing, Listing::Loading));
    app.handle(click(5, 2)); // the first place: Home (/data)
    assert!(matches!(app.tab.listing, Listing::Loading));

    let (tx, _rx) = mpsc::channel();
    let mut app = App::new(PathBuf::from("/data/Projects"), places(), tx);
    app.path_bar_area = Rect::new(0, 0, 80, 1);
    let generation = app.generation;
    app.handle(AppEvent::Listing {
        generation,
        path: PathBuf::from("/data/Projects"),
        result: Ok(Vec::new()),
    });
    app.handle(click(3, 0)); // "⌂ Home"
    assert!(matches!(app.tab.listing, Listing::Loading));
}

#[test]
fn tilde_goes_home() {
    let (tx, _rx) = mpsc::channel();
    let mut app = App::new(PathBuf::from("/data/Projects"), places(), tx);
    app.handle(key(KeyCode::Char('~')));
    let generation = app.generation;
    assert_eq!(generation, 2);
}

#[test]
fn plus_minus_and_ctrl_wheel_switch_views() {
    let (mut app, _rx) = app();
    assert_eq!(app.view, View::Grid); // default
    app.view = View::Detailed;
    app.handle(key(KeyCode::Char('+')));
    assert_eq!(app.view, View::Normal);
    app.handle(key(KeyCode::Char('+')));
    assert_eq!((app.view, app.grid_level), (View::Grid, None)); // grid, fitted to the window
    app.handle(key(KeyCode::Char('+'))); // larger boxes
    assert_eq!(app.grid_level, Some(1));
    app.drawn_grid_level = 1;
    app.handle(key(KeyCode::Char('-')));
    assert_eq!(app.grid_level, Some(0));
    app.drawn_grid_level = 0;
    app.handle(key(KeyCode::Char('-'))); // smallest boxes: back to the box list
    app.handle(key(KeyCode::Char('-')));
    assert_eq!(app.view, View::Detailed);

    app.handle(AppEvent::Input(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 30,
        row: 5,
        modifiers: KeyModifiers::CONTROL,
    })));
    assert_eq!(app.view, View::Normal);
    assert_eq!(selected_name(&app), "Music"); // Ctrl+wheel does not move the selection
}

#[test]
fn clicks_use_the_drawn_view() {
    let (mut app, _rx) = app();
    app.drawn_view = View::Normal;
    // list_area.y = 2, rows start at 4; entry 1 (Projects) spans rows 9..=12
    app.handle(click(30, 10));
    assert_eq!(selected_name(&app), "Projects");
}

#[test]
fn alt_arrows_enter_and_leave_folders_in_every_view() {
    let (mut app, _rx) = app();
    app.drawn_view = View::Grid;
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::ALT,
    ))));
    assert!(matches!(app.tab.listing, Listing::Loading)); // entered Music

    let (tx, _rx) = mpsc::channel();
    let mut app = App::new(PathBuf::from("/data/Projects"), places(), tx);
    app.drawn_view = View::Grid;
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Up,
        KeyModifiers::ALT,
    ))));
    assert_eq!(app.generation, 2); // asked for /data
}

#[test]
fn v_switches_between_compact_list_and_large_view() {
    let (mut app, _rx) = app();
    assert_eq!(app.view, View::Grid);
    app.handle(key(KeyCode::Char('v')));
    assert_eq!(app.view, View::Detailed);
    app.handle(key(KeyCode::Char('v')));
    assert_eq!(app.view, View::Grid);
}

#[test]
fn tab_cycles_places_files_and_places_keys_open_a_place() {
    let (mut app, _rx) = app();
    app.sidebar_area = Rect::new(0, 1, 22, 10);
    app.tab.focus = Focus::Places;
    app.handle(key(KeyCode::Tab));
    assert_eq!(app.tab.focus, Focus::Files);
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    ))));
    assert_eq!(app.tab.focus, Focus::Places);
    app.handle(key(KeyCode::Down)); // stays on the only place
    app.handle(key(KeyCode::Enter)); // opens Home (/data)
    assert_eq!(app.tab.focus, Focus::Files);
    assert!(matches!(app.tab.listing, Listing::Loading));
}

#[test]
fn theme_picker_previews_and_esc_restores() {
    let (mut app, _rx) = app();
    let before = liman_widgets::theme::current_index();
    app.handle(key(KeyCode::Char('t')));
    app.handle(key(KeyCode::Down));
    assert_ne!(liman_widgets::theme::current_index(), before); // previewed
    app.handle(key(KeyCode::Esc));
    assert_eq!(liman_widgets::theme::current_index(), before);
    assert!(app.theme_picker.is_none());
}

#[test]
fn ctrl_h_toggles_hidden_files() {
    let (mut app, _rx) = app();
    let ctrl_h = AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL,
    )));
    app.places.home = std::env::temp_dir().join("liman-test-no-config"); // keep the real config untouched
    app.handle(ctrl_h);
    assert!(app.options.show_hidden);
    assert!(matches!(app.tab.listing, Listing::Loading));
}

/// Delivers a listing of `path` with the given names (all folders) for the newest request.
fn arrive(app: &mut App, path: &str, names: &[&str]) {
    let generation = app.generation;
    app.handle(AppEvent::Listing {
        generation,
        path: PathBuf::from(path),
        result: Ok(names.iter().map(|n| entry(n, true)).collect()),
    });
}

#[test]
fn alt_left_right_are_up_and_into_never_back_into_a_folder() {
    let (mut app, _rx) = app(); // in /data, Music selected
    let alt = |code| AppEvent::Input(Event::Key(KeyEvent::new(code, KeyModifiers::ALT)));
    app.handle(key(KeyCode::Char('j'))); // Projects
    app.handle(alt(KeyCode::Right)); // into Projects
    arrive(&mut app, "/data/Projects", &["liman"]);
    app.handle(alt(KeyCode::Left)); // up to /data, Projects selected
    arrive(&mut app, "/data", &["Music", "Projects"]);
    assert_eq!(selected_name(&app), "Projects");
    app.handle(alt(KeyCode::Left)); // up again: /, not back into Projects
    arrive(&mut app, "/", &["data"]);
    assert_eq!(app.tab.cwd, PathBuf::from("/"));
}

#[test]
fn shift_arrows_mark_a_range_from_where_shift_started() {
    let (mut app, _rx) = app(); // /data: Music, Projects
    arrive(&mut app, "/data", &["a", "b", "c", "d"]);
    app.drawn_view = View::Detailed;
    let shift = |code| AppEvent::Input(Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT)));
    app.handle(key(KeyCode::Char('j'))); // b
    app.handle(shift(KeyCode::Down)); // b..c
    app.handle(shift(KeyCode::Down)); // b..d
    let marked = |app: &App| {
        let mut names: Vec<String> = app
            .tab
            .marked
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(marked(&app), ["b", "c", "d"]);
    app.handle(shift(KeyCode::Up)); // back to b..c
    assert_eq!(marked(&app), ["b", "c"]);
    app.handle(shift(KeyCode::Home)); // a..b
    assert_eq!(marked(&app), ["a", "b"]);
}

#[test]
fn ctrl_left_right_walk_the_history_and_restore_the_selection() {
    let (mut app, _rx) = app(); // in /data, Music selected
    app.handle(key(KeyCode::Char('j'))); // Projects
    app.handle(key(KeyCode::Enter));
    arrive(&mut app, "/data/Projects", &["liman"]);
    let alt = |code| AppEvent::Input(Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL)));
    app.handle(alt(KeyCode::Left)); // back
    arrive(&mut app, "/data", &["Music", "Projects"]);
    assert_eq!(app.tab.cwd, PathBuf::from("/data"));
    assert_eq!(selected_name(&app), "Projects"); // remembered
    app.handle(alt(KeyCode::Right)); // forward
    arrive(&mut app, "/data/Projects", &["liman"]);
    assert_eq!(app.tab.cwd, PathBuf::from("/data/Projects"));
    assert!(app.tab.forward_stack.is_empty());
    assert_eq!(app.tab.back_stack, [PathBuf::from("/data")]);
}

#[test]
fn ctrl_f_searches_subfolders_into_a_results_view() {
    let dir = std::env::temp_dir().join(format!("liman-ctrlf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/report.pdf"), "x").unwrap();
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(dir.clone(), Places::from_user_dirs("", &dir), tx);
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    ))));
    for c in "REPORT".chars() {
        app.handle(key(KeyCode::Char(c)));
    }
    // Live: the results view is there while typing, Enter only keeps it.
    assert!(app.tab.results.is_some());
    app.handle(key(KeyCode::Enter));
    assert!(app.search_input.is_none());
    while app.tab.results.as_ref().is_none_or(|r| r.running) {
        let ev = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("search result");
        app.handle(ev);
    }
    assert_eq!(app.tab.results.as_ref().unwrap().count, 1);
    assert_eq!(app.visible_entry(0).unwrap().name, "sub/report.pdf");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ctrl_d_bookmarks_the_selected_folder_and_again_removes_it() {
    let (mut app, _rx) = app();
    let bookmarks_in_sidebar = |app: &App| {
        app.sidebar
            .iter()
            .filter(|p| p.kind == liman_core::SpecialDir::Bookmark)
            .count()
    };
    let ctrl_d = || {
        AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
        )))
    };
    app.handle(key(KeyCode::Char('j'))); // Projects
    app.handle(ctrl_d());
    assert_eq!(app.bookmarks, [PathBuf::from("/data/Projects")]);
    assert_eq!(bookmarks_in_sidebar(&app), 1);
    assert!(
        app.sidebar
            .iter()
            .any(|p| p.kind == liman_core::SpecialDir::Bookmark && p.name == "Projects")
    );
    app.handle(ctrl_d());
    assert!(app.bookmarks.is_empty());
    assert_eq!(bookmarks_in_sidebar(&app), 0);
    // Files can be pinned too.
    app.handle(key(KeyCode::Char('j'))); // a.txt
    app.handle(ctrl_d());
    assert_eq!(app.bookmarks, [PathBuf::from("/data/a.txt")]);
    app.handle(ctrl_d());
    // Dragging an entry onto the QUICK ACCESS title pins it.
    let on_title = (app.sidebar_area.x + 3, app.sidebar_area.y);
    app.handle(AppEvent::Input(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 30,
        row: 4, // first row of the list: Music
        modifiers: KeyModifiers::NONE,
    })));
    for (kind, (column, row)) in [
        (MouseEventKind::Drag(MouseButton::Left), on_title),
        (MouseEventKind::Up(MouseButton::Left), on_title),
    ] {
        app.handle(AppEvent::Input(Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })));
    }
    assert_eq!(app.bookmarks, [PathBuf::from("/data/Music")]);
}

#[test]
fn palette_runs_the_chosen_action_and_right_click_offers_entry_actions() {
    let (mut app, _rx) = app();
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('p'),
        KeyModifiers::CONTROL,
    ))));
    for c in "small".chars() {
        app.handle(key(KeyCode::Char(c)));
    }
    app.handle(key(KeyCode::Enter));
    assert!(app.menu.is_none());
    assert_eq!(app.view, View::Detailed); // "Small list / large view" ran

    app.drawn_view = View::Detailed;
    app.handle(AppEvent::Input(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 30,
        row: 5, // Projects
        modifiers: KeyModifiers::NONE,
    })));
    let menu = app.menu.as_ref().expect("context menu");
    assert_eq!(menu.items, Action::ON_ENTRY);
    assert_eq!(selected_name(&app), "Projects");
}

#[test]
fn s_sorts_in_place_and_keeps_the_selection() {
    let (mut app, _rx) = app();
    app.sort.key = liman_core::sort::SortKey::Modified; // so 's' goes to Type next
    app.handle(key(KeyCode::Char('G'))); // c.rs
    app.set_sort(liman_core::sort::SortOrder {
        key: liman_core::sort::SortKey::Type,
        descending: true,
    });
    let names: Vec<_> = app.visible_entries().map(|e| e.name.clone()).collect();
    assert_eq!(names, ["Music", "Projects", "a.txt", "c.rs", "b.pdf"]);
    assert_eq!(selected_name(&app), "c.rs");
}

#[test]
fn ctrl_arrows_resize_the_panel_within_limits() {
    let (mut app, _rx) = app();
    app.tab.term_mode = TermMode::Panel;
    app.term_area = Rect::new(0, 20, 80, 10);
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Up,
        KeyModifiers::CONTROL,
    ))));
    assert_eq!(app.term_height, Some(12));
    for _ in 0..10 {
        app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::CONTROL,
        ))));
    }
    assert_eq!(app.term_height, Some(4));
}

#[test]
fn grid_arrows_move_by_tile_and_by_row() {
    let (mut app, _rx) = app();
    app.drawn_view = View::Grid;
    app.list_area = Rect::new(24, 2, 40, 30); // 2 tiles per row
    app.handle(key(KeyCode::Right));
    assert_eq!(selected_name(&app), "Projects");
    app.handle(key(KeyCode::Down));
    assert_eq!(selected_name(&app), "b.pdf");
    app.handle(key(KeyCode::Left));
    assert_eq!(selected_name(&app), "a.txt");
    assert_eq!(app.tab.cwd, PathBuf::from("/data")); // left did not go up a folder
}

#[test]
fn wheel_moves_the_selection() {
    let (mut app, _rx) = app();
    app.handle(AppEvent::Input(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 30,
        row: 5,
        modifiers: KeyModifiers::NONE,
    })));
    assert_eq!(selected_name(&app), "b.pdf");
}

#[test]
fn folder_changes_already_in_the_listing_do_not_reload() {
    let (mut app, _rx) = app();
    let generation = app.generation;
    // A change seen before the listing was requested is in it: nothing to do.
    let before = app.listing_started - Duration::from_millis(1);
    app.handle(AppEvent::FolderChanged {
        dir: PathBuf::from("/data"),
        at: before,
    });
    assert_eq!(app.generation, generation);
    // A newer change reloads, even while that listing is still on its way.
    app.handle(AppEvent::FolderChanged {
        dir: PathBuf::from("/data"),
        at: Instant::now(),
    });
    assert_eq!(app.generation, generation + 1);
    app.handle(AppEvent::FolderChanged {
        dir: PathBuf::from("/data"),
        at: Instant::now(),
    });
    assert_eq!(app.generation, generation + 2);
    // Changes in the folder we are leaving do not cancel the move.
    app.handle(key(KeyCode::Enter)); // into Music
    let moving = app.generation;
    app.handle(AppEvent::FolderChanged {
        dir: PathBuf::from("/data"),
        at: Instant::now(),
    });
    assert_eq!(app.generation, moving);
}

#[test]
fn rename_in_a_results_view_starts_from_the_file_name() {
    let (mut app, _rx) = app();
    let generation = app.generation;
    let mut found = entry("a.rs", false);
    found.name = "src/a.rs".into();
    found.path = PathBuf::from("/data/src/a.rs");
    app.handle(AppEvent::Listing {
        generation,
        path: PathBuf::from("/data"),
        result: Ok(vec![found]),
    });
    app.handle(key(KeyCode::F(2)));
    assert_eq!(app.rename.as_ref().map(|r| r.text.as_str()), Some("a.rs"));
}

#[test]
fn ctrl_l_types_a_path_and_enter_goes_there() {
    let (mut app, _rx) = app();
    let ctrl_l = AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('l'),
        KeyModifiers::CONTROL,
    )));
    app.handle(ctrl_l);
    assert_eq!(app.path_input.as_deref(), Some("~/")); // /data is home in these tests
    app.handle(key(KeyCode::Esc));
    assert_eq!(app.path_input, None);
    app.handle(AppEvent::Input(Event::Key(KeyEvent::new(
        KeyCode::Char('l'),
        KeyModifiers::CONTROL,
    ))));
    for _ in 0.."~/".len() {
        app.handle(key(KeyCode::Backspace));
    }
    app.handle(key(KeyCode::Char('/')));
    app.handle(key(KeyCode::Enter)); // "/" is a folder everywhere
    assert_eq!(app.path_input, None);
    assert_eq!(app.loading_path.as_deref(), Some(Path::new("/")));
}

#[test]
fn the_folder_tree_opens_the_way_and_arrows_open_and_close_branches() {
    let dir = std::env::temp_dir().join(format!("liman-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("a/b")).unwrap();
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(dir.join("a"), Places::from_user_dirs("", &dir), tx);
    let names = |app: &App| -> Vec<String> {
        app.tree
            .rows()
            .iter()
            .map(|r| format!("{}{}", r.depth, r.name))
            .collect()
    };
    // The listing of `a` and the subfolders of home arrive: open down to `a`, and `a`
    // itself shows its subfolders.
    while !names(&app).contains(&"2b".to_string()) {
        app.handle(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    let a = app.tree.rows().iter().position(|r| r.name == "a").unwrap();
    app.tab.focus = Focus::Places;
    app.sidebar_selected = Sidebar::dir_row(app.sidebar.len(), a);
    app.handle(key(KeyCode::Left)); // close `a`
    assert!(!names(&app).contains(&"2b".to_string()));
    app.handle(key(KeyCode::Right)); // open it again
    assert!(names(&app).contains(&"2b".to_string()));
    app.handle(key(KeyCode::Left));
    app.handle(key(KeyCode::Left)); // closed: go to the parent row
    assert_eq!(
        app.sidebar_selected,
        Sidebar::dir_row(app.sidebar.len(), a - 1)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn places_cursor_past_a_shrunk_tree_still_moves() {
    let (mut app, _rx) = app();
    app.tab.focus = Focus::Places;
    app.sidebar_selected = 999; // the tree was longer before
    app.handle(key(KeyCode::Up));
    assert!(app.sidebar_selected < app.sidebar_len() - 1);
}

#[test]
fn stale_results_are_ignored() {
    let (mut app, _rx) = app();
    app.load(PathBuf::from("/"));
    app.handle(AppEvent::Listing {
        generation: 1,
        path: PathBuf::from("/old"),
        result: Ok(Vec::new()),
    });
    assert!(matches!(app.tab.listing, Listing::Loading));
}

#[test]
fn tab_never_opens_a_closed_terminal() {
    let dir = std::env::temp_dir();
    let (tx, _rx) = mpsc::channel();
    let mut app = App::new(dir.clone(), Places::from_user_dirs("", &dir), tx);
    app.sidebar_area = Rect::new(0, 0, 20, 10); // sidebar visible
    let tab = AppEvent::Input(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    app.handle(tab);
    assert!(app.tab.terminal.is_none());
    assert_eq!(app.tab.term_mode, TermMode::Hidden);
    assert_eq!(app.tab.focus, Focus::Places);
}

#[test]
fn folder_counts_arrive_after_the_listing() {
    let dir = std::env::temp_dir().join(format!("liman-counts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for f in ["a", "b", "c"] {
        std::fs::write(dir.join("sub").join(f), "").unwrap();
    }
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(dir.clone(), Places::from_user_dirs("", &dir), tx);
    let count = |app: &App| match &app.tab.listing {
        Listing::Ready(e) => e[0].item_count,
        _ => None,
    };
    while count(&app).is_none() {
        app.handle(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    assert_eq!(count(&app), Some(3));

    // A reload with "sub" untouched counts nothing again.
    let next = |app: &mut App| {
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let counts = matches!(ev, AppEvent::Counts { .. });
        app.handle(ev);
        counts
    };
    app.refresh();
    while app.loading_path.is_some() {
        assert!(!next(&mut app), "unchanged folder was counted again");
    }
    let late = rx.recv_timeout(Duration::from_millis(200));
    assert!(!late.is_ok_and(|ev| matches!(ev, AppEvent::Counts { .. })));
    // A new file inside changes the folder's time: counted again.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(dir.join("sub/d"), "").unwrap();
    app.refresh();
    while count(&app) != Some(4) {
        next(&mut app);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

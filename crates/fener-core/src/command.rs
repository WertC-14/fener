//! Named commands, and the one table that gives them their keys. The leader (Space) keys, the
//! which-key box, the start screen and the "Keymaps" picker all read from here, so a command
//! added once shows up everywhere.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    FindFiles,
    Explorer,
    RecentFiles,
    FindText,
    NewFile,
    Keymaps,
    Themes,
    Dashboard,
    Save,
    Quit,
    ToggleNumbers,
    ToggleReader,
    ToggleRelativeNumbers,
    ClearSearch,
}

pub struct Binding {
    pub command: Command,
    /// Keys after Space, as LazyVim has them (`""`: no leader key).
    pub leader: &'static str,
    pub label: &'static str,
}

const fn bind(command: Command, leader: &'static str, label: &'static str) -> Binding {
    Binding {
        command,
        leader,
        label,
    }
}

pub const COMMANDS: &[Binding] = &[
    bind(Command::FindFiles, " ", "Find Files"),
    bind(Command::FindText, "/", "Find Text (Grep)"),
    bind(Command::Explorer, "e", "Explorer (folder tree)"),
    bind(Command::FindFiles, "ff", "Find Files"),
    bind(Command::RecentFiles, "fr", "Recent Files"),
    bind(Command::NewFile, "fn", "New File"),
    bind(Command::FindText, "sg", "Grep"),
    bind(Command::Keymaps, "sk", "Keymaps"),
    bind(Command::Themes, "uC", "Colorscheme with Preview"),
    bind(Command::ToggleReader, "um", "Toggle Markdown Reader"),
    bind(Command::ToggleNumbers, "ul", "Toggle Line Numbers"),
    bind(
        Command::ToggleRelativeNumbers,
        "uL",
        "Toggle Relative Numbers",
    ),
    bind(Command::ClearSearch, "ur", "Clear Search Highlight"),
    bind(Command::Quit, "qq", "Quit All"),
    bind(Command::Dashboard, "", "Start Screen (:Dashboard)"),
    bind(Command::Save, "", "Save File (Ctrl+S, :w)"),
];

/// Names of the leader groups (shown as `+name` in which-key).
pub const GROUPS: &[(&str, &str)] = &[
    ("f", "+file/find"),
    ("s", "+search"),
    ("u", "+ui"),
    ("q", "+quit/session"),
];

/// The start screen menu: key, label, command (LazyVim's dashboard, minus what fener lacks).
pub const DASHBOARD: &[(char, &str, Command)] = &[
    ('f', "Find File", Command::FindFiles),
    ('n', "New File", Command::NewFile),
    ('g', "Find Text", Command::FindText),
    ('e', "Explorer", Command::Explorer),
    ('r', "Recent Files", Command::RecentFiles),
    ('t', "Themes", Command::Themes),
    ('?', "Keymaps", Command::Keymaps),
    ('q', "Quit", Command::Quit),
];

/// Vim keys worth knowing, for the "Keymaps" picker (a cheat sheet; picking one only shows it).
pub const VIM_KEYS: &[(&str, &str)] = &[
    ("i a", "Insert before / after the cursor"),
    ("o O", "New line below / above, and insert"),
    ("Esc", "Back to Normal mode"),
    ("h j k l", "Left, down, up, right"),
    ("w b e", "Next word, previous word, end of word"),
    ("0 ^ $", "Line start, first non-blank, line end"),
    ("gg G", "First line, last line (5G: line 5)"),
    ("f t F T", "Jump to a character on the line (; , repeat)"),
    ("{ }", "Previous / next paragraph"),
    ("%", "Matching bracket"),
    ("Ctrl+D Ctrl+U", "Half a page down / up"),
    ("zz", "Center the cursor line"),
    ("v V", "Select characters / lines (Visual)"),
    ("x", "Delete a character"),
    ("dd", "Delete a line"),
    ("cc", "Change a line"),
    ("yy", "Copy (yank) a line"),
    ("p P", "Paste after / before"),
    ("dw cw yw", "Delete / change / copy a word"),
    ("diw ciw", "Delete / change the word under the cursor"),
    ("ci\" ci(", "Change inside quotes / brackets"),
    ("D C", "Delete / change to the end of the line"),
    (">> <<", "Indent / outdent"),
    ("J", "Join with the next line"),
    ("r", "Replace one character"),
    ("~", "Switch case"),
    ("u", "Undo"),
    ("Ctrl+R", "Redo"),
    (".", "Repeat the last change"),
    ("/ ?", "Search forward / backward"),
    ("n N", "Next / previous match"),
    ("*", "Search the word under the cursor"),
    (":w", "Save"),
    (":q", "Quit (:q! without saving)"),
    (":e FILE", "Open a file (:e! drops changes)"),
    ("Ctrl+H Ctrl+L", "To the folder tree / back to the text"),
    (
        "␣ u m",
        "Markdown: formatted reader / source (i, Esc: edit)",
    ),
];

/// How a leader key sequence is written for people: `␣ f f`.
pub fn leader_label(keys: &str) -> String {
    std::iter::once("␣".to_string())
        .chain(keys.chars().map(|c| {
            if c == ' ' {
                "␣".into()
            } else {
                c.to_string()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

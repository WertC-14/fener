//! Named commands, and the one table that gives them their keys. The leader (Space) keys, the
//! which-key box, the start screen and the "Keymaps" picker all read from here, so a command
//! added once shows up everywhere.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    FindFiles,
    Explorer,
    SearchLines,
    Run,
    Build,
    Headings,
    Terminal,
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

impl Command {
    /// The name used in the config file's `[keys]` table (`run`, `find-files`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Self::FindFiles => "find-files",
            Self::Explorer => "explorer",
            Self::SearchLines => "find-in-file",
            Self::Run => "run",
            Self::Build => "build",
            Self::Headings => "headings",
            Self::Terminal => "terminal",
            Self::RecentFiles => "recent-files",
            Self::FindText => "find-text",
            Self::NewFile => "new-file",
            Self::Keymaps => "keymaps",
            Self::Themes => "themes",
            Self::Dashboard => "dashboard",
            Self::Save => "save",
            Self::Quit => "quit",
            Self::ToggleNumbers => "toggle-numbers",
            Self::ToggleReader => "toggle-reader",
            Self::ToggleRelativeNumbers => "toggle-relative-numbers",
            Self::ClearSearch => "clear-search",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        COMMANDS
            .iter()
            .map(|b| b.command)
            .find(|c| c.name() == name)
    }
}

/// Where a leader key is offered: the Space menu changes with the open file (code or Markdown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    Code,
    Markdown,
}

impl When {
    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "",
            Self::Code => "code",
            Self::Markdown => "markdown",
        }
    }
}

/// The bindings offered in `context` (a `Code` or `Markdown` file).
pub fn bindings(context: When) -> impl Iterator<Item = &'static Binding> {
    COMMANDS
        .iter()
        .filter(move |b| b.when == When::Always || b.when == context)
}

pub struct Binding {
    pub when: When,
    pub command: Command,
    /// Keys after Space, as LazyVim has them (`""`: no leader key).
    pub leader: &'static str,
    pub label: &'static str,
}

const fn bind(command: Command, leader: &'static str, label: &'static str) -> Binding {
    bind_in(When::Always, command, leader, label)
}

const fn bind_in(
    when: When,
    command: Command,
    leader: &'static str,
    label: &'static str,
) -> Binding {
    Binding {
        when,
        command,
        leader,
        label,
    }
}

pub const COMMANDS: &[Binding] = &[
    // Space as a "super key": the everyday commands are one key after it.
    bind_in(When::Code, Command::Run, "↵", "Run (F5)"),
    bind_in(When::Code, Command::Build, "b", "Build / Check (no run)"),
    bind_in(
        When::Markdown,
        Command::ToggleReader,
        "r",
        "Reader / Source (Ctrl+E)",
    ),
    bind_in(When::Markdown, Command::Headings, "h", "Go to Heading"),
    bind(Command::FindFiles, " ", "Find Files"),
    bind(Command::FindText, "/", "Find Text (Grep)"),
    bind(Command::Explorer, "e", "Explorer (folder tree; Ctrl+B)"),
    bind(Command::Terminal, "t", "Terminal open / close (Ctrl+/)"),
    bind(Command::Save, "w", "Save (Ctrl+S)"),
    bind(Command::SearchLines, "o", "Find in File (Ctrl+F)"),
    bind(Command::FindFiles, "ff", "Find Files"),
    bind(Command::RecentFiles, "fr", "Recent Files"),
    bind(Command::NewFile, "fn", "New File"),
    bind(Command::FindText, "sg", "Grep"),
    bind(Command::SearchLines, "sb", "Find in File (Ctrl+F)"),
    bind(Command::Terminal, "ft", "Terminal (Ctrl+/, F4)"),
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
    ("u Ctrl+Z", "Undo (Ctrl+Z in any mode)"),
    ("Ctrl+R Ctrl+Y", "Redo (Ctrl+Y in any mode)"),
    ("Ctrl+E", "Markdown: reader <-> editing"),
    (".", "Repeat the last change"),
    ("/ ?", "Search forward / backward"),
    ("n N", "Next / previous match"),
    ("*", "Search the word under the cursor"),
    (":w", "Save"),
    (":q", "Quit (:q! without saving)"),
    (":e FILE", "Open a file (:e! drops changes)"),
    ("Tab", "Folder tree <-> text (Ctrl+H, Ctrl+L too)"),
    ("Ctrl+B", "Show / hide the folder tree"),
    ("Ctrl+F", "Find in this file"),
    ("F5", "Save, build and run in the terminal (Space Enter)"),
    ("␣ 1…9", "Go to tab 1…9 (1: the files)"),
    (
        "Ctrl+/ F4",
        "Terminal below the text; F6 moves between them",
    ),
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

//! Color schemes: [`THEMES`] are fener's own (LazyVim's tokyonight first); an embedding app
//! can make a [`Theme`] from its palette.

use std::cell::Cell;

use ratatui::style::Color;

/// A color scheme. [`THEMES`] has fener's own; an embedding app can make one from its palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    /// Text background.
    pub bg: Color,
    /// Status line, panels, picker box.
    pub bg_dark: Color,
    /// Cursor line and selected rows.
    pub bg_line: Color,
    pub bg_visual: Color,
    pub bg_search: Color,
    pub fg: Color,
    pub fg_dim: Color,
    /// Line numbers and `~` past the end.
    pub gutter: Color,
    pub orange: Color,
    pub blue: Color,
    pub green: Color,
    pub magenta: Color,
    pub yellow: Color,
    pub red: Color,
    pub cyan: Color,
}

const fn hex(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[rustfmt::skip]
pub const fn theme(name: &'static str, c: [u32; 15]) -> Theme {
    Theme {
        name, bg: hex(c[0]), bg_dark: hex(c[1]), bg_line: hex(c[2]), bg_visual: hex(c[3]),
        bg_search: hex(c[4]), fg: hex(c[5]), fg_dim: hex(c[6]), gutter: hex(c[7]),
        orange: hex(c[8]), blue: hex(c[9]), green: hex(c[10]), magenta: hex(c[11]),
        yellow: hex(c[12]), red: hex(c[13]), cyan: hex(c[14]),
    }
}

// bg, bg_dark, bg_line, bg_visual, bg_search, fg, fg_dim, gutter,
// orange, blue, green, magenta, yellow, red, cyan

#[rustfmt::skip]
pub const THEMES: [Theme; 6] = [
    theme("tokyonight-night", [0x1a1b26, 0x16161e, 0x292e42, 0x283457, 0x3d59a1, 0xc0caf5, 0x565f89, 0x3b4261,
        0xff9e64, 0x7aa2f7, 0x9ece6a, 0xbb9af7, 0xe0af68, 0xf7768e, 0x7dcfff]),
    theme("tokyonight-storm", [0x24283b, 0x1f2335, 0x292e42, 0x2e3c64, 0x3d59a1, 0xc0caf5, 0x565f89, 0x3b4261,
        0xff9e64, 0x7aa2f7, 0x9ece6a, 0xbb9af7, 0xe0af68, 0xf7768e, 0x7dcfff]),
    theme("tokyonight-moon", [0x222436, 0x1e2030, 0x2f334d, 0x2d3f76, 0x3e68d7, 0xc8d3f5, 0x636da6, 0x3b4261,
        0xff966c, 0x82aaff, 0xc3e88d, 0xc099ff, 0xffc777, 0xff757f, 0x86e1fc]),
    theme("tokyonight-day", [0xe1e2e7, 0xd0d5e3, 0xc4c8da, 0xb7c1e3, 0x7890dd, 0x3760bf, 0x848cb5, 0xa8aecb,
        0xb15c00, 0x2e7de9, 0x587539, 0x9854f1, 0x8c6c3e, 0xf52a65, 0x007197]),
    theme("catppuccin-mocha", [0x1e1e2e, 0x181825, 0x313244, 0x45475a, 0x585b70, 0xcdd6f4, 0x6c7086, 0x45475a,
        0xfab387, 0x89b4fa, 0xa6e3a1, 0xcba6f7, 0xf9e2af, 0xf38ba8, 0x89dceb]),
    theme("gruvbox", [0x282828, 0x1d2021, 0x3c3836, 0x504945, 0x665c54, 0xebdbb2, 0x928374, 0x665c54,
        0xfe8019, 0x83a598, 0xb8bb26, 0xd3869b, 0xfabd2f, 0xfb4934, 0x8ec07c]),
];

thread_local! {
    /// The theme of the frame being drawn (set by [`crate::render`]).
    static CURRENT: Cell<Theme> = const { Cell::new(THEMES[0]) };
}

/// The colors to draw with: the theme of the frame being drawn.
pub fn t() -> Theme {
    CURRENT.with(Cell::get)
}

/// Sets the theme [`t`] returns until the next frame.
pub fn set_current(theme: &Theme) {
    CURRENT.with(|c| c.set(*theme));
}

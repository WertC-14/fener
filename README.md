# fener

A modal terminal editor in the spirit of LazyVim, built with Rust and Ratatui.
It starts small and grows one piece at a time; sister project of [liman](https://github.com/WertC-14/liman).

**Status:** early development (v0.1 in progress).

## What works

- Open, edit and save a file: `fener FILE`, `:w`, `:q`, `:wq`, `:x`, `:q!`, `Ctrl+S`
- Vim's language: `[count] [operator [count]] (motion | text object)`
  - motions: `h j k l`, `w W b B e E`, `0 ^ $`, `gg G` (`5G`), `f t F T ; ,`, `{ }`, `%`, `n N`, `*`
  - operators: `d c y > <` (`dd cc yy >> <<`), text objects `iw aw iW aW i" a" i' i( a( i[ i{ i<` ...
  - `x X s S D C Y p P J r ~ u Ctrl+R .`, Insert (`i a I A o O`), Visual (`v V`)
- Search `/` `?` with smart case, highlighted matches, `:noh`
- `Space` leader with a which-key box (LazyVim: `Space q q` quit, `Space u l` / `Space u L` line numbers)
- LazyVim's look: tokyonight colors, relative line numbers, cursor line, a lualine-like status line

## Not yet (on purpose)

LSP, tree-sitter, plugins, file tree, fuzzy finder, multiple buffers and windows, Obsidian vault.
They come one by one; see fm-research ADR 0010.

## Build

```bash
cargo install --path crates/fener
fener notes.md
```

## Layout

- `crates/fener-core` — no terminal code: the document (rope + change record + undo), Vim motions and text
  objects, and the editor state machine.
- `crates/fener` — the terminal app (Ratatui).

## License

GPL-3.0-or-later.

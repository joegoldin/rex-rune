# ghostty-vt

Safe Rust API for [libghostty-vt](https://ghostty.org), the terminal
emulation library extracted from the Ghostty terminal emulator. The
underlying library is built from Ghostty's official source tarball by
[`ghostty-vt-sys`](https://crates.io/crates/ghostty-vt-sys) with Zig 0.17.

The crate version normally follows gpui-cn and records its Ghostty source as
build metadata. For example, `0.5.0+ghostty.1.3.2-main.b40acce` uses Ghostty
`1.3.2-main` at commit `b40acce`.

This crate is a fork of [libghostty-vt](https://github.com/Uzaaft/libghostty-rs)
0.2.2 (MIT OR Apache-2.0, by Uzaaft and pluiedev) pinned to a newer Ghostty
commit. Module, type and method names and call shapes are kept, and every
getter returns `Result`. The differences:

- No custom allocators and no `'alloc` lifetime. Every handle uses
  libghostty's default allocator.
- Terminal effects are queued as `terminal::Event` values and drained with
  `Terminal::events` after each `vt_write`, instead of boxed closures. The
  render hold callback (synchronized output) stays a callback because it
  must capture a frame synchronously.
- Added: render hold, `vt_write_until_ground`, continuation buffers, the
  newer terminal options (device attributes, XTVERSION, ENQ, terminfo name,
  title report, scrollback and clipboard limits, resize behavior, mode
  defaults, Kitty image limits), OSC 5522 clipboard writes, render state
  overscan, row ids, `next_dirty`, `clean`, the `search` module, the
  `snapshot` module, and the `terminfo` and `shell_integration` modules.

Handles are `!Send + !Sync`: drive a terminal from one thread.

## Resources

`terminfo::dir()` writes the compiled `xterm-ghostty` terminfo database to a
cache directory once and returns the path for `TERMINFO`.
`shell_integration::resources_dir()` does the same for Ghostty's bash, zsh,
fish, elvish and nushell integration scripts and returns the path for
`GHOSTTY_RESOURCES_DIR`. `GHOSTTY_VT_CACHE_DIR` overrides the cache root.

## Features

- `kitty-graphics` (default): the Kitty graphics API.
- `png`: a PNG decoder for Kitty graphics built on the `png` crate.
- `log`, `tracing`: forward libghostty logs to those crates.

## Requirements and environment

See the sys crate: Zig 0.17 on `PATH`, network or `GHOSTTY_SOURCE_DIR` on
the first build, `GHOSTTY_ZIG_SYSTEM_DIR` for offline package resolution.

## License

Apache-2.0. Ghostty's terminfo source and shell integration scripts are MIT
(see `LICENSE-GHOSTTY`). Files adapted from libghostty-vt say so in their
header.

# Vendored Rune: window-chrome additions

`crates/rune`, `crates/rune-motion`, `crates/rune-staging` and `crates/procwatch` are
[anomalyco/rune](https://github.com/anomalyco/rune) at
`100b7d93a442b7758f426dd0f63e2dd9cd72aff4` (MIT, see `LICENSE`), with the changes below.
The only manifest change is replacing `edition.workspace`/`license.workspace` with their
values, since these crates sit outside Rune's workspace. `procwatch` is an optional
dependency of `rune` (the `frame-profile` feature).

`rune-kit` and `rune-theme-opencode` still come from the same git revision. The root
`Cargo.toml` patches `rune`, `rune-motion` and `rune-staging` to these paths so the whole
dependency graph sees a single `rune`.

## Changes

Rex draws its own window controls in a borderless window. At this revision a borderless
Linux window cannot minimize, maximize from a button, edge-resize or open the window
manager's menu. The patch adds the following without changing existing behavior on macOS:

- `Tx::toggle_maximize()` and `Cx::window_maximized()` (refreshed on every `Resized`).
- `Tx::minimize()` on borderless windows off macOS.
- `El::window_resize(WindowEdge)` (winit `drag_resize_window`, started inside the press, like
  `window_drag`) and `Cursor::ResizeEdge(WindowEdge)`.
- `Tx::show_window_menu(x, y)` (winit `show_window_menu`; Wayland and Windows only in winit).
- Off macOS, a `window_drag` press waits for 3pt of travel before `drag_window`, so the
  region's double-click (maximize) still reaches Rune: an X11 move grab otherwise takes the
  second press.
- `set_app_icon_png()` registers a native window icon before startup, using the existing
  image dependency and unmodified RGBA. All top-level windows receive it through
  `WindowOptions`. This supplies X11's `_NET_WM_ICON` for task lists that do not resolve
  desktop-file icons; Wayland still uses the app ID and the installed desktop entry. Invalid
  PNGs return `ImageError`.
- `El::no_wrap()` for styled display text without ellipsis or fade. Rex owns terminal line
  breaks, so fractional-DPI width rounding must not wrap a text run onto another row.
- Linux system-font discovery includes `fc-list` results and deduplicates them with the
  conventional directory scan. This lets Rune's existing CJK, symbol and script fallbacks
  find fontconfig-managed files outside `/usr/share/fonts`, including Nix store fonts.
  It does not change the configured primary family or the font-selection machinery.

Spike: `examples/chrome_spike.rs` and `scripts/chrome-spike-x11.py`.

Drop this copy once upstream Rune has equivalent APIs.

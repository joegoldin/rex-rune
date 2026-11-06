//! Safe Rust API for `libghostty-vt`, the terminal emulation library
//! extracted from the [Ghostty](https://ghostty.org) terminal emulator.
//!
//! `libghostty-vt` contains the logic for handling the core parts of a
//! terminal emulator: parsing terminal escape sequences, maintaining terminal
//! state, encoding input events, etc. It handles scrollback, line wrapping,
//! reflow on resize, search, snapshots, and more.
//!
//! This crate is a fork of
//! [`libghostty-vt`](https://github.com/Uzaaft/libghostty-rs) 0.2.2
//! (MIT OR Apache-2.0, by Uzaaft and pluiedev) pinned to a newer Ghostty
//! commit. It keeps the upstream module, type and method names and call
//! shapes, with two changes:
//!
//! - There is no custom allocator support and no `'alloc` lifetime. Every
//!   handle uses libghostty's default allocator.
//! - Terminal effects (bell, title, pty output, clipboard writes, ...) are
//!   not boxed closures. They are queued as [`terminal::Event`] values and
//!   drained with [`Terminal::events`] after each [`Terminal::vt_write`].
//!   The one exception is the [render hold](Terminal::set_render_hold)
//!   callback, which must capture a frame synchronously.
//!
//! The core type is the [`Terminal`]. Start there.
//!
//! # Thread safety
//!
//! The C API is not thread-safe, so every handle in this crate is `!Send`
//! and `!Sync`. Create the terminal on the thread that owns it, and
//! communicate with other threads through channels.
//!
//! # Resources
//!
//! [`terminfo::dir`] and [`shell_integration::resources_dir`] write Ghostty's
//! `xterm-ghostty` terminfo database and shell integration scripts to a cache
//! directory once, for the process that spawns the shell.
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(missing_copy_implementations)]
#![allow(
    clippy::missing_errors_doc,
    reason = "underlying C API may return any error outside of expected and
    mitigated situations, and it is not feasible to document them all"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub use ghostty_vt_sys as ffi;

// Make sure that `Terminal`'s own impl blocks (i.e. core functions)
// are placed *before* any extra impl blocks from other modules,
// e.g. Kitty Graphics extensions, Selection APIs
pub mod terminal;

pub mod alloc;
pub mod build_info;
pub mod error;
pub mod fmt;
pub mod focus;
mod io;
pub mod key;
pub mod kitty;
pub mod log;
pub mod mouse;
pub mod osc;
pub mod paste;
pub mod render;
mod resources;
pub mod screen;
pub mod search;
pub mod selection;
pub mod sgr;
pub mod shell_integration;
pub mod snapshot;
pub mod style;
pub mod terminfo;
pub mod unicode;

#[doc(inline)]
pub use crate::{
    error::Error,
    log::{Logger, set_logger},
    render::RenderState,
    terminal::{Event, Options as TerminalOptions, Terminal},
};

pub(crate) fn sys_set<T>(opt: ffi::SysOption::Type, val: *const T) -> error::Result<()> {
    let result = unsafe { ffi::ghostty_sys_set(opt, val.cast()) };
    error::from_result(result)
}

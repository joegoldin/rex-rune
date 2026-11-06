//! Ghostty's `xterm-ghostty` terminfo entry.
//!
//! The terminfo source and its compiled database are generated from the
//! pinned Ghostty source by `scripts/sync.sh` (with `tic -x`) and embedded
//! in this crate. [`dir`] writes the database to a cache directory once so
//! a child process can find it through `TERMINFO`.
//!
//! ```no_run
//! let terminfo = ghostty_vt::terminfo::dir().unwrap();
//! let shell = std::process::Command::new("sh")
//!     .env("TERM", ghostty_vt::terminfo::TERM)
//!     .env("TERMINFO", &terminfo)
//!     .spawn();
//! ```

use std::io;
use std::path::PathBuf;

use crate::resources::{File, install};

/// The `TERM` value for Ghostty's terminfo entry.
pub const TERM: &str = "xterm-ghostty";

/// The terminfo source, as `infocmp` would print it.
pub const SOURCE: &str = include_str!("../resources/terminfo/ghostty.terminfo");

const XTERM_GHOSTTY: &[u8] = include_bytes!("../resources/terminfo/78/xterm-ghostty");
const GHOSTTY: &[u8] = include_bytes!("../resources/terminfo/67/ghostty");

// ncurses looks the entry up under a single-letter directory on most
// systems and under a hex directory on macOS, so both are written.
const FILES: &[File] = &[
    File {
        path: "ghostty.terminfo",
        bytes: SOURCE.as_bytes(),
    },
    File {
        path: "x/xterm-ghostty",
        bytes: XTERM_GHOSTTY,
    },
    File {
        path: "78/xterm-ghostty",
        bytes: XTERM_GHOSTTY,
    },
    File {
        path: "g/ghostty",
        bytes: GHOSTTY,
    },
    File {
        path: "67/ghostty",
        bytes: GHOSTTY,
    },
];

/// The directory holding the compiled `xterm-ghostty` terminfo database.
///
/// The database is written once per crate version under the platform cache
/// directory (or `GHOSTTY_VT_CACHE_DIR`). Point `TERMINFO` at the returned
/// path, or append it to `TERMINFO_DIRS`.
pub fn dir() -> io::Result<PathBuf> {
    install("terminfo", FILES)
}

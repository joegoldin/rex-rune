//! Ghostty's shell integration scripts.
//!
//! The scripts for bash, zsh, fish, elvish and nushell are copied from the
//! pinned Ghostty source by `scripts/sync.sh` and embedded in this crate.
//! [`resources_dir`] writes them to a cache directory once, laid out the way
//! Ghostty's `GHOSTTY_RESOURCES_DIR` expects: a `shell-integration`
//! directory with one subdirectory per shell.
//!
//! How each shell picks the scripts up is documented in Ghostty's
//! `src/shell-integration/README.md`: bash through `ENV` in POSIX mode, zsh
//! through `ZDOTDIR`, fish and elvish through `XDG_DATA_DIRS`, and nushell
//! through its vendor autoload directory.

use std::io;
use std::path::PathBuf;

use crate::resources::{File, install};

macro_rules! script {
    ($path:literal) => {
        File {
            path: concat!("shell-integration/", $path),
            bytes: include_bytes!(concat!("../resources/shell-integration/", $path)),
        }
    };
}

const FILES: &[File] = &[
    script!("bash/bash-preexec.sh"),
    script!("bash/ghostty.bash"),
    script!("elvish/lib/ghostty-integration.elv"),
    script!("fish/vendor_conf.d/ghostty-shell-integration.fish"),
    script!("nushell/vendor/autoload/ghostty.nu"),
    script!("zsh/.zshenv"),
    script!("zsh/ghostty-integration"),
];

/// The directory to use as `GHOSTTY_RESOURCES_DIR`.
///
/// It contains `shell-integration/<shell>/...`. The scripts are written
/// once per crate version under the platform cache directory (or
/// `GHOSTTY_VT_CACHE_DIR`).
pub fn resources_dir() -> io::Result<PathBuf> {
    install("resources", FILES)
}

/// The shells with an integration script.
pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "elvish", "nushell"];

//! Write embedded resources to a per-version cache directory once.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A file to install: its path relative to the resource directory and its
/// bytes.
pub(crate) struct File {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

/// The cache root for this crate version and Ghostty pin.
///
/// Honors `GHOSTTY_VT_CACHE_DIR`, then the platform cache directory.
pub(crate) fn root() -> io::Result<PathBuf> {
    let base = match std::env::var_os("GHOSTTY_VT_CACHE_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::cache_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no cache directory"))?
            .join("ghostty-vt"),
    };
    let commit = &crate::ffi::GHOSTTY_COMMIT[..7.min(crate::ffi::GHOSTTY_COMMIT.len())];
    Ok(base.join(format!("{}-{commit}", env!("CARGO_PKG_VERSION"))))
}

/// Install `files` under `root/subdir` once and return that directory.
///
/// The files are written to a temporary sibling and renamed into place, so
/// a concurrent process or thread sees either nothing or the complete set.
/// Each call stages into its own sibling, named by the process and a
/// counter, so two threads of one process never share one. A marker file
/// records completion.
pub(crate) fn install(subdir: &str, files: &[File]) -> io::Result<PathBuf> {
    install_in(&root()?, subdir, files)
}

/// [`install`] under an explicit root.
fn install_in(root: &std::path::Path, subdir: &str, files: &[File]) -> io::Result<PathBuf> {
    let target = root.join(subdir);
    if target.join(".complete").exists() {
        return Ok(target);
    }
    fs::create_dir_all(root)?;
    static STAGING: AtomicU64 = AtomicU64::new(0);
    let staging = root.join(format!(
        ".{subdir}-{}-{}",
        std::process::id(),
        STAGING.fetch_add(1, Ordering::Relaxed)
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for file in files {
        let path = staging.join(file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, file.bytes)?;
    }
    fs::write(staging.join(".complete"), b"")?;
    match fs::rename(&staging, &target) {
        Ok(()) => {}
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            if !target.join(".complete").exists() {
                return Err(error);
            }
        }
    }
    Ok(target)
}

/// Whether a directory holds a complete install.
#[cfg(test)]
pub(crate) fn is_complete(dir: &std::path::Path) -> bool {
    dir.join(".complete").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_writes_once() {
        let temp = std::env::temp_dir().join(format!("ghostty-vt-res-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        // SAFETY: Tests in this module run single-threaded per process and
        // no other test reads this variable concurrently.
        unsafe { std::env::set_var("GHOSTTY_VT_CACHE_DIR", &temp) };
        let files = [File {
            path: "a/b.txt",
            bytes: b"hello",
        }];
        let dir = install("unit", &files).unwrap();
        assert!(is_complete(&dir));
        assert_eq!(fs::read(dir.join("a/b.txt")).unwrap(), b"hello");
        let again = install("unit", &files).unwrap();
        assert_eq!(dir, again);
        unsafe { std::env::remove_var("GHOSTTY_VT_CACHE_DIR") };
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn threads_that_install_at_once_all_get_the_complete_set() {
        let root = std::env::temp_dir().join(format!("ghostty-vt-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        static FILES: [File; 2] = [
            File {
                path: "x/1",
                bytes: b"one",
            },
            File {
                path: "x/2",
                bytes: b"two",
            },
        ];
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let root = root.clone();
                std::thread::spawn(move || install_in(&root, "race", &FILES))
            })
            .collect();
        for thread in threads {
            let dir = thread.join().unwrap().unwrap();
            assert_eq!(fs::read(dir.join("x/2")).unwrap(), b"two");
        }
        let leftovers = fs::read_dir(&root).unwrap().count();
        assert_eq!(leftovers, 1, "only the installed directory remains");
        let _ = fs::remove_dir_all(&root);
    }
}

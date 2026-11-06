//! Check that this crate identifies the Ghostty source used by ghostty-vt-sys.

use std::env;

fn main() {
    let commit = env::var("DEP_GHOSTTY_VT_COMMIT").expect("DEP_GHOSTTY_VT_COMMIT");
    let ghostty_version = env::var("DEP_GHOSTTY_VT_VERSION").expect("DEP_GHOSTTY_VT_VERSION");
    let short_commit = commit
        .get(..7)
        .unwrap_or_else(|| panic!("ghostty-vt-sys has an invalid Ghostty commit: {commit}"));
    let (upstream_version, version_commit) =
        ghostty_version.rsplit_once('+').unwrap_or_else(|| {
            panic!("ghostty-vt-sys version must end in the short commit: {ghostty_version}")
        });
    assert_eq!(
        version_commit, short_commit,
        "ghostty-vt-sys version does not match its commit"
    );

    let package_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let metadata = package_version.split_once('+').map(|(_, value)| value);
    let expected = format!("ghostty.{upstream_version}.{short_commit}");
    assert_eq!(
        metadata,
        Some(expected.as_str()),
        "ghostty-vt version must end in +{expected}"
    );
}

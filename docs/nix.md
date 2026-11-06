# Linux Nix packaging

This fork targets Linux. Its flake exposes `packages.<system>.rex-linux` (also `default`), a development shell, formatter and package check for `x86_64-linux` and `aarch64-linux`. An output being exposed does not mean it has been built on that architecture; record actual verification separately.

From a checkout:

```fish
nix build path:.#rex-linux
./result/bin/rex-linux --help
nix develop path:.
cargo test --locked --all-features
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
```

`path:.` includes new local Nix files before they have been committed. Once the packaging is published in your fork, a consumer can pin `github:joegoldin/rex-rune` and install `inputs.rex-rune.packages.${pkgs.stdenv.hostPlatform.system}.default` in its Linux Home Manager configuration. Keep this input's own nixpkgs pin: upstream needs a newer Rust/Zig toolchain than some system configurations provide.

The package installs `rex-linux` and the upstream `rex-probe` / `rex-workspace-probe` diagnostic binaries, plus the desktop entry and inherited Rex icon. It wraps the GUI with Linux graphics/input libraries and font discovery. Run the probes only against disposable servers: they create and remove their own sessions.

The package's check phase runs Rustfmt, Clippy with warnings denied, Rust unit/headless UI tests, and the missing/stale-socket regression that proves an unavailable server does not cause the client to execute `rex`. The icon's separate provenance and licensing notice remains in `resources/icons/README.md`; the application's MIT license does not cover that artwork.

## Reproducibility

- `flake.lock` pins nixpkgs and the Zig overlay. `Cargo.lock` pins Rust dependencies, including Rune's git source; fixed-output hashes protect fetched git/Zig dependencies.
- The Ghostty source archive is pinned to the same `83fb95624b0439ee660c2679b43b38c8b63769cd` revision and SHA256 as `ghostty-vt-sys` 0.8.0's `GHOSTTY.lock`.
- Ghostty's sources and all lazy Zig dependencies are fetched before the main build. Zig 0.17's cached archives are extracted into the directory layout required by `--system`. Cargo's build script receives `GHOSTTY_SOURCE_DIR`, `GHOSTTY_ZIG_SYSTEM_DIR` and `ZIG`, so compiling the application does not require network access inside the build sandbox.
- When updating `ghostty-vt`, update the archive pin and Zig dependency hash together. When updating Rune, review the vendored patches and Cargo git dependency hash together.

## Session safety

Installing or starting this GUI must not start, replace, reconfigure or stop a Rex server. It connects to your existing `REX_SERVER` endpoint or the upstream local socket default. Configure the Rex server separately, as the existing den Rex aspect already does. Tests and probes that create sessions must use disposable servers, never your daily sessions.

Adding the package to den and evaluating it is separate from activating a host configuration. Do not restart existing Herdr/Rex services merely to migrate the GUI launcher.

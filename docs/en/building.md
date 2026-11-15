[← README](../../README.md) · **English** · [Deutsch](../de/building.md)

# Building

Paneldeck is written in Rust (egui/eframe, ConPTY via `portable-pty`, terminal emulation via `alacritty_terminal`).

## On Windows (native, MSVC)
Rust (rustup, toolchain `stable-x86_64-pc-windows-msvc`) and the Visual Studio Build Tools are enough:

    cargo build --release        # -> target\release\paneldeck.exe
    cargo test --release

A self-built `paneldeck.exe` is portable: it creates `paneldeck-data` next to itself (here under `target\release\`). Use `--data-dir` or `--profile` to keep test data apart from your real data (see [Command line](command-line.md)).

## Cross-build on Linux
Possible, but not tested regularly:

    rustup target add x86_64-pc-windows-gnu && apt install gcc-mingw-w64-x86-64
    cargo build --release --target x86_64-pc-windows-gnu

## Release files
Releases are built from a `release/x.y` branch with `tools/release.sh <version>` (see `CLAUDE.md` for the branching and release process) – on Windows from Git Bash; it needs Python 3 and, for the installer, NSIS (`makensis`). It runs the tests, builds the program and the installer, checks both with Microsoft Defender (`tools/defender-scan.ps1`), runs the end-to-end tests and packs three release files:

| File | Contents |
|---|---|
| `Paneldeck-Setup-<version>.exe` | installer (per user, no administrator rights) |
| `paneldeck-<version>-windows-x64.zip` | portable ZIP: folder `paneldeck-<version>/` with `paneldeck.exe`, licenses, README and changelog |
| `SHA256SUMS.txt` | SHA-256 checksums of both files (used by the [self-update](updates.md)) |

`--publish` also pushes the tag and creates the GitHub release (GitHub CLI `gh`, logged in); `--no-git` only builds and packages.

## Tests
- `cargo test --release` – unit tests and headless UI tests.
- `tools/smoke-mcp.ps1` – end-to-end test of the agent interface against a copy of the built `paneldeck.exe` in its own temporary data folder.
- `tools/update-test.ps1` – installs an older version into a temporary folder with a fake user profile and updates it with the new installer.
- `tools/update-feed.sh <version> <folder>` – creates a local fake release (with `latest.json`, ZIP, installer and `SHA256SUMS.txt`) for testing the self-update with a build that has the Cargo feature `update-test-feed` (started with `PANELDECK_UPDATE_TEST_FEED=<folder>`; see [Updates](updates.md#testing-the-self-update-developers)); nothing is published.

The manual test plan is `TESTPLAN.md` (German).

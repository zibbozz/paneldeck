# portable-pty 0.9.0 (vendored, patched for Paneldeck)

Unmodified copy of `portable-pty` 0.9.0 from crates.io (wezterm `pty/`, commit f8921727a11b9f8b073e8c24821d72fd41283500,
MIT, see `LICENSE.md`), used through `[patch.crates-io]` in Paneldeck's `Cargo.toml`, with one change in
`src/win/psuedocon.rs`, `src/win/conpty.rs` and `src/win/mod.rs` (marked "Paneldeck patch"):

- Upstream loads `conpty.dll` once per process by bare name (`LoadLibrary("conpty.dll")`, DLL search order) and uses it
  for every pseudo console. Here `ConPtySystem::with_sideloaded_conpty(Some(path))` loads a bundled `conpty.dll` by
  absolute path; `ConPtySystem::default()` / `native_pty_system()` use the system ConPTY (kernel32).
- If the sideloaded DLL cannot be loaded or `CreatePseudoConsole` fails, the pseudo console is created on kernel32.
- The sideloaded DLL gets only `PSEUDOCONSOLE_INHERIT_CURSOR` (the legacy inbox flags `RESIZE_QUIRK` and
  `WIN32_INPUT_MODE` are not passed to it; kernel32 still gets all three as upstream).
- `ConPtyMasterPty::host()` / `ConPtyHost` report which implementation a pty runs on.

To update: copy the new crate source over this folder, re-apply the marked changes, rebuild.

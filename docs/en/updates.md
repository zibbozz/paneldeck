[← README](../../README.md) · **English** · [Deutsch](../de/updates.md)

# Updates

From 2.1.0 on, Paneldeck can update itself from [GitHub Releases](https://github.com/zibbozz/paneldeck/releases). Nothing is self-hosted: the check and the download both go to GitHub. Only regular releases are offered (no pre-releases), and Paneldeck never downgrades.

## Checking for updates
- *Settings → About → Updates* shows the installed version and whether this copy is *(installed)* or *(portable)*, the button **Check for updates**, the time of the last check and a skipped version (with *Reset*).
- **Check automatically (at start, at most once a day)** – **off by default**; there is no first-run question. After a failed automatic check (e.g. offline) Paneldeck tries again at the earliest an hour later.
- `--no-update-check` suppresses the automatic check for one start (see [Command line](command-line.md#--no-update-check)).
- Agents (MCP) cannot trigger an update.

## When an update is available
A button appears in the title bar, right-aligned directly left of the minimize button (tooltip "Version X is available"; it shows the progress while downloading). A click opens a dialog with:
- the new and the installed version,
- **What's new?** – opens the release page on GitHub in your browser,
- **Update now**, **Later** (hides the dialog and the button; *Details …* in *Settings → About* reopens it) and **Skip this version** (the automatic check no longer offers it; a manual check still shows it, marked "(skipped)").

A jump to a **new major version** (e.g. 2.x → 3.0) shows a clear warning, and a database backup is made automatically before updating.

## Installing
1. **Download** in the background, then a check against `SHA256SUMS.txt` (see below). The download can be cancelled.
2. **Restart confirmation:** Paneldeck restarts to finish the update. If terminals or Claude sessions are running, it says how many; their processes end, and the sessions are restored after the restart. *Not now* leaves a *Restart* button in the title bar.
3. **Install:**
   - **Installed copies** download `Paneldeck-Setup-<version>.exe` and run it silently into the same folder. Your choice of desktop shortcut and context menu is kept.
   - **Portable copies** download `paneldeck-<version>-windows-x64.zip`, replace `paneldeck.exe` (the old one is kept as `paneldeck.exe.old` and removed after the new version has started) and restart.
4. The new version starts with the **same parameters** (data folder, profile) and restores your sessions.

A copy counts as *installed* when the installer's uninstall entry points to exactly its folder; every other copy (also a portable copy next to an installation) is *portable*.

## Integrity
- Every release from 2.1.0 on has an additional asset **`SHA256SUMS.txt`** with the SHA-256 checksums of the installer and the ZIP (format of `sha256sum`). The download is checked against it and **discarded on a mismatch** ("Checksum mismatch – the download was discarded."), and checked again right before installing.
- Releases **without** `SHA256SUMS.txt` (2.0.0 and older) are only announced, never installed automatically.
- Downloads only go over HTTPS to GitHub hosts (`api.github.com`, `github.com` and GitHub's download servers) and only for this repository.
- Without code signing (see the [code signing policy](../../README.md#code-signing-policy)) this protects against corrupted or incomplete downloads, **not** against a compromised GitHub account.

You can check a manually downloaded file yourself, e.g. in PowerShell: `Get-FileHash .\Paneldeck-Setup-2.1.0.exe` and compare with the line in `SHA256SUMS.txt`; in Git Bash: `sha256sum -c SHA256SUMS.txt`.

## Privacy
The check asks the public GitHub API for the latest release – without a login, and only when you click *Check for updates* or have switched on the automatic check. GitHub sees your IP address and the Paneldeck version (`User-Agent: Paneldeck/<version>`); no other data is sent. See [Privacy](../../README.md#privacy).

## Updating by hand
You can always update without the built-in update: run the new installer over the old version (also while Paneldeck runs), or replace `paneldeck.exe` of a portable copy. Settings, sessions and data stay. The self-update is first usable when updating **from** 2.1.0 to a later version.

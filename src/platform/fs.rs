//! File system details that differ between systems.

/// Open the file so that others may only read it while the handle is open (no writing, renaming or deleting).
/// Windows: share mode `FILE_SHARE_READ`. Elsewhere there are no mandatory share modes; nothing changes.
pub fn deny_write_share(o: &mut std::fs::OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        o.share_mode(FILE_SHARE_READ);
    }
    #[cfg(not(windows))]
    let _ = o;
}

/// Is the entry a reparse point (junction, mount point, symbolic link of any kind)? Symbolic links in the sense of
/// `FileType::is_symlink` are checked by the caller; this adds the Windows-only kinds. Always `false` elsewhere.
pub fn is_reparse_point(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        let _ = m;
        false
    }
}

/// Does this platform have drive letters (`C:` …)? Only then `drive_type` answers.
pub const DRIVE_LETTERS: bool = cfg!(windows);

/// Type of the drive with this letter (`GetDriveTypeW` of `<letter>:\\`: 2 = removable, 3 = fixed, 4 = network, 5 = CD,
/// 6 = RAM disk, 0/1 = unknown/no drive). The query itself does not touch the network. `None` without drive letters.
pub fn drive_type(letter: char) -> Option<u32> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDriveTypeW(root: *const u16) -> u32;
        }
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: nullterminierter Pfad, der während des Aufrufs lebt.
        Some(unsafe { GetDriveTypeW(root.as_ptr()) })
    }
    #[cfg(not(windows))]
    {
        let _ = letter;
        None
    }
}

/// Replace `target` by `replacement` (same folder) in one step, for atomic saving. Windows: `ReplaceFileW` when the
/// target exists (keeps attributes such as hidden, the ACL and the creation time of the target), else a move.
/// Elsewhere: `rename` (atomic on the same file system; the caller copies the permissions beforehand).
pub fn replace_file(replacement: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn ReplaceFileW(
                replaced: *const u16,
                replacement: *const u16,
                backup: *const u16,
                flags: u32,
                exclude: *mut core::ffi::c_void,
                reserved: *mut core::ffi::c_void,
            ) -> i32;
        }
        const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;
        if target.exists() {
            let w = |p: &std::path::Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() };
            let (t, r) = (w(target), w(replacement));
            // SAFETY: beide Pfade sind nullterminiert und leben während des Aufrufs; keine Sicherung, keine Reserven.
            let ok = unsafe {
                ReplaceFileW(t.as_ptr(), r.as_ptr(), std::ptr::null(), REPLACEFILE_IGNORE_MERGE_ERRORS, std::ptr::null_mut(), std::ptr::null_mut())
            };
            if ok != 0 {
                return Ok(());
            }
            // Fällt z. B. auf FAT-Laufwerken oder bei fehlenden Rechten auf die Attribute aus: normales Umbenennen.
        }
        std::fs::rename(replacement, target)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(replacement, target)
    }
}

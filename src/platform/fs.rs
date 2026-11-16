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

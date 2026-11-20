//! Minimaler ZIP-Leser: genau eine Datei aus dem portablen Release-ZIP holen (gespeichert oder Deflate), mit CRC-32-Prüfung;
//! für macOS (#150) den ganzen Ordner `Paneldeck.app/` mit Unix-Rechten ([`extract_tree`]).
//! Kein ZIP64, keine Verschlüsselung – beides kommt in unseren Release-Dateien nicht vor und wird abgelehnt.

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8))
}

/// Inhalt der Datei `name` (exakter Pfad im Archiv, mit `/`), höchstens `max` Bytes entpackt.
pub fn extract(zip: &[u8], name: &str, max: usize) -> Result<Vec<u8>, String> {
    // Ende des zentralen Verzeichnisses: Signatur in den letzten 64 KiB + 22 Bytes suchen (Kommentar max. 65535).
    let lo = zip.len().saturating_sub(22 + 0xffff);
    let eocd = (lo..=zip.len().saturating_sub(22)).rev().find(|&i| u32_at(zip, i) == Some(0x0605_4b50)).ok_or("zip: no end of central directory")?;
    let count = u16_at(zip, eocd + 10).ok_or("zip")? as usize;
    let cd_size = u32_at(zip, eocd + 12).ok_or("zip")? as usize;
    let cd_off = u32_at(zip, eocd + 16).ok_or("zip")? as usize;
    if count == 0xffff || cd_off == 0xffff_ffff || cd_off.checked_add(cd_size).is_none_or(|e| e > eocd) {
        return Err("zip: unsupported (zip64) or damaged".into());
    }
    let mut p = cd_off;
    for _ in 0..count {
        if u32_at(zip, p) != Some(0x0201_4b50) {
            return Err("zip: damaged central directory".into());
        }
        let flags = u16_at(zip, p + 8).ok_or("zip")?;
        let method = u16_at(zip, p + 10).ok_or("zip")?;
        let crc = u32_at(zip, p + 16).ok_or("zip")?;
        let csize = u32_at(zip, p + 20).ok_or("zip")? as usize;
        let usize_ = u32_at(zip, p + 24).ok_or("zip")? as usize;
        let nlen = u16_at(zip, p + 28).ok_or("zip")? as usize;
        let xlen = u16_at(zip, p + 30).ok_or("zip")? as usize;
        let clen = u16_at(zip, p + 32).ok_or("zip")? as usize;
        let local = u32_at(zip, p + 42).ok_or("zip")? as usize;
        let entry = zip.get(p + 46..p + 46 + nlen).ok_or("zip")?;
        p += 46 + nlen + xlen + clen;
        if entry != name.as_bytes() {
            continue;
        }
        if flags & 1 != 0 {
            return Err("zip: encrypted".into());
        }
        if usize_ > max || csize == 0xffff_ffff {
            return Err("zip: entry too large".into());
        }
        if u32_at(zip, local) != Some(0x0403_4b50) {
            return Err("zip: damaged local header".into());
        }
        let start = local + 30 + u16_at(zip, local + 26).ok_or("zip")? as usize + u16_at(zip, local + 28).ok_or("zip")? as usize;
        let data = zip.get(start..start.checked_add(csize).ok_or("zip")?).ok_or("zip: truncated")?;
        let out = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, max).map_err(|e| format!("zip: deflate {:?}", e.status))?,
            m => return Err(format!("zip: compression method {m}")),
        };
        if out.len() != usize_ || crc32(&out) != crc {
            return Err("zip: checksum mismatch".into());
        }
        return Ok(out);
    }
    Err(format!("zip: {name} not found"))
}

/// Ein Eintrag aus [`extract_tree`]: Pfad unterhalb des Präfixes (mit `/`), Inhalt, Unix-Rechte (nur `0o777`-Bits) bzw.
/// Ordner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
    pub dir: bool,
}

/// Ist `rel` ein sicherer relativer Pfad (keine leeren Teile, kein `.`/`..`, kein `\`, kein `:`/NUL/Steuerzeichen)?
pub fn safe_rel(rel: &str) -> bool {
    !rel.is_empty() && !rel.starts_with('/') && rel.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.chars().any(|ch| ch == '\\' || ch == ':' || ch.is_control()))
}

/// macOS (#150): alle Einträge unter `prefix` (z. B. `Paneldeck.app/`) mit CRC-Prüfung, höchstens `max_total` Bytes
/// entpackt und höchstens 4096 Einträge. Abgelehnt werden: Symlinks und andere Sonderdateien, unsichere Pfade
/// ([`safe_rel`]), doppelte Pfade, Verschlüsselung, ZIP64. `__MACOSX/` (Ressourcen von `ditto`) wird übergangen, alles
/// andere außerhalb von `prefix` ebenso. Rechte: aus den Unix-Attributen (Ersteller-System 3), sonst 0o644 bzw. 0o755
/// für Ordner und `Contents/MacOS/*`.
pub fn extract_tree(zip: &[u8], prefix: &str, max_total: usize) -> Result<Vec<Entry>, String> {
    let lo = zip.len().saturating_sub(22 + 0xffff);
    let eocd = (lo..=zip.len().saturating_sub(22)).rev().find(|&i| u32_at(zip, i) == Some(0x0605_4b50)).ok_or("zip: no end of central directory")?;
    let count = u16_at(zip, eocd + 10).ok_or("zip")? as usize;
    let cd_size = u32_at(zip, eocd + 12).ok_or("zip")? as usize;
    let cd_off = u32_at(zip, eocd + 16).ok_or("zip")? as usize;
    if count == 0xffff || cd_off == 0xffff_ffff || cd_off.checked_add(cd_size).is_none_or(|e| e > eocd) {
        return Err("zip: unsupported (zip64) or damaged".into());
    }
    if count > 4096 {
        return Err("zip: too many entries".into());
    }
    let mut out: Vec<Entry> = Vec::new();
    let mut total = 0usize;
    let mut p = cd_off;
    for _ in 0..count {
        if u32_at(zip, p) != Some(0x0201_4b50) {
            return Err("zip: damaged central directory".into());
        }
        let made_by = u16_at(zip, p + 4).ok_or("zip")?;
        let flags = u16_at(zip, p + 8).ok_or("zip")?;
        let method = u16_at(zip, p + 10).ok_or("zip")?;
        let crc = u32_at(zip, p + 16).ok_or("zip")?;
        let csize = u32_at(zip, p + 20).ok_or("zip")? as usize;
        let usize_ = u32_at(zip, p + 24).ok_or("zip")? as usize;
        let nlen = u16_at(zip, p + 28).ok_or("zip")? as usize;
        let xlen = u16_at(zip, p + 30).ok_or("zip")? as usize;
        let clen = u16_at(zip, p + 32).ok_or("zip")? as usize;
        let ext = u32_at(zip, p + 38).ok_or("zip")?;
        let local = u32_at(zip, p + 42).ok_or("zip")? as usize;
        let raw = zip.get(p + 46..p + 46 + nlen).ok_or("zip")?;
        p += 46 + nlen + xlen + clen;
        let name = std::str::from_utf8(raw).map_err(|_| "zip: name is not UTF-8")?;
        if name.starts_with("__MACOSX/") {
            continue;
        }
        let Some(rel) = name.strip_prefix(prefix) else { continue };
        if rel.is_empty() {
            continue; // der Präfix-Ordner selbst
        }
        let dir = rel.ends_with('/');
        let rel = rel.trim_end_matches('/');
        if !safe_rel(rel) {
            return Err(format!("zip: unsafe path {name:?}"));
        }
        let unix = made_by >> 8 == 3;
        let kind = if unix { (ext >> 16) & 0o170000 } else { 0 };
        if unix && kind != 0 && kind != 0o100000 && kind != 0o040000 {
            return Err(format!("zip: {name}: symlink or special file"));
        }
        let dir = dir || kind == 0o040000;
        let mode = if unix && (ext >> 16) & 0o777 != 0 {
            (ext >> 16) & 0o777
        } else if dir || rel.starts_with("Contents/MacOS/") {
            0o755
        } else {
            0o644
        };
        if out.iter().any(|e| e.path == rel) {
            return Err(format!("zip: duplicate {name}"));
        }
        if flags & 1 != 0 {
            return Err("zip: encrypted".into());
        }
        if dir {
            out.push(Entry { path: rel.to_string(), data: Vec::new(), mode, dir: true });
            continue;
        }
        if csize == 0xffff_ffff || usize_ > max_total.saturating_sub(total) {
            return Err("zip: content too large".into());
        }
        if u32_at(zip, local) != Some(0x0403_4b50) {
            return Err("zip: damaged local header".into());
        }
        let start = local + 30 + u16_at(zip, local + 26).ok_or("zip")? as usize + u16_at(zip, local + 28).ok_or("zip")? as usize;
        let data = zip.get(start..start.checked_add(csize).ok_or("zip")?).ok_or("zip: truncated")?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, usize_.max(1)).map_err(|e| format!("zip: deflate {:?}", e.status))?,
            m => return Err(format!("zip: compression method {m}")),
        };
        if bytes.len() != usize_ || crc32(&bytes) != crc {
            return Err("zip: checksum mismatch".into());
        }
        total += bytes.len();
        out.push(Entry { path: rel.to_string(), data: bytes, mode, dir: false });
    }
    if out.is_empty() {
        return Err(format!("zip: {prefix} not found"));
    }
    Ok(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Kleiner ZIP-Schreiber für Tests (gespeichert oder Deflate).
    pub fn make_zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let v: Vec<(&str, &[u8], bool, Option<u32>)> = files.iter().map(|(n, d, z)| (*n, *d, *z, None)).collect();
        make_zip_unix(&v)
    }

    /// Wie [`make_zip`], mit Unix-Modus (`st_mode`, z. B. `0o100755`) je Eintrag wie von `ditto`/`zip` unter macOS.
    pub fn make_zip_unix(files: &[(&str, &[u8], bool, Option<u32>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data, deflate, unix_mode) in files {
            let comp = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
            let off = out.len() as u32;
            let crc = crc32(data);
            let method: u16 = if *deflate { 8 } else { 0 };
            let mut h = Vec::new();
            h.extend(0x0403_4b50u32.to_le_bytes());
            h.extend(20u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(method.to_le_bytes());
            h.extend([0u8; 4]);
            h.extend(crc.to_le_bytes());
            h.extend((comp.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(3u16.to_le_bytes());
            h.extend(name.as_bytes());
            h.extend([1, 2, 3]); // Extra-Feld
            out.extend(&h);
            out.extend(&comp);
            cd.extend(0x0201_4b50u32.to_le_bytes());
            cd.extend((if unix_mode.is_some() { 0x0314u16 } else { 20u16 }).to_le_bytes());
            cd.extend(20u16.to_le_bytes());
            cd.extend(0u16.to_le_bytes());
            cd.extend(method.to_le_bytes());
            cd.extend([0u8; 4]);
            cd.extend(crc.to_le_bytes());
            cd.extend((comp.len() as u32).to_le_bytes());
            cd.extend((data.len() as u32).to_le_bytes());
            cd.extend((name.len() as u16).to_le_bytes());
            cd.extend(0u16.to_le_bytes());
            cd.extend(0u16.to_le_bytes());
            cd.extend([0u8; 4]);
            cd.extend((unix_mode.unwrap_or(0) << 16).to_le_bytes());
            cd.extend(off.to_le_bytes());
            cd.extend(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend(&cd);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((cd.len() as u32).to_le_bytes());
        out.extend(cd_off.to_le_bytes());
        out.extend(4u16.to_le_bytes());
        out.extend(b"note");
        out
    }

    #[test]
    fn crc_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn extracts_stored_and_deflate() {
        let exe: Vec<u8> = b"MZ".iter().copied().chain((0..50_000u32).map(|i| (i % 251) as u8)).collect();
        let z = make_zip(&[("paneldeck-2.1.0/LICENSE", b"MIT", false), ("paneldeck-2.1.0/paneldeck.exe", &exe, true), ("paneldeck-2.1.0/README.md", b"readme", false)]);
        assert_eq!(extract(&z, "paneldeck-2.1.0/paneldeck.exe", 1 << 20).unwrap(), exe);
        assert_eq!(extract(&z, "paneldeck-2.1.0/LICENSE", 100).unwrap(), b"MIT");
        assert!(extract(&z, "paneldeck.exe", 1 << 20).is_err(), "nur der exakte Pfad");
        assert!(extract(&z, "paneldeck-2.1.0/paneldeck.exe", 1000).is_err(), "Größenlimit");
    }

    #[test]
    fn extracts_app_bundle_tree() {
        let bin: Vec<u8> = (0..40_000u32).map(|i| (i % 253) as u8).collect();
        let z = make_zip_unix(&[
            ("Paneldeck.app/", b"", false, Some(0o040755)),
            ("Paneldeck.app/Contents/", b"", false, Some(0o040755)),
            ("Paneldeck.app/Contents/Info.plist", b"<plist/>", true, Some(0o100644)),
            ("Paneldeck.app/Contents/MacOS/paneldeck", &bin, true, Some(0o100755)),
            ("__MACOSX/Paneldeck.app/._Info.plist", b"rsrc", false, Some(0o100644)),
            ("Other/readme.txt", b"x", false, None),
        ]);
        let t = extract_tree(&z, "Paneldeck.app/", 1 << 20).unwrap();
        let paths: Vec<&str> = t.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["Contents", "Contents/Info.plist", "Contents/MacOS/paneldeck"]);
        assert!(t[0].dir && t[0].mode == 0o755);
        assert_eq!((t[2].mode, t[2].data.len()), (0o755, bin.len()));
        assert_eq!(t[1].mode, 0o644);
        assert!(extract_tree(&z, "Paneldeck.app/", 1000).is_err(), "size limit");
        // without Unix attributes: default modes
        let plain = make_zip(&[("Paneldeck.app/Contents/MacOS/paneldeck", b"bin", false), ("Paneldeck.app/Contents/Info.plist", b"p", false)]);
        let t = extract_tree(&plain, "Paneldeck.app/", 100).unwrap();
        assert_eq!((t[0].mode, t[1].mode), (0o755, 0o644));
        // refused: symlinks, path tricks, duplicates, nothing found
        let link = make_zip_unix(&[("Paneldeck.app/Contents/x", b"/etc/passwd", false, Some(0o120777))]);
        assert!(extract_tree(&link, "Paneldeck.app/", 100).unwrap_err().contains("symlink"));
        for bad in ["Paneldeck.app/../evil", "Paneldeck.app/Contents//x", "Paneldeck.app/a\\b", "Paneldeck.app/./x"] {
            assert!(extract_tree(&make_zip(&[(bad, b"x", false)]), "Paneldeck.app/", 100).is_err(), "{bad}");
        }
        assert!(extract_tree(&make_zip(&[("Paneldeck.app/a", b"x", false), ("Paneldeck.app/a", b"y", false)]), "Paneldeck.app/", 100).is_err());
        assert!(extract_tree(&make_zip(&[("x/a", b"x", false)]), "Paneldeck.app/", 100).is_err());
        assert!(safe_rel("Contents/MacOS/paneldeck") && !safe_rel("/abs") && !safe_rel("a/../b") && !safe_rel(""));
    }

    #[test]
    fn rejects_damage() {
        let z = make_zip(&[("a/paneldeck.exe", b"MZ hello hello hello", false)]);
        let mut bad = z.clone();
        let i = bad.windows(5).position(|w| w == b"MZ he").unwrap();
        bad[i + 3] ^= 0xff; // Datenbyte kippen → CRC passt nicht
        assert_eq!(extract(&bad, "a/paneldeck.exe", 100).unwrap_err(), "zip: checksum mismatch");
        assert!(extract(&z[..z.len() / 2], "a/paneldeck.exe", 100).is_err());
        assert!(extract(b"", "a", 10).is_err());
        assert!(extract(b"PK\x05\x06garbage", "a", 10).is_err());
    }

    /// Ein echtes ZIP von Pythons `zipfile` (wie `tools/package.sh`), wenn verfügbar – sonst übersprungen.
    #[test]
    fn reads_python_zipfile_if_available() {
        let d = std::env::temp_dir().join(format!("paneldeck-test-pyzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let script = "import zipfile,sys\nz=zipfile.ZipFile(sys.argv[1],'w',zipfile.ZIP_DEFLATED,compresslevel=9)\nz.writestr('paneldeck-1.2.3/paneldeck.exe', b'MZ'+bytes(range(256))*400)\nz.writestr('paneldeck-1.2.3/LICENSE','MIT')\nz.close()\n";
        let zp = d.join("t.zip");
        let ok = std::process::Command::new("python").args(["-I", "-c", script]).arg(&zp).output().map(|o| o.status.success()).unwrap_or(false);
        if ok {
            let z = std::fs::read(&zp).unwrap();
            let exe = extract(&z, "paneldeck-1.2.3/paneldeck.exe", 1 << 20).unwrap();
            assert_eq!(exe.len(), 2 + 256 * 400);
            assert_eq!(&exe[..2], b"MZ");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

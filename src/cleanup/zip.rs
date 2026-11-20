//! ZIP-Sicherung des Datenordners vor „Paneldeck komplett zurücksetzen“ (#307). Minimaler Schreiber (Deflate über
//! `miniz_oxide`, UTF-8-Namen, kein ZIP64 – über 4 GB bzw. 65535 Einträge wird abgelehnt).
//!
//! Ohne Zugangsdaten: In `settings.json` fallen die gespeicherten API-Schlüssel der Modell-Server weg (sie liegen ohnehin
//! nur verschlüsselt für diesen Windows-Benutzer dort). Ausgelassen werden außerdem Sperre, IPC-Datei (Token), andere
//! Profile und die Vormerkung; die Datenbank kommt als konsistente Kopie (`VACUUM INTO`) hinein.

use super::FileEntry;
use std::io::Write;
use std::path::Path;

/// Eintrag des zentralen Verzeichnisses.
struct Central {
    name: Vec<u8>,
    crc: u32,
    comp: u32,
    size: u32,
    offset: u32,
    method: u16,
}

fn dos_time() -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let n = chrono::Local::now().naive_local();
    let time = ((n.hour() as u16) << 11) | ((n.minute() as u16) << 5) | (n.second() as u16 / 2);
    let date = (((n.year().clamp(1980, 2107) - 1980) as u16) << 9) | ((n.month() as u16) << 5) | n.day() as u16;
    (time, date)
}

/// `settings.json` ohne API-Schlüssel (`llm_servers[].api_key`). Unlesbares JSON bleibt draußen (`None`).
pub fn strip_credentials(settings: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(settings).ok()?;
    let mut v: serde_json::Value = serde_json::from_str(crate::state::strip_bom(text)).ok()?;
    if let Some(list) = v.get_mut("llm_servers").and_then(|l| l.as_array_mut()) {
        for s in list {
            if let Some(o) = s.as_object_mut() {
                o.remove("api_key");
            }
        }
    }
    serde_json::to_vec_pretty(&v).ok()
}

/// Schreibt die ZIP-Datei `out` aus (Name im Archiv, Inhalt). Gibt (Bytes vor dem Packen, Dateien) zurück.
pub fn write(out: &Path, files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Result<(u64, u64), String> {
    let err = |e: std::io::Error| format!("{}: {e}", out.display());
    let tmp = out.with_extension("zip.part");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(err)?);
    let (time, date) = dos_time();
    let mut central: Vec<Central> = Vec::new();
    let (mut offset, mut raw) = (0u64, 0u64);
    let res = (|| -> Result<(), String> {
        for (name, data) in files {
            let packed = miniz_oxide::deflate::compress_to_vec(&data, 6);
            let (method, body): (u16, &[u8]) = if packed.len() < data.len() { (8, &packed) } else { (0, &data) };
            let name_b = name.replace('\\', "/").into_bytes();
            if offset + body.len() as u64 + 100 > u32::MAX as u64 || data.len() as u64 > u32::MAX as u64 || central.len() >= 65535 {
                return Err(crate::state::t("Zu groß für eine ZIP-Datei (über 4 GB oder 65535 Dateien).", "Too large for a ZIP file (over 4 GB or 65535 files).").into());
            }
            let crc = crate::app::update::zip::crc32(&data);
            let mut h = Vec::with_capacity(30 + name_b.len());
            h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            h.extend_from_slice(&20u16.to_le_bytes()); // Version
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8-Namen
            h.extend_from_slice(&method.to_le_bytes());
            h.extend_from_slice(&time.to_le_bytes());
            h.extend_from_slice(&date.to_le_bytes());
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&name_b);
            f.write_all(&h).map_err(err)?;
            f.write_all(body).map_err(err)?;
            central.push(Central { name: name_b, crc, comp: body.len() as u32, size: data.len() as u32, offset: offset as u32, method });
            offset += (h.len() + body.len()) as u64;
            raw += data.len() as u64;
        }
        let cd_start = offset;
        for c in &central {
            let mut h = Vec::with_capacity(46 + c.name.len());
            h.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0x0800u16.to_le_bytes());
            h.extend_from_slice(&c.method.to_le_bytes());
            h.extend_from_slice(&time.to_le_bytes());
            h.extend_from_slice(&date.to_le_bytes());
            h.extend_from_slice(&c.crc.to_le_bytes());
            h.extend_from_slice(&c.comp.to_le_bytes());
            h.extend_from_slice(&c.size.to_le_bytes());
            h.extend_from_slice(&(c.name.len() as u16).to_le_bytes());
            h.extend_from_slice(&[0u8; 8]); // Extra, Kommentar, Disk, interne Attribute
            h.extend_from_slice(&0u32.to_le_bytes()); // externe Attribute
            h.extend_from_slice(&c.offset.to_le_bytes());
            h.extend_from_slice(&c.name);
            f.write_all(&h).map_err(err)?;
            offset += h.len() as u64;
        }
        if offset > u32::MAX as u64 {
            return Err(crate::state::t("Zu groß für eine ZIP-Datei (über 4 GB).", "Too large for a ZIP file (over 4 GB).").into());
        }
        let mut e = Vec::with_capacity(22);
        e.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        e.extend_from_slice(&[0u8; 4]);
        e.extend_from_slice(&(central.len() as u16).to_le_bytes());
        e.extend_from_slice(&(central.len() as u16).to_le_bytes());
        e.extend_from_slice(&((offset - cd_start) as u32).to_le_bytes());
        e.extend_from_slice(&(cd_start as u32).to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        f.write_all(&e).map_err(err)?;
        f.flush().map_err(err)
    })();
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", out.display())
    })?;
    Ok((raw, central.len() as u64))
}

/// Inhalt der Sicherung eines Datenordners: alle Dateien dieser Instanz (siehe Moduldoku), `settings.json` ohne Schlüssel,
/// die Datenbank als Kopie `db_copy` (fehlt sie, kommt keine Datenbank hinein). Unlesbare Dateien sind ein Fehler.
pub fn data_folder_entries(data_dir: &Path, db_copy: Option<&Path>) -> Result<Vec<(String, Vec<u8>)>, String> {
    let files: Vec<FileEntry> = super::data_files(data_dir);
    let db = crate::db::FILE;
    let mut out = Vec::new();
    for f in files {
        let Ok(rel) = f.path.strip_prefix(data_dir) else { continue };
        let name = rel.to_string_lossy().replace('\\', "/");
        if name == db || name == format!("{db}-wal") || name == format!("{db}-shm") || name.ends_with(".tmp") {
            continue;
        }
        let data = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path.display()))?;
        if name == "settings.json" {
            if let Some(clean) = strip_credentials(&data) {
                out.push((name, clean));
            }
            continue;
        }
        out.push((name, data));
    }
    if let Some(c) = db_copy {
        out.push((db.to_string(), std::fs::read(c).map_err(|e| format!("{}: {e}", c.display()))?));
    }
    Ok(out)
}

//! Text encodings and line endings: detection on open, decoding, and streaming encoding on save.

use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// Encodings the editor can read and write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextEncoding {
    #[default]
    Utf8,
    /// UTF-8 with byte order mark.
    Utf8Bom,
    /// UTF-16 little endian (written with BOM).
    Utf16Le,
    /// UTF-16 big endian (written with BOM).
    Utf16Be,
    /// Windows-1252 (Western European; superset of Latin-1 for printable chars).
    Windows1252,
}

impl TextEncoding {
    pub const ALL: [TextEncoding; 5] =
        [TextEncoding::Utf8, TextEncoding::Utf8Bom, TextEncoding::Utf16Le, TextEncoding::Utf16Be, TextEncoding::Windows1252];

    /// Short label for the status bar (not translated; these are technical names).
    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf8Bom => "UTF-8 with BOM",
            TextEncoding::Utf16Le => "UTF-16 LE",
            TextEncoding::Utf16Be => "UTF-16 BE",
            TextEncoding::Windows1252 => "Windows-1252",
        }
    }

    /// Stable id (as in `.editorconfig` where one exists): `utf-8`, `utf-8-bom`, `utf-16le`, `utf-16be`, `windows-1252`.
    pub fn id(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "utf-8",
            TextEncoding::Utf8Bom => "utf-8-bom",
            TextEncoding::Utf16Le => "utf-16le",
            TextEncoding::Utf16Be => "utf-16be",
            TextEncoding::Windows1252 => "windows-1252",
        }
    }

    /// Parse an id (also `.editorconfig` charsets: `latin1` → Windows-1252).
    pub fn from_id(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => TextEncoding::Utf8,
            "utf-8-bom" | "utf8bom" | "utf-8bom" => TextEncoding::Utf8Bom,
            "utf-16le" | "utf16le" => TextEncoding::Utf16Le,
            "utf-16be" | "utf16be" => TextEncoding::Utf16Be,
            "windows-1252" | "cp1252" | "latin1" | "iso-8859-1" => TextEncoding::Windows1252,
            _ => return None,
        })
    }

    fn bom(self) -> &'static [u8] {
        match self {
            TextEncoding::Utf8Bom => &[0xEF, 0xBB, 0xBF],
            TextEncoding::Utf16Le => &[0xFF, 0xFE],
            TextEncoding::Utf16Be => &[0xFE, 0xFF],
            _ => &[],
        }
    }

    /// Can every char be written in this encoding? (Only Windows-1252 is limited.)
    pub fn is_unicode(self) -> bool {
        self != TextEncoding::Windows1252
    }
}

/// Line endings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    /// Old Mac style; read and kept, rare.
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }

    /// Platform default for new files.
    pub fn native() -> Self {
        if cfg!(windows) { LineEnding::CrLf } else { LineEnding::Lf }
    }

    /// Parse `lf`/`crlf`/`cr` (as in `.editorconfig`).
    pub fn from_id(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "lf" => LineEnding::Lf,
            "crlf" => LineEnding::CrLf,
            "cr" => LineEnding::Cr,
            _ => return None,
        })
    }
}

/// Line endings found in a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EolStats {
    pub lf: usize,
    pub crlf: usize,
    pub cr: usize,
}

impl EolStats {
    /// More than one kind present.
    pub fn is_mixed(&self) -> bool {
        [self.lf, self.crlf, self.cr].iter().filter(|n| **n > 0).count() > 1
    }

    pub fn total(&self) -> usize {
        self.lf + self.crlf + self.cr
    }

    /// Most frequent kind; `None` without any line break. Ties: CRLF, then LF, then CR.
    pub fn majority(&self) -> Option<LineEnding> {
        if self.total() == 0 {
            return None;
        }
        let mut best = (LineEnding::CrLf, self.crlf);
        if self.lf > best.1 {
            best = (LineEnding::Lf, self.lf);
        }
        if self.cr > best.1 {
            best = (LineEnding::Cr, self.cr);
        }
        Some(best.0)
    }
}

/// Result of decoding file bytes.
#[derive(Clone, Debug)]
pub struct Decoded {
    /// Text with `\n` as the only line break.
    pub text: String,
    pub encoding: TextEncoding,
    pub eol_stats: EolStats,
    /// Invalid sequences were replaced by U+FFFD (saving would change those bytes).
    pub lossy: bool,
    /// Contains NUL bytes outside UTF-16 (probably a binary file).
    pub looks_binary: bool,
}

/// Guess the encoding of `bytes`: BOM, then valid UTF-8, then BOM-less UTF-16 (many zero bytes on one side),
/// else Windows-1252.
pub fn detect(bytes: &[u8]) -> TextEncoding {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return TextEncoding::Utf8Bom;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return TextEncoding::Utf16Le;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return TextEncoding::Utf16Be;
    }
    if let Some(e) = guess_utf16(bytes) {
        return e;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return TextEncoding::Utf8;
    }
    TextEncoding::Windows1252
}

/// BOM-less UTF-16: in the first 4 KiB, ≥ 40 % of the bytes on one side are zero and almost none on the other.
fn guess_utf16(bytes: &[u8]) -> Option<TextEncoding> {
    let n = bytes.len().min(4096) & !1;
    if n < 4 {
        return None;
    }
    let pairs = n / 2;
    let (mut even0, mut odd0) = (0usize, 0usize);
    for i in (0..n).step_by(2) {
        if bytes[i] == 0 {
            even0 += 1;
        }
        if bytes[i + 1] == 0 {
            odd0 += 1;
        }
    }
    if odd0 * 10 >= pairs * 4 && even0 * 20 <= pairs {
        Some(TextEncoding::Utf16Le)
    } else if even0 * 10 >= pairs * 4 && odd0 * 20 <= pairs {
        Some(TextEncoding::Utf16Be)
    } else {
        None
    }
}

/// Decode `bytes` (detecting the encoding unless `forced`) and normalise line breaks to `\n`.
pub fn decode(bytes: &[u8], forced: Option<TextEncoding>) -> Decoded {
    let encoding = forced.unwrap_or_else(|| detect(bytes));
    let (raw, lossy): (std::borrow::Cow<'_, str>, bool) = match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => {
            let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
            match std::str::from_utf8(b) {
                Ok(s) => (s.into(), false),
                Err(_) => (String::from_utf8_lossy(b).into_owned().into(), true),
            }
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let le = encoding == TextEncoding::Utf16Le;
            let b = bytes.strip_prefix(encoding.bom()).unwrap_or(bytes);
            let odd = b.len() % 2 == 1;
            let units = b.chunks_exact(2).map(|p| if le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) });
            let mut lossy = odd;
            let mut s = String::with_capacity(b.len() / 2);
            for r in char::decode_utf16(units) {
                match r {
                    Ok(c) => s.push(c),
                    Err(_) => {
                        lossy = true;
                        s.push('\u{FFFD}');
                    }
                }
            }
            if odd {
                s.push('\u{FFFD}');
            }
            (s.into(), lossy)
        }
        TextEncoding::Windows1252 => {
            // Without BOM sniffing: a forced Windows-1252 must not turn into UTF-8.
            let (s, had_errors) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
            (s, had_errors)
        }
    };
    let looks_binary = !matches!(encoding, TextEncoding::Utf16Le | TextEncoding::Utf16Be) && memchr_nul(bytes);
    let (text, eol_stats) = normalize_eol(&raw);
    Decoded { text, encoding, eol_stats, lossy, looks_binary }
}

fn memchr_nul(b: &[u8]) -> bool {
    b.iter().take(64 * 1024).any(|x| *x == 0)
}

/// Count line endings and turn `\r\n` and lone `\r` into `\n`.
pub fn normalize_eol(s: &str) -> (String, EolStats) {
    let mut st = EolStats::default();
    if !s.as_bytes().contains(&b'\r') {
        st.lf = s.as_bytes().iter().filter(|b| **b == b'\n').count();
        return (s.to_string(), st);
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' => {
                out.push_str(&s[last..i]);
                out.push('\n');
                if i + 1 < b.len() && b[i + 1] == b'\n' {
                    st.crlf += 1;
                    i += 2;
                } else {
                    st.cr += 1;
                    i += 1;
                }
                last = i;
            }
            b'\n' => {
                st.lf += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&s[last..]);
    (out, st)
}

/// A char the target encoding cannot represent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unencodable {
    /// Char index in the document.
    pub pos: usize,
    pub ch: char,
}

/// First char of `rope` that `enc` cannot write (`None` = everything fits).
pub fn first_unencodable(rope: &Rope, enc: TextEncoding) -> Option<Unencodable> {
    if enc.is_unicode() {
        return None;
    }
    let mut pos = 0;
    for chunk in rope.chunks() {
        for c in chunk.chars() {
            if !fits_1252(c) {
                return Some(Unencodable { pos, ch: c });
            }
            pos += 1;
        }
    }
    None
}

fn fits_1252(c: char) -> bool {
    if (c as u32) < 0x80 {
        return true;
    }
    let mut buf = [0u8; 4];
    let s = c.encode_utf8(&mut buf);
    let mut enc = encoding_rs::WINDOWS_1252.new_encoder();
    let mut out = [0u8; 8];
    let (res, _, _) = enc.encode_from_utf8_without_replacement(s, &mut out, true);
    matches!(res, encoding_rs::EncoderResult::InputEmpty)
}

/// Write `rope` in `enc` with `eol` line breaks (BOM included where the encoding has one). Fails with
/// `InvalidData` if a char does not fit the encoding; check [`first_unencodable`] first for a good message.
/// Returns the number of bytes written.
pub fn write_encoded<W: Write + ?Sized>(rope: &Rope, enc: TextEncoding, eol: LineEnding, out: &mut W) -> std::io::Result<u64> {
    let mut n = 0u64;
    let bom = enc.bom();
    out.write_all(bom)?;
    n += bom.len() as u64;
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut encoder = (enc == TextEncoding::Windows1252).then(|| encoding_rs::WINDOWS_1252.new_encoder());
    for chunk in rope.chunks() {
        buf.clear();
        let mut piece_start = 0;
        let bytes = chunk.as_bytes();
        let mut emit = |piece: &str, buf: &mut Vec<u8>| -> std::io::Result<()> {
            encode_piece(piece, enc, encoder.as_mut(), buf)
        };
        if eol == LineEnding::Lf {
            emit(chunk, &mut buf)?;
        } else {
            for (i, b) in bytes.iter().enumerate() {
                if *b == b'\n' {
                    emit(&chunk[piece_start..i], &mut buf)?;
                    emit(eol.as_str(), &mut buf)?;
                    piece_start = i + 1;
                }
            }
            emit(&chunk[piece_start..], &mut buf)?;
        }
        out.write_all(&buf)?;
        n += buf.len() as u64;
    }
    Ok(n)
}

fn encode_piece(
    s: &str,
    enc: TextEncoding,
    encoder: Option<&mut encoding_rs::Encoder>,
    buf: &mut Vec<u8>,
) -> std::io::Result<()> {
    match enc {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => buf.extend_from_slice(s.as_bytes()),
        TextEncoding::Utf16Le => s.encode_utf16().for_each(|u| buf.extend_from_slice(&u.to_le_bytes())),
        TextEncoding::Utf16Be => s.encode_utf16().for_each(|u| buf.extend_from_slice(&u.to_be_bytes())),
        TextEncoding::Windows1252 => {
            let encoder = encoder.expect("encoder for windows-1252");
            if s.is_ascii() {
                buf.extend_from_slice(s.as_bytes());
                return Ok(());
            }
            let mut src = s;
            let mut tmp = [0u8; 4096];
            loop {
                let (res, read, written) = encoder.encode_from_utf8_without_replacement(src, &mut tmp, false);
                buf.extend_from_slice(&tmp[..written]);
                src = &src[read..];
                match res {
                    encoding_rs::EncoderResult::InputEmpty => break,
                    encoding_rs::EncoderResult::OutputFull => continue,
                    encoding_rs::EncoderResult::Unmappable(c) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("character U+{:04X} cannot be written as Windows-1252", c as u32),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Encode the whole rope into memory (small texts, tests).
pub fn encode_to_vec(rope: &Rope, enc: TextEncoding, eol: LineEnding) -> std::io::Result<Vec<u8>> {
    let mut v = Vec::with_capacity(rope.len_bytes() + 16);
    write_encoded(rope, enc, eol, &mut v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Grüße, Welt – ¼ € «ok»\nzweite Zeile\n";

    #[test]
    fn round_trips_all_encodings_and_eols() {
        let rope = Rope::from_str(SAMPLE);
        for enc in TextEncoding::ALL {
            for eol in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
                let bytes = encode_to_vec(&rope, enc, eol).unwrap();
                let d = decode(&bytes, None);
                assert_eq!(d.encoding, enc, "{enc:?} {eol:?}");
                assert_eq!(d.text, SAMPLE, "{enc:?} {eol:?}");
                assert!(!d.lossy);
                assert_eq!(d.eol_stats.majority(), Some(eol));
                assert_eq!(d.eol_stats.total(), 2);
                // Same bytes again.
                assert_eq!(encode_to_vec(&Rope::from_str(&d.text), d.encoding, eol).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn utf16_without_bom_is_detected() {
        let le: Vec<u8> = "hello world\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(detect(&le), TextEncoding::Utf16Le);
        let be: Vec<u8> = "hello world\r\n".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(detect(&be), TextEncoding::Utf16Be);
        assert_eq!(decode(&be, None).text, "hello world\n");
    }

    #[test]
    fn windows_1252_fallback_and_unmappable() {
        let bytes = b"caf\xe9 \x80\r\n";
        let d = decode(bytes, None);
        assert_eq!(d.encoding, TextEncoding::Windows1252);
        assert_eq!(d.text, "café €\n");
        let r = Rope::from_str("ab€ 日");
        assert_eq!(first_unencodable(&r, TextEncoding::Windows1252), Some(Unencodable { pos: 4, ch: '日' }));
        assert!(encode_to_vec(&r, TextEncoding::Windows1252, LineEnding::Lf).is_err());
        assert_eq!(first_unencodable(&r, TextEncoding::Utf16Le), None);
    }

    #[test]
    fn mixed_line_endings_reported() {
        let (t, st) = normalize_eol("a\r\nb\r\nc\nd\re");
        assert_eq!(t, "a\nb\nc\nd\ne");
        assert_eq!(st, EolStats { lf: 1, crlf: 2, cr: 1 });
        assert!(st.is_mixed());
        assert_eq!(st.majority(), Some(LineEnding::CrLf));
        assert_eq!(EolStats::default().majority(), None);
    }

    #[test]
    fn invalid_utf8_forced_is_lossy() {
        let d = decode(b"ok\xff", Some(TextEncoding::Utf8));
        assert!(d.lossy);
        assert_eq!(d.text, "ok\u{FFFD}");
        assert!(decode(b"a\0b", None).looks_binary);
    }
}

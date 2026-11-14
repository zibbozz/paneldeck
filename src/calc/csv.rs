//! CSV-Schreiber und -Leser im eigenen Format (RFC-4180-artig, UTF-8). Der Leser nimmt **genau** das vom Schreiber erzeugte Format an
//! und lehnt alles andere mit Zeile und Spalte ab; `read_csv(write_csv(t)) == t` gilt für jede Tabelle (Rundreise).
//!
//! # Format
//!
//! ```text
//! datei    = [BOM] satz { ZEILENENDE satz } [ZEILENENDE]
//! satz     = feld { TRENNER feld }               (* alle Sätze haben genau so viele Felder wie die Kopfzeile *)
//! feld     = einfach | zitiert
//! einfach  = { Zeichen außer TRENNER, '"', CR, LF }
//! zitiert  = '"' { Zeichen außer '"' | '""' } '"'   (* darin sind Trenner, Zeilenumbrüche und beliebiges Unicode erlaubt *)
//! ```
//!
//! * `TRENNER` ist Komma oder Semikolon ([`Delimiter`]); `ZEILENENDE` ist CR LF (Standard) oder LF ([`LineEnding`]); beim Lesen sind
//!   beide erlaubt, auch gemischt. Ein einzelnes CR ohne LF ist ein Fehler.
//! * Die erste Zeile ist immer die Kopfzeile. Eine Tabelle braucht mindestens eine Spalte; Zeilen ohne Spalten gibt es nicht.
//! * Der Schreiber setzt ein Feld in Anführungszeichen, wenn es den Trenner, `"`, CR oder LF enthält, mit U+FEFF beginnt
//!   (sonst würde der Leser es für eine BOM halten) oder wenn es leer ist und die Tabelle nur eine Spalte hat
//!   (sonst gäbe es eine Leerzeile). Alles andere bleibt unverändert, auch Leerraum am Rand (es wird nichts gekürzt).
//! * Mit [`WriteOptions::bom`] beginnt die Datei mit einer UTF-8-BOM (damit Excel UTF-8 erkennt); der Leser überspringt eine BOM am Anfang.
//! * Strenge: Anführungszeichen mitten in einem einfachen Feld, Zeichen nach dem schließenden Anführungszeichen, ein nicht
//!   geschlossenes Feld, Leerzeilen, abweichende Spaltenzahl und ungültiges UTF-8 (nur [`read_csv_bytes`]) sind Fehler.
//!
//! Hinweis: Das Format schützt nicht vor Tabellenkalkulations-Formeln – Zellen, die mit `=`, `+`, `-` oder `@` beginnen,
//! werden unverändert geschrieben (sonst wäre die Rundreise nicht exakt).

use crate::state::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Delimiter {
    #[default]
    Comma,
    Semicolon,
}

impl Delimiter {
    pub fn as_char(self) -> char {
        match self {
            Delimiter::Comma => ',',
            Delimiter::Semicolon => ';',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// CR LF wie in RFC 4180.
    #[default]
    CrLf,
    Lf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::CrLf => "\r\n",
            LineEnding::Lf => "\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriteOptions {
    pub delimiter: Delimiter,
    /// UTF-8-BOM an den Anfang setzen (für Excel).
    pub bom: bool,
    pub line_ending: LineEnding,
}

/// Kopfzeile plus Datenzeilen. Jede Zeile hat genau so viele Felder wie die Kopfzeile (prüft der Schreiber).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsvTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl CsvTable {
    pub fn new<H: Into<String>>(headers: impl IntoIterator<Item = H>) -> Self {
        CsvTable { headers: headers.into_iter().map(Into::into).collect(), rows: Vec::new() }
    }

    pub fn push_row<S: Into<String>>(&mut self, row: impl IntoIterator<Item = S>) {
        self.rows.push(row.into_iter().map(Into::into).collect());
    }

    /// Spaltenindex zur Überschrift (erste Übereinstimmung, Groß-/Kleinschreibung zählt).
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.headers.iter().position(|h| h == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvErrorKind {
    /// Keine Daten (leer oder nur BOM).
    Empty,
    /// Die Tabelle hat keine Spalten (nur beim Schreiben).
    NoColumns,
    /// `"` mitten in einem nicht zitierten Feld.
    UnexpectedQuote,
    /// Zitiertes Feld wird nicht geschlossen (Position = öffnendes Anführungszeichen).
    UnterminatedQuote,
    /// Zeichen nach dem schließenden Anführungszeichen (statt Trenner oder Zeilenende).
    CharAfterQuote(char),
    /// CR ohne folgendes LF.
    BareCarriageReturn,
    /// Leere Zeile.
    BlankLine,
    /// Falsche Zahl von Feldern.
    RaggedRow { expected: usize, found: usize },
    /// Ungültiges UTF-8 (nur [`read_csv_bytes`]).
    InvalidUtf8,
}

/// Fehler mit Position: `line` und `column` sind 1-basiert (Spalte in Zeichen); Zeilen zählen physische Zeilen der Datei,
/// auch innerhalb zitierter Felder. Beim Schreiben ist `line` die Nummer der Zeile in der entstehenden Datei, `column` 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    pub kind: CsvErrorKind,
    pub line: usize,
    pub column: usize,
}

impl CsvError {
    pub fn message(&self, lang: Lang) -> String {
        let at = match lang {
            Lang::De => format!("Zeile {}, Spalte {}: ", self.line, self.column),
            Lang::En => format!("Line {}, column {}: ", self.line, self.column),
        };
        let body: String = match &self.kind {
            CsvErrorKind::Empty => lang.t("Die Datei enthält keine Daten.", "The file contains no data.").to_string(),
            CsvErrorKind::NoColumns => lang.t("Die Tabelle hat keine Spalten.", "The table has no columns.").to_string(),
            CsvErrorKind::UnexpectedQuote => lang
                .t(
                    "Unerwartetes Anführungszeichen in einem Feld ohne Anführungszeichen (das ganze Feld muss in Anführungszeichen stehen).",
                    "Unexpected quote inside an unquoted field (the whole field must be quoted).",
                )
                .to_string(),
            CsvErrorKind::UnterminatedQuote => lang
                .t("Das mit einem Anführungszeichen begonnene Feld wird nicht geschlossen.", "The quoted field is never closed.")
                .to_string(),
            CsvErrorKind::CharAfterQuote(c) => {
                let c = lang.q(&c.escape_debug().to_string());
                match lang {
                    Lang::De => format!("Nach dem schließenden Anführungszeichen folgt {c} statt eines Trenners oder Zeilenendes."),
                    Lang::En => format!("The closing quote is followed by {c} instead of a delimiter or line end."),
                }
            }
            CsvErrorKind::BareCarriageReturn => lang
                .t("Zeilenumbruch mit einzelnem CR (erwartet: CR LF oder LF).", "Line break with a lone CR (expected CR LF or LF).")
                .to_string(),
            CsvErrorKind::BlankLine => lang.t("Leere Zeile.", "Blank line.").to_string(),
            CsvErrorKind::RaggedRow { expected, found } => match lang {
                Lang::De => format!("Die Zeile hat {found} statt {expected} Felder."),
                Lang::En => format!("The row has {found} fields instead of {expected}."),
            },
            CsvErrorKind::InvalidUtf8 => lang.t("Die Datei ist nicht gültig als UTF-8 kodiert.", "The file is not valid UTF-8.").to_string(),
        };
        at + &body
    }
}

impl std::fmt::Display for CsvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message(Lang::En))
    }
}

impl std::error::Error for CsvError {}

fn needs_quotes(field: &str, delimiter: char, single_column: bool) -> bool {
    (field.is_empty() && single_column)
        || field.starts_with('\u{FEFF}')
        || field.chars().any(|c| c == delimiter || c == '"' || c == '\n' || c == '\r')
}

fn write_field(out: &mut String, field: &str, delimiter: char, single_column: bool) {
    if needs_quotes(field, delimiter, single_column) {
        out.push('"');
        for c in field.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    } else {
        out.push_str(field);
    }
}

/// Tabelle als Text schreiben. Fehler: keine Spalten oder eine Zeile mit falscher Feldzahl.
pub fn write_csv(table: &CsvTable, options: &WriteOptions) -> Result<String, CsvError> {
    let columns = table.headers.len();
    if columns == 0 {
        return Err(CsvError { kind: CsvErrorKind::NoColumns, line: 1, column: 1 });
    }
    let delim = options.delimiter.as_char();
    let eol = options.line_ending.as_str();
    let single = columns == 1;
    let mut out = String::new();
    if options.bom {
        out.push('\u{FEFF}');
    }
    let mut line = 1usize;
    let write_record = |out: &mut String, fields: &[String], line: &mut usize| {
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                out.push(delim);
            }
            write_field(out, f, delim, single);
        }
        out.push_str(eol);
        *line += 1 + fields.iter().map(|f| f.matches('\n').count()).sum::<usize>();
    };
    write_record(&mut out, &table.headers, &mut line);
    for row in &table.rows {
        if row.len() != columns {
            return Err(CsvError { kind: CsvErrorKind::RaggedRow { expected: columns, found: row.len() }, line, column: 1 });
        }
        write_record(&mut out, row, &mut line);
    }
    Ok(out)
}

/// Welcher Trenner steht in der Kopfzeile? Zählt Trenner außerhalb von Anführungszeichen in der ersten Zeile;
/// mehr Semikolons als Kommas → Semikolon, sonst Komma (auch bei Gleichstand und ohne Trenner).
pub fn detect_delimiter(text: &str) -> Delimiter {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let (mut commas, mut semis) = (0usize, 0usize);
    let mut in_quotes = false;
    for c in text.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            '\n' | '\r' if !in_quotes => break,
            ',' if !in_quotes => commas += 1,
            ';' if !in_quotes => semis += 1,
            _ => {}
        }
    }
    if semis > commas { Delimiter::Semicolon } else { Delimiter::Comma }
}

/// Lesezeiger mit Zeilen-/Spaltenzähler.
struct Cursor {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
}

impl Cursor {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    fn error(&self, kind: CsvErrorKind) -> CsvError {
        CsvError { kind, line: self.line, column: self.column }
    }
}

/// Text lesen. Die erste Zeile wird zur Kopfzeile. Siehe Moduldoku für das erlaubte Format.
pub fn read_csv(text: &str, delimiter: Delimiter) -> Result<CsvTable, CsvError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let delim = delimiter.as_char();
    let mut cur = Cursor { chars: text.chars().collect(), pos: 0, line: 1, column: 1 };
    if cur.chars.is_empty() {
        return Err(cur.error(CsvErrorKind::Empty));
    }
    let mut headers: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();
    while cur.peek().is_some() {
        if matches!(cur.peek(), Some('\n') | Some('\r')) {
            return Err(cur.error(CsvErrorKind::BlankLine));
        }
        let record_line = cur.line;
        let mut fields: Vec<String> = Vec::new();
        loop {
            let mut field = String::new();
            if cur.peek() == Some('"') {
                let (quote_line, quote_column) = (cur.line, cur.column);
                cur.bump();
                loop {
                    match cur.bump() {
                        None => return Err(CsvError { kind: CsvErrorKind::UnterminatedQuote, line: quote_line, column: quote_column }),
                        Some('"') => {
                            if cur.peek() == Some('"') {
                                cur.bump();
                                field.push('"');
                            } else {
                                break;
                            }
                        }
                        Some(c) => field.push(c),
                    }
                }
                match cur.peek() {
                    None | Some('\n') | Some('\r') => {}
                    Some(c) if c == delim => {}
                    Some(c) => return Err(cur.error(CsvErrorKind::CharAfterQuote(c))),
                }
            } else {
                while let Some(c) = cur.peek() {
                    if c == delim || c == '\n' || c == '\r' {
                        break;
                    }
                    if c == '"' {
                        return Err(cur.error(CsvErrorKind::UnexpectedQuote));
                    }
                    field.push(c);
                    cur.bump();
                }
            }
            fields.push(field);
            if cur.peek() == Some(delim) {
                cur.bump();
            } else {
                break;
            }
        }
        // Satzende: Dateiende, LF oder CR LF.
        match cur.peek() {
            None => {}
            Some('\n') => {
                cur.bump();
            }
            Some('\r') => {
                if cur.peek_at(1) == Some('\n') {
                    cur.bump();
                    cur.bump();
                } else {
                    return Err(cur.error(CsvErrorKind::BareCarriageReturn));
                }
            }
            Some(_) => unreachable!("Satz endet nur an Trenner, Zeilenende oder Dateiende"),
        }
        match &headers {
            None => headers = Some(fields),
            Some(h) => {
                if fields.len() != h.len() {
                    return Err(CsvError { kind: CsvErrorKind::RaggedRow { expected: h.len(), found: fields.len() }, line: record_line, column: 1 });
                }
                rows.push(fields);
            }
        }
    }
    Ok(CsvTable { headers: headers.unwrap_or_default(), rows })
}

/// Wie [`read_csv`], aber aus Bytes; ungültiges UTF-8 wird mit der Stelle des ersten ungültigen Bytes gemeldet.
pub fn read_csv_bytes(bytes: &[u8], delimiter: Delimiter) -> Result<CsvTable, CsvError> {
    match std::str::from_utf8(bytes) {
        Ok(text) => read_csv(text, delimiter),
        Err(e) => {
            let valid = std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or("");
            let valid = valid.strip_prefix('\u{FEFF}').unwrap_or(valid);
            let line = 1 + valid.matches('\n').count();
            let column = 1 + valid.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0);
            Err(CsvError { kind: CsvErrorKind::InvalidUtf8, line, column })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(headers: &[&str], rows: &[&[&str]]) -> CsvTable {
        CsvTable { headers: headers.iter().map(|s| s.to_string()).collect(), rows: rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect() }
    }

    fn all_option_combinations() -> Vec<WriteOptions> {
        let mut v = Vec::new();
        for delimiter in [Delimiter::Comma, Delimiter::Semicolon] {
            for bom in [false, true] {
                for line_ending in [LineEnding::CrLf, LineEnding::Lf] {
                    v.push(WriteOptions { delimiter, bom, line_ending });
                }
            }
        }
        v
    }

    fn round_trip(t: &CsvTable) {
        for o in all_option_combinations() {
            let text = write_csv(t, &o).unwrap();
            let back = read_csv(&text, o.delimiter).unwrap_or_else(|e| panic!("{o:?}: {e}\n{text:?}"));
            assert_eq!(&back, t, "{o:?}\n{text:?}");
        }
    }

    fn read_err(text: &str) -> CsvError {
        match read_csv(text, Delimiter::Comma) {
            Ok(t) => panic!("{text:?} should fail, got {t:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn simple_table_default_options() {
        let t = table(&["id", "name"], &[&["1", "Ada"], &["2", "Bob"]]);
        assert_eq!(write_csv(&t, &WriteOptions::default()).unwrap(), "id,name\r\n1,Ada\r\n2,Bob\r\n");
    }

    #[test]
    fn semicolon_and_lf() {
        let t = table(&["a", "b"], &[&["1", "2"]]);
        let o = WriteOptions { delimiter: Delimiter::Semicolon, bom: false, line_ending: LineEnding::Lf };
        assert_eq!(write_csv(&t, &o).unwrap(), "a;b\n1;2\n");
    }

    #[test]
    fn bom_is_written_and_skipped() {
        let t = table(&["a"], &[&["x"]]);
        let o = WriteOptions { bom: true, ..Default::default() };
        let text = write_csv(&t, &o).unwrap();
        assert!(text.starts_with('\u{FEFF}'));
        assert_eq!(read_csv(&text, Delimiter::Comma).unwrap(), t);
        let bytes = text.as_bytes();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
    }

    #[test]
    fn quoting_rules() {
        let t = table(&["h"], &[&["plain"], &["with,comma"], &["with;semi"], &["say \"hi\""], &["two\nlines"], &["cr\rhere"], &[" padded "]]);
        let c = write_csv(&t, &WriteOptions::default()).unwrap();
        assert_eq!(c, "h\r\nplain\r\n\"with,comma\"\r\nwith;semi\r\n\"say \"\"hi\"\"\"\r\n\"two\nlines\"\r\n\"cr\rhere\"\r\n padded \r\n");
        let s = write_csv(&t, &WriteOptions { delimiter: Delimiter::Semicolon, ..Default::default() }).unwrap();
        assert!(s.contains("\r\nwith,comma\r\n"));
        assert!(s.contains("\"with;semi\""));
    }

    #[test]
    fn empty_cells_stay_unquoted_with_several_columns() {
        let t = table(&["a", "b", "c"], &[&["", "", ""], &["x", "", "z"]]);
        let text = write_csv(&t, &WriteOptions::default()).unwrap();
        assert_eq!(text, "a,b,c\r\n,,\r\nx,,z\r\n");
        round_trip(&t);
    }

    #[test]
    fn single_column_empty_cell_is_quoted() {
        let t = table(&["a"], &[&[""], &["x"], &[""]]);
        let text = write_csv(&t, &WriteOptions { line_ending: LineEnding::Lf, ..Default::default() }).unwrap();
        assert_eq!(text, "a\n\"\"\nx\n\"\"\n");
        round_trip(&t);
    }

    #[test]
    fn single_empty_header_is_quoted() {
        let t = table(&[""], &[&["x"]]);
        let text = write_csv(&t, &WriteOptions::default()).unwrap();
        assert!(text.starts_with("\"\"\r\n"));
        round_trip(&t);
    }

    #[test]
    fn header_only_table() {
        let t = table(&["a", "b"], &[]);
        assert_eq!(write_csv(&t, &WriteOptions::default()).unwrap(), "a,b\r\n");
        round_trip(&t);
    }

    #[test]
    fn round_trip_umlauts_and_unicode() {
        let t = table(&["Name", "Straße", "日本語"], &[&["Müller", "Äpfel & Öl", "漢字 かな"], &["ß", "ü", "é"], &["Ελληνικά", "Кириллица", "עברית"]]);
        round_trip(&t);
    }

    #[test]
    fn round_trip_emoji_and_combining_characters() {
        let t = table(&["e", "c"], &[&["😀 👨‍👩‍👧‍👦 🇩🇪", "e\u{301}"], &["𝔘𝔫𝔦𝔠𝔬𝔡𝔢", "\u{200B}zero\u{200D}width"]]);
        round_trip(&t);
    }

    #[test]
    fn round_trip_special_characters_and_quotes() {
        let t = table(
            &["a,b", "c;d", "\"q\""],
            &[&["\"", "\"\"", "\"\"\""], &["a\"b", "\"a", "a\""], &[",", ";", ",;,"], &["'single'", "tab\there", "back\\slash"]],
        );
        round_trip(&t);
    }

    #[test]
    fn round_trip_line_breaks_in_cells() {
        let t = table(&["note", "x"], &[&["line1\nline2", "a"], &["cr\r\nlf", "b"], &["only\rcr", "c"], &["\n", "\r\n"], &["trailing\n", "\nleading"], &["\n\n\n", "d"]]);
        round_trip(&t);
    }

    #[test]
    fn round_trip_whitespace_only_and_padded() {
        let t = table(&["a", "b"], &[&[" ", "  x  "], &["\t", " \u{A0} "]]);
        round_trip(&t);
    }

    #[test]
    fn round_trip_leading_bom_character_in_first_header() {
        let t = table(&["\u{FEFF}name", "b"], &[&["\u{FEFF}", "\u{FEFF}x"]]);
        round_trip(&t);
    }

    #[test]
    fn round_trip_single_column_with_odd_values() {
        let t = table(&["only"], &[&[""], &["\n"], &[" "], &["\""], &[","]]);
        round_trip(&t);
    }

    #[test]
    fn round_trip_many_rows_and_columns() {
        let headers: Vec<String> = (0..20).map(|i| format!("col{i}")).collect();
        let mut t = CsvTable { headers, rows: vec![] };
        for r in 0..200 {
            t.rows.push((0..20).map(|c| if (r + c) % 7 == 0 { String::new() } else { format!("r{r},c{c} \"q\"\n{}", r * c) }).collect());
        }
        round_trip(&t);
    }

    #[test]
    fn round_trip_long_cell() {
        let long = "ä,\"\n".repeat(50_000);
        let t = table(&["a"], &[&[long.as_str()]]);
        round_trip(&t);
    }

    #[test]
    fn writer_rejects_ragged_rows_with_line() {
        let t = table(&["a", "b"], &[&["1", "2"], &["only one"]]);
        let e = write_csv(&t, &WriteOptions::default()).unwrap_err();
        assert_eq!(e.kind, CsvErrorKind::RaggedRow { expected: 2, found: 1 });
        assert_eq!(e.line, 3);
        let t = table(&["a", "b"], &[&["multi\nline", "2"], &["1", "2", "3"]]);
        let e = write_csv(&t, &WriteOptions::default()).unwrap_err();
        assert_eq!(e.kind, CsvErrorKind::RaggedRow { expected: 2, found: 3 });
        assert_eq!(e.line, 4); // Kopfzeile + 2 Zeilen des ersten Satzes
    }

    #[test]
    fn writer_rejects_no_columns() {
        let e = write_csv(&CsvTable::default(), &WriteOptions::default()).unwrap_err();
        assert_eq!(e.kind, CsvErrorKind::NoColumns);
    }

    #[test]
    fn reader_accepts_lf_crlf_and_missing_final_newline() {
        let expected = table(&["a", "b"], &[&["1", "2"], &["3", "4"]]);
        assert_eq!(read_csv("a,b\n1,2\n3,4\n", Delimiter::Comma).unwrap(), expected);
        assert_eq!(read_csv("a,b\r\n1,2\r\n3,4\r\n", Delimiter::Comma).unwrap(), expected);
        assert_eq!(read_csv("a,b\r\n1,2\r\n3,4", Delimiter::Comma).unwrap(), expected);
        assert_eq!(read_csv("a,b\n1,2\r\n3,4", Delimiter::Comma).unwrap(), expected);
    }

    #[test]
    fn reader_keeps_line_breaks_inside_quotes_exactly() {
        let t = read_csv("a,b\r\n\"x\r\ny\",\"p\nq\"\r\n", Delimiter::Comma).unwrap();
        assert_eq!(t.rows[0], vec!["x\r\ny".to_string(), "p\nq".to_string()]);
    }

    #[test]
    fn reader_uses_the_chosen_delimiter() {
        let t = read_csv("a;b\n1,5;2\n", Delimiter::Semicolon).unwrap();
        assert_eq!(t.rows[0], vec!["1,5".to_string(), "2".to_string()]);
        let t = read_csv("a;b\n1;2\n", Delimiter::Comma).unwrap();
        assert_eq!(t.headers, vec!["a;b".to_string()]);
    }

    #[test]
    fn reader_trailing_delimiter_means_empty_last_field() {
        let t = read_csv("a,b\n1,\n", Delimiter::Comma).unwrap();
        assert_eq!(t.rows[0], vec!["1".to_string(), String::new()]);
    }

    #[test]
    fn reader_only_header_without_newline() {
        let t = read_csv("a,b", Delimiter::Comma).unwrap();
        assert_eq!(t.headers, vec!["a".to_string(), "b".to_string()]);
        assert!(t.rows.is_empty());
    }

    #[test]
    fn error_empty_input() {
        let e = read_err("");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::Empty, 1, 1));
        assert_eq!(read_err("\u{FEFF}").kind, CsvErrorKind::Empty);
    }

    #[test]
    fn error_quote_inside_unquoted_field() {
        let e = read_err("a,b\n1,x\"y\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::UnexpectedQuote, 2, 4));
        let e = read_err("a,b\n1, \"y\"\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::UnexpectedQuote, 2, 4));
    }

    #[test]
    fn error_unterminated_quote_points_at_the_opening_quote() {
        let e = read_err("a,b\n1,\"abc\nstill open\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::UnterminatedQuote, 2, 3));
        let e = read_err("\"abc");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::UnterminatedQuote, 1, 1));
        let e = read_err("a\n\"\"\"");
        assert_eq!(e.kind, CsvErrorKind::UnterminatedQuote);
    }

    #[test]
    fn error_text_after_closing_quote() {
        let e = read_err("a,b\n\"x\"y,2\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::CharAfterQuote('y'), 2, 4));
        let e = read_err("a,b\n\"x\" ,2\n");
        assert_eq!(e.kind, CsvErrorKind::CharAfterQuote(' '));
    }

    #[test]
    fn error_bare_carriage_return() {
        let e = read_err("a,b\r1,2\r");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::BareCarriageReturn, 1, 4));
        let e = read_err("a,b\n1,2\r");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::BareCarriageReturn, 2));
    }

    #[test]
    fn error_blank_lines() {
        let e = read_err("a,b\n\n1,2\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::BlankLine, 2, 1));
        let e = read_err("a,b\n1,2\n\n");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::BlankLine, 3));
        let e = read_err("\na,b\n");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::BlankLine, 1));
        let e = read_err("a,b\r\n\r\n");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::BlankLine, 2));
    }

    #[test]
    fn error_ragged_rows_report_the_start_line_of_the_record() {
        let e = read_err("a,b\n1,2\n3\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::RaggedRow { expected: 2, found: 1 }, 3, 1));
        let e = read_err("a,b\n\"x\ny\",2,3\n");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::RaggedRow { expected: 2, found: 3 }, 2));
        // Zeilenzähler läuft durch zitierte Zeilenumbrüche weiter.
        let e = read_err("a,b\n\"x\ny\",2\n\"z\",\n\"q\"\n");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::RaggedRow { expected: 2, found: 1 }, 5));
    }

    #[test]
    fn error_position_after_quoted_newlines() {
        let e = read_err("a,b\n\"x\ny\",2\n1,2\"\n");
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::UnexpectedQuote, 4, 4));
    }

    #[test]
    fn error_column_counts_characters_not_bytes() {
        let e = read_err("a,b\n\u{e4}\u{e4}\u{1F600},x\"\n");
        assert_eq!((e.line, e.column), (2, 6));
    }

    #[test]
    fn first_error_wins() {
        // Zeile 2 ist ragged, Zeile 3 hat ein falsches Anführungszeichen: gemeldet wird Zeile 2.
        let e = read_err("a,b\n1\n2,x\"y\n");
        assert_eq!((e.kind.clone(), e.line), (CsvErrorKind::RaggedRow { expected: 2, found: 1 }, 2));
    }

    #[test]
    fn invalid_utf8_reports_line_and_column() {
        let mut bytes = b"a,b\n1,2\nx,".to_vec();
        bytes.push(0xFF);
        let e = read_csv_bytes(&bytes, Delimiter::Comma).unwrap_err();
        assert_eq!((e.kind.clone(), e.line, e.column), (CsvErrorKind::InvalidUtf8, 3, 3));
        let ok = read_csv_bytes("a,b\n\u{fc},2\n".as_bytes(), Delimiter::Comma).unwrap();
        assert_eq!(ok.rows[0][0], "\u{fc}");
        let e = read_csv_bytes(&[0xFF], Delimiter::Comma).unwrap_err();
        assert_eq!((e.line, e.column), (1, 1));
    }

    #[test]
    fn bytes_with_bom_work() {
        let t = read_csv_bytes(b"\xEF\xBB\xBFa,b\r\n1,2\r\n", Delimiter::Comma).unwrap();
        assert_eq!(t.headers, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn messages_are_bilingual_with_position() {
        let e = read_err("a,b\n1,x\"y\n");
        let (en, de) = (e.message(Lang::En), e.message(Lang::De));
        assert!(en.starts_with("Line 2, column 4: "), "{en}");
        assert!(de.starts_with("Zeile 2, Spalte 4: "), "{de}");
        assert_ne!(en.split_once(": ").unwrap().1, de.split_once(": ").unwrap().1);
        assert_eq!(e.to_string(), en);
    }

    #[test]
    fn every_error_kind_has_two_different_messages() {
        let kinds = [
            CsvErrorKind::Empty,
            CsvErrorKind::NoColumns,
            CsvErrorKind::UnexpectedQuote,
            CsvErrorKind::UnterminatedQuote,
            CsvErrorKind::CharAfterQuote('x'),
            CsvErrorKind::BareCarriageReturn,
            CsvErrorKind::BlankLine,
            CsvErrorKind::RaggedRow { expected: 3, found: 2 },
            CsvErrorKind::InvalidUtf8,
        ];
        for kind in kinds {
            let e = CsvError { kind, line: 4, column: 2 };
            let (en, de) = (e.message(Lang::En), e.message(Lang::De));
            assert!(en.contains("Line 4, column 2") && de.contains("Zeile 4, Spalte 2"));
            assert_ne!(en.split_once(": ").unwrap().1, de.split_once(": ").unwrap().1, "{e:?}");
        }
    }

    #[test]
    fn char_after_quote_message_shows_escaped_char() {
        let e = CsvError { kind: CsvErrorKind::CharAfterQuote('\t'), line: 1, column: 1 };
        assert!(e.message(Lang::En).contains("\\t"));
    }

    #[test]
    fn detect_delimiter_cases() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n"), Delimiter::Comma);
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n"), Delimiter::Semicolon);
        assert_eq!(detect_delimiter("\u{FEFF}a;b\r\n1,5;2\r\n"), Delimiter::Semicolon);
        assert_eq!(detect_delimiter("\"a;b;c\",d\n1,2\n"), Delimiter::Comma);
        assert_eq!(detect_delimiter("\"a,b,c\";d\n"), Delimiter::Semicolon);
        assert_eq!(detect_delimiter("single\n1;2;3\n"), Delimiter::Comma); // nur die erste Zeile zählt
        assert_eq!(detect_delimiter(""), Delimiter::Comma);
        assert_eq!(detect_delimiter("a,b;c\n"), Delimiter::Comma); // Gleichstand
    }

    #[test]
    fn written_files_are_detected_with_their_delimiter() {
        let t = table(&["a,b", "c;d", "e"], &[&["1", "2", "3"]]);
        for o in all_option_combinations() {
            let text = write_csv(&t, &o).unwrap();
            // Bei zwei Kommas und einem Semikolon in den Überschriften wäre „Komma“ falsch geraten, wenn Zitate mitgezählt würden.
            let detected = detect_delimiter(&text);
            assert_eq!(detected, o.delimiter, "{o:?}\n{text:?}");
        }
    }

    #[test]
    fn table_helpers() {
        let mut t = CsvTable::new(["id", "title"]);
        t.push_row(["1", "First"]);
        t.push_row(vec![String::from("2"), String::from("Second")]);
        assert_eq!(t.column_index("title"), Some(1));
        assert_eq!(t.column_index("Title"), None);
        assert_eq!(t.rows.len(), 2);
        round_trip(&t);
    }

    #[test]
    fn duplicate_headers_are_allowed() {
        let t = table(&["a", "a"], &[&["1", "2"]]);
        assert_eq!(t.column_index("a"), Some(0));
        round_trip(&t);
    }

    #[test]
    fn written_text_has_one_record_per_line_without_embedded_breaks() {
        let t = table(&["a", "b"], &[&["1", "2"], &["3", "4"]]);
        let text = write_csv(&t, &WriteOptions::default()).unwrap();
        assert_eq!(text.matches("\r\n").count(), 3);
        assert!(!text.replace("\r\n", "").contains('\n'));
    }
}

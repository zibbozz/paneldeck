//! Lexer: zerlegt den Eingabetext in Token mit Byte-Positionen. Fehlertolerant (für Parser und Autovervollständigung).

use super::ast::{CmpOp, Span, ValueKind};
use super::error::{ErrorCode, ParseError};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    /// unquotiertes Wort (auch Schlüsselwörter, Zahlen, Daten)
    Word(String),
    /// Text in Anführungszeichen; `text` ist entschlüsselt, `closed` = schließendes Anführungszeichen vorhanden
    Quoted { text: String, closed: bool },
    Op(CmpOp),
    LParen,
    RParen,
    Comma,
    /// unbekanntes Zeichen (der Fehler wurde bereits gemeldet)
    Invalid(char),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// Zeichen, die in einem unquotierten Wort vorkommen dürfen (`Done`, `2.0.0`, `PAN-12`, `2026-11-14T10:30`, Umlaute …).
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// Reservierte Wörter: müssen als Name oder Wert in Anführungszeichen stehen.
/// (`BY`, `ASC`, `DESC` sind nur innerhalb von `ORDER BY` Schlüsselwörter und deshalb nicht reserviert.)
pub(crate) fn is_reserved_word(s: &str) -> bool {
    const WORDS: [&str; 9] = ["AND", "OR", "NOT", "IN", "IS", "EMPTY", "NULL", "ORDER", "SORT"];
    WORDS.iter().any(|w| w.eq_ignore_ascii_case(s))
}

/// Ordnet ein unquotiertes Wort syntaktisch ein: Zahl, Datum, Datum mit Uhrzeit oder einfaches Wort.
pub(crate) fn classify_word(s: &str) -> ValueKind {
    if is_number(s) {
        ValueKind::Number(s.to_string())
    } else if is_date_shape(s) {
        ValueKind::Date(s.to_string())
    } else if is_datetime_shape(s) {
        ValueKind::DateTime(s.to_string())
    } else {
        ValueKind::Word(s.to_string())
    }
}

/// `-?[0-9]+(\.[0-9]+)?`
pub(crate) fn is_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// `JJJJ-MM-TT` (nur die Form)
fn is_date_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// `JJJJ-MM-TTTHH:MM` oder `…:SS` (nur die Form)
fn is_datetime_shape(s: &str) -> bool {
    let Some((date, time)) = s.split_once('T') else { return false };
    if !is_date_shape(date) {
        return false;
    }
    let parts: Vec<&str> = time.split(':').collect();
    (parts.len() == 2 || parts.len() == 3) && parts.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Zerlegt `input`. Gibt die Token und die lexikalischen Fehler zurück (unbekannte Zeichen, offene Strings, falsche Escapes).
pub(crate) fn lex(input: &str) -> (Vec<Token>, Vec<ParseError>) {
    let mut toks = Vec::new();
    let mut errs = Vec::new();
    let mut it = input.char_indices().peekable();
    while let Some(&(i, c)) = it.peek() {
        if c.is_whitespace() {
            it.next();
            continue;
        }
        let single = |tok: Tok, len: usize| Token { tok, span: Span::new(i, len) };
        match c {
            '(' => {
                it.next();
                toks.push(single(Tok::LParen, 1));
            }
            ')' => {
                it.next();
                toks.push(single(Tok::RParen, 1));
            }
            ',' => {
                it.next();
                toks.push(single(Tok::Comma, 1));
            }
            '=' => {
                it.next();
                toks.push(single(Tok::Op(CmpOp::Eq), 1));
            }
            '~' => {
                it.next();
                toks.push(single(Tok::Op(CmpOp::Contains), 1));
            }
            '!' | '<' | '>' => {
                it.next();
                let next = it.peek().map(|&(_, n)| n);
                let (op, len) = match (c, next) {
                    ('!', Some('=')) => (Some(CmpOp::Ne), 2),
                    ('!', Some('~')) => (Some(CmpOp::NotContains), 2),
                    ('<', Some('=')) => (Some(CmpOp::Le), 2),
                    ('>', Some('=')) => (Some(CmpOp::Ge), 2),
                    ('<', _) => (Some(CmpOp::Lt), 1),
                    ('>', _) => (Some(CmpOp::Gt), 1),
                    _ => (None, 1),
                };
                if len == 2 {
                    it.next();
                }
                match op {
                    Some(op) => toks.push(single(Tok::Op(op), len)),
                    None => {
                        errs.push(ParseError { span: Span::new(i, 1), code: ErrorCode::UnexpectedChar(c) });
                        toks.push(single(Tok::Invalid(c), 1));
                    }
                }
            }
            '"' | '\'' => {
                it.next();
                let quote = c;
                let mut text = String::new();
                let mut closed = false;
                let mut end = input.len();
                while let Some((j, d)) = it.next() {
                    if d == quote {
                        closed = true;
                        end = j + d.len_utf8();
                        break;
                    }
                    if d == '\\' {
                        match it.next() {
                            Some((_, e @ ('\\' | '"' | '\''))) => text.push(e),
                            Some((_, 'n')) => text.push('\n'),
                            Some((_, 't')) => text.push('\t'),
                            Some((_, 'r')) => text.push('\r'),
                            Some((k, e)) => {
                                errs.push(ParseError { span: Span::new(j, k + e.len_utf8() - j), code: ErrorCode::InvalidEscape(e) });
                                text.push(e);
                            }
                            None => break, // Eingabe endet nach dem Backslash: String bleibt offen
                        }
                        continue;
                    }
                    text.push(d);
                }
                let span = Span::new(i, end - i);
                if !closed {
                    errs.push(ParseError { span, code: ErrorCode::UnterminatedString });
                }
                toks.push(Token { tok: Tok::Quoted { text, closed }, span });
            }
            c if is_word_char(c) => {
                let mut end = i;
                while let Some(&(j, d)) = it.peek() {
                    if is_word_char(d) {
                        end = j + d.len_utf8();
                        it.next();
                    } else {
                        break;
                    }
                }
                toks.push(Token { tok: Tok::Word(input[i..end].to_string()), span: Span::new(i, end - i) });
            }
            other => {
                it.next();
                let len = other.len_utf8();
                errs.push(ParseError { span: Span::new(i, len), code: ErrorCode::UnexpectedChar(other) });
                toks.push(Token { tok: Tok::Invalid(other), span: Span::new(i, len) });
            }
        }
    }
    (toks, errs)
}

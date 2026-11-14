//! Rekursiver Abstieg über die Token des Lexers. Sammelt mehrere Fehler (Wiederaufsetzen an `AND`/`OR`/`ORDER`/`)`).

use super::ast::*;
use super::error::{ErrorCode, Expect, Found, ParseError};
use super::lexer::{Tok, Token, classify_word, lex};

/// Tiefste erlaubte Verschachtelung (Klammern und `NOT`), schützt vor Stapelüberlauf bei bösartiger Eingabe.
const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kw {
    And,
    Or,
    Not,
    In,
    Is,
    Empty,
    Null,
    Order,
    Sort,
    By,
    Asc,
    Desc,
}

pub(crate) fn kw_of(word: &str) -> Option<Kw> {
    const TABLE: [(&str, Kw); 12] = [
        ("AND", Kw::And),
        ("OR", Kw::Or),
        ("NOT", Kw::Not),
        ("IN", Kw::In),
        ("IS", Kw::Is),
        ("EMPTY", Kw::Empty),
        ("NULL", Kw::Null),
        ("ORDER", Kw::Order),
        ("SORT", Kw::Sort),
        ("BY", Kw::By),
        ("ASC", Kw::Asc),
        ("DESC", Kw::Desc),
    ];
    TABLE.iter().find(|(w, _)| w.eq_ignore_ascii_case(word)).map(|&(_, k)| k)
}

/// Schlüsselwort eines Tokens (nur unquotierte Wörter).
fn tok_kw(t: &Token) -> Option<Kw> {
    match &t.tok {
        Tok::Word(w) => kw_of(w),
        _ => None,
    }
}

/// Reserviert im Sinne von „darf nicht als Name oder Wert stehen“ (ohne `BY`/`ASC`/`DESC`).
pub(crate) fn is_reserved_kw(k: Kw) -> bool {
    !matches!(k, Kw::By | Kw::Asc | Kw::Desc)
}

/// Parst eine Abfrage. Bei Fehlern kommen alle gefundenen (nach Position sortiert) zurück.
pub fn parse(input: &str) -> Result<Query, Vec<ParseError>> {
    let (toks, mut errors) = lex(input);
    let toks: Vec<Token> = toks.into_iter().filter(|t| !matches!(t.tok, Tok::Invalid(_))).collect();
    let mut p = Parser { src: input, toks, pos: 0, errors: Vec::new(), order: Vec::new(), order_seen: false, depth: 0 };
    let has_filter = p.peek().is_some() && !p.is_order_start();
    let filter = p.parse_query_filter();
    p.parse_rest(!has_filter || filter.is_some());
    errors.append(&mut p.errors);
    if errors.is_empty() {
        Ok(Query { filter, order: p.order })
    } else {
        errors.sort_by_key(|e| e.span.start);
        errors.dedup();
        Err(errors)
    }
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
    order: Vec<OrderTerm>,
    order_seen: bool,
    depth: usize,
}

impl<'a> Parser<'a> {
    // ----- Grundoperationen -----

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, n: usize) -> Option<&Token> {
        self.toks.get(self.pos + n)
    }

    fn peek_kw(&self) -> Option<Kw> {
        self.peek().and_then(tok_kw)
    }

    fn advance(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn end_span(&self) -> Span {
        Span::new(self.src.len(), 0)
    }

    fn at_span(&self) -> Span {
        self.peek().map(|t| t.span).unwrap_or_else(|| self.end_span())
    }

    fn found(&self) -> Found {
        match self.peek() {
            Some(t) => Found::Token(self.src[t.span.range()].to_string()),
            None => Found::End,
        }
    }

    fn err(&mut self, span: Span, code: ErrorCode) {
        self.errors.push(ParseError { span, code });
    }

    /// Fehler „erwartet: …“ am aktuellen Token (bzw. am Ende der Eingabe).
    fn expected(&mut self, what: Expect) {
        let (span, found) = (self.at_span(), self.found());
        self.err(span, ErrorCode::Expected { what, found });
    }

    fn is_order_start(&self) -> bool {
        matches!(self.peek_kw(), Some(Kw::Order | Kw::Sort))
    }

    /// Beginnt am aktuellen Token eine Bedingung?
    fn at_condition_start(&self) -> bool {
        match self.peek() {
            Some(t) => match &t.tok {
                Tok::Quoted { .. } | Tok::LParen => true,
                Tok::Word(w) => match kw_of(w) {
                    None => true,
                    Some(k) => matches!(k, Kw::Not | Kw::Is),
                },
                _ => false,
            },
            None => false,
        }
    }

    /// Überspringt Token bis zu einer Stelle, an der wieder aufgesetzt werden kann: Ende, `AND`, `OR`, `ORDER`/`SORT`
    /// oder eine `)`, die nicht zu einer beim Überspringen geöffneten `(` gehört. Verbraucht diese Token nicht.
    fn recover(&mut self) {
        let mut depth = 0usize;
        while let Some(t) = self.peek() {
            match &t.tok {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Tok::Word(w) if depth == 0 && matches!(kw_of(w), Some(Kw::And | Kw::Or | Kw::Order | Kw::Sort)) => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// Überspringt innerhalb einer Liste/Argumentliste bis zum nächsten `,` oder `)` (auf der aktuellen Ebene) bzw. `ORDER`/Ende.
    fn recover_in_list(&mut self) {
        let mut depth = 0usize;
        while let Some(t) = self.peek() {
            match &t.tok {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Tok::Comma if depth == 0 => return,
                Tok::Word(w) if matches!(kw_of(w), Some(Kw::Order | Kw::Sort)) => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    // ----- Abfrage -----

    fn parse_query_filter(&mut self) -> Option<Expr> {
        if self.peek().is_none() || self.is_order_start() {
            return None;
        }
        self.parse_or()
    }

    /// Alles nach dem Filter: `ORDER BY`-Blöcke und übrig gebliebene Token. `filter_ok` = der Filter war vollständig gültig.
    fn parse_rest(&mut self, filter_ok: bool) {
        let mut junk_run = false;
        let mut quiet = !filter_ok;
        while let Some(t) = self.peek() {
            if self.is_order_start() {
                let before = self.errors.len();
                self.parse_order_by();
                quiet = quiet || self.errors.len() > before;
                junk_run = false;
                continue;
            }
            let span = t.span;
            if matches!(t.tok, Tok::RParen) {
                self.err(span, ErrorCode::UnmatchedParen);
                self.pos += 1;
                junk_run = false;
                continue;
            }
            // Überzähliges Zeug: nur den ersten Token einer zusammenhängenden Folge melden – und gar nichts,
            // wenn davor schon ein Fehler war (Folgefehler vermeiden).
            if !junk_run && !quiet {
                let what = if self.order_seen { Expect::OrderTail } else { Expect::LogicOrEnd };
                self.expected(what);
            }
            junk_run = true;
            self.pos += 1;
        }
    }

    fn parse_or(&mut self) -> Option<Expr> {
        let mut items = Vec::new();
        let mut ok = true;
        match self.parse_and() {
            Some(e) => items.push(e),
            None => ok = false,
        }
        while self.peek_kw() == Some(Kw::Or) {
            self.pos += 1;
            match self.parse_and() {
                Some(e) => items.push(e),
                None => ok = false,
            }
        }
        if !ok {
            return None;
        }
        Some(if items.len() == 1 { items.remove(0) } else { Expr::Or(items) })
    }

    fn parse_and(&mut self) -> Option<Expr> {
        let mut items = Vec::new();
        let mut ok = true;
        match self.parse_not() {
            Some(e) => items.push(e),
            None => ok = false,
        }
        loop {
            let implicit = if self.peek_kw() == Some(Kw::And) {
                self.pos += 1;
                false
            } else if self.at_condition_start() {
                // Zwei Bedingungen ohne AND/OR dazwischen: einmal melden, dann so tun, als stünde ein AND da.
                // Weitere Fehler in der folgenden Bedingung werden unterdrückt (meist nur Folgefehler).
                self.expected(Expect::LogicOrEnd);
                true
            } else {
                break;
            };
            let mark = self.errors.len();
            let item = self.parse_not();
            if implicit && self.errors.len() > mark {
                self.errors.truncate(mark);
                if item.is_none() {
                    self.recover();
                }
            }
            match item {
                Some(e) if !implicit => items.push(e),
                Some(e) => {
                    items.push(e);
                    ok = false; // das fehlende AND war ein Fehler
                }
                None => ok = false,
            }
        }
        if !ok {
            return None;
        }
        Some(if items.len() == 1 { items.remove(0) } else { Expr::And(items) })
    }

    fn parse_not(&mut self) -> Option<Expr> {
        if self.peek_kw() == Some(Kw::Not) {
            if self.depth >= MAX_DEPTH {
                let span = self.at_span();
                self.err(span, ErrorCode::TooDeep);
                self.pos = self.toks.len();
                return None;
            }
            self.pos += 1;
            self.depth += 1;
            let inner = self.parse_not();
            self.depth -= 1;
            return inner.map(|e| Expr::Not(Box::new(e)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        let Some(t) = self.peek().cloned() else {
            self.expected(Expect::Condition);
            return None;
        };
        match &t.tok {
            Tok::LParen => self.parse_group(t.span),
            Tok::Word(w) if kw_of(w) == Some(Kw::Is) => self.parse_predicate(),
            Tok::Word(w) if kw_of(w).is_some_and(is_reserved_kw) => {
                self.expected(Expect::Condition);
                self.recover();
                None
            }
            Tok::Word(w) => {
                let name = Name::at(w.clone(), t.span);
                self.pos += 1;
                self.parse_field_condition(name, false)
            }
            Tok::Quoted { text, .. } => {
                let name = Name::at(text.clone(), t.span);
                self.pos += 1;
                self.parse_field_condition(name, true)
            }
            // Eine `)` ohne offene Klammer meldet `parse_rest` als „ohne öffnende Klammer“.
            Tok::RParen if self.depth == 0 => None,
            _ => {
                self.expected(Expect::Condition);
                self.recover();
                None
            }
        }
    }

    fn parse_group(&mut self, open: Span) -> Option<Expr> {
        if self.depth >= MAX_DEPTH {
            self.err(open, ErrorCode::TooDeep);
            self.pos = self.toks.len();
            return None;
        }
        self.pos += 1; // (
        let mark = self.errors.len();
        self.depth += 1;
        let inner = self.parse_or();
        self.depth -= 1;
        match self.peek().map(|t| &t.tok) {
            Some(Tok::RParen) => {
                self.pos += 1;
                inner
            }
            None => {
                if self.errors.len() == mark {
                    self.err(open, ErrorCode::UnclosedParen);
                }
                None
            }
            Some(_) if self.is_order_start() => {
                if self.errors.len() == mark {
                    self.err(open, ErrorCode::UnclosedParen);
                }
                None
            }
            Some(_) => {
                // Überzähliges Zeug vor der schließenden Klammer
                if self.errors.len() == mark {
                    self.expected(Expect::LogicOrEnd);
                }
                self.recover();
                if matches!(self.peek().map(|t| &t.tok), Some(Tok::RParen)) {
                    self.pos += 1;
                }
                None
            }
        }
    }

    /// `IS [NOT] name` am Anfang einer Bedingung.
    fn parse_predicate(&mut self) -> Option<Expr> {
        self.pos += 1; // IS
        let mut negated = false;
        if self.peek_kw() == Some(Kw::Not) {
            negated = true;
            self.pos += 1;
        }
        let Some(t) = self.peek().cloned() else {
            self.expected(Expect::PredicateName);
            return None;
        };
        let name = match &t.tok {
            Tok::Word(w) if kw_of(w).is_none_or(|k| !is_reserved_kw(k)) => Name::at(w.clone(), t.span),
            Tok::Quoted { text, .. } => Name::at(text.clone(), t.span),
            _ => {
                self.expected(Expect::PredicateName);
                self.recover();
                return None;
            }
        };
        self.pos += 1;
        Some(Expr::Predicate { name, negated })
    }

    /// Nach dem Feldnamen (schon verbraucht): Operator und Wert(e). `quoted` = der Name stand in Anführungszeichen
    /// (dann ist er ohne Operator ein Freitext).
    fn parse_field_condition(&mut self, field: Name, quoted: bool) -> Option<Expr> {
        let Some(t) = self.peek().cloned() else {
            if quoted {
                return Some(Expr::Text(field.text, field.loc));
            }
            self.expected(Expect::Operator);
            return None;
        };
        match &t.tok {
            Tok::Op(op) => {
                self.pos += 1;
                match self.parse_value() {
                    Some(value) => Some(Expr::Compare { field, op: *op, op_loc: Loc(t.span), value }),
                    None => {
                        self.recover();
                        None
                    }
                }
            }
            Tok::Word(w) => match kw_of(w) {
                Some(Kw::In) => {
                    self.pos += 1;
                    self.parse_in_list(field, false, t.span)
                }
                Some(Kw::Not) => {
                    self.pos += 1;
                    if self.peek_kw() == Some(Kw::In) {
                        let in_span = self.at_span();
                        self.pos += 1;
                        self.parse_in_list(field, true, t.span.union(in_span))
                    } else {
                        self.expected(Expect::In);
                        self.recover();
                        None
                    }
                }
                Some(Kw::Is) => {
                    self.pos += 1;
                    let mut negated = false;
                    let mut span = t.span;
                    if self.peek_kw() == Some(Kw::Not) {
                        negated = true;
                        span = span.union(self.at_span());
                        self.pos += 1;
                    }
                    if matches!(self.peek_kw(), Some(Kw::Empty | Kw::Null)) {
                        span = span.union(self.at_span());
                        self.pos += 1;
                        Some(Expr::IsEmpty { field, negated, op_loc: Loc(span) })
                    } else {
                        self.expected(Expect::EmptyKeyword);
                        self.recover();
                        None
                    }
                }
                _ if quoted => Some(Expr::Text(field.text, field.loc)),
                _ => {
                    self.expected(Expect::Operator);
                    self.recover();
                    None
                }
            },
            _ if quoted => Some(Expr::Text(field.text, field.loc)),
            _ => {
                self.expected(Expect::Operator);
                self.recover();
                None
            }
        }
    }

    /// `( wert, wert, … )` nach `IN` (das `IN` ist schon verbraucht).
    fn parse_in_list(&mut self, field: Name, negated: bool, op_span: Span) -> Option<Expr> {
        let Some(open) = self.peek().filter(|t| matches!(t.tok, Tok::LParen)).map(|t| t.span) else {
            self.expected(Expect::OpenParen);
            self.recover();
            return None;
        };
        self.pos += 1;
        if let Some(close) = self.peek().filter(|t| matches!(t.tok, Tok::RParen)).map(|t| t.span) {
            self.err(open.union(close), ErrorCode::EmptyList);
            self.pos += 1;
            return None;
        }
        let mut values = Vec::new();
        let mut ok = true;
        loop {
            match self.parse_value() {
                Some(v) => values.push(v),
                None => {
                    ok = false;
                    self.recover_in_list();
                }
            }
            match self.peek().map(|t| &t.tok) {
                Some(Tok::Comma) => {
                    self.pos += 1;
                }
                Some(Tok::RParen) => {
                    self.pos += 1;
                    break;
                }
                None => {
                    if ok {
                        self.err(open, ErrorCode::UnclosedParen);
                    }
                    return None;
                }
                Some(_) if self.is_order_start() => {
                    if ok {
                        self.err(open, ErrorCode::UnclosedParen);
                    }
                    return None;
                }
                Some(_) => {
                    if ok {
                        self.expected(Expect::ListSeparator);
                    }
                    ok = false;
                    self.recover_in_list();
                    // an `,` weitermachen, an `)` beenden, sonst (ORDER/Ende) aufgeben
                    match self.peek().map(|t| &t.tok) {
                        Some(Tok::Comma) => self.pos += 1,
                        Some(Tok::RParen) => {
                            self.pos += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
            }
        }
        if !ok {
            return None;
        }
        Some(Expr::In { field, negated, op_loc: Loc(op_span), values })
    }

    /// Ein Wert: Zahl, Datum, Wort, Text, `NULL`/`EMPTY` oder Funktionsaufruf. Meldet Fehler selbst; `None` ohne Überspringen
    /// (außer dass ein unbekanntes Wort verbraucht wird, wenn es ein Name ist).
    fn parse_value(&mut self) -> Option<Value> {
        let Some(t) = self.peek().cloned() else {
            self.expected(Expect::Value);
            return None;
        };
        match &t.tok {
            Tok::Quoted { text, .. } => {
                self.pos += 1;
                Some(Value { kind: ValueKind::Text(text.clone()), loc: Loc(t.span) })
            }
            Tok::Word(w) => match kw_of(w) {
                Some(Kw::Null | Kw::Empty) => {
                    self.pos += 1;
                    Some(Value { kind: ValueKind::Null, loc: Loc(t.span) })
                }
                Some(k) if is_reserved_kw(k) => {
                    self.expected(Expect::Value);
                    None
                }
                _ => {
                    self.pos += 1;
                    if matches!(self.peek().map(|t| &t.tok), Some(Tok::LParen)) {
                        return self.parse_call(Name::at(w.clone(), t.span));
                    }
                    Some(Value { kind: classify_word(w), loc: Loc(t.span) })
                }
            },
            _ => {
                self.expected(Expect::Value);
                None
            }
        }
    }

    /// Funktionsaufruf; Name schon verbraucht, aktuelles Token ist `(`. Verschachtelte Aufrufe zählen wie Klammern zur Tiefe
    /// (sonst läuft `a(a(a(…` bei langer Eingabe über den Stapel).
    fn parse_call(&mut self, name: Name) -> Option<Value> {
        if self.depth >= MAX_DEPTH {
            let open = self.at_span();
            self.err(open, ErrorCode::TooDeep);
            self.pos = self.toks.len();
            return None;
        }
        self.depth += 1;
        let r = self.parse_call_inner(name);
        self.depth -= 1;
        r
    }

    fn parse_call_inner(&mut self, name: Name) -> Option<Value> {
        let open = self.at_span();
        self.pos += 1;
        let mut args = Vec::new();
        let mut ok = true;
        if matches!(self.peek().map(|t| &t.tok), Some(Tok::RParen)) {
            let close = self.at_span();
            self.pos += 1;
            return Some(Value { loc: Loc(name.span().union(close)), kind: ValueKind::Func { name, args } });
        }
        let end;
        loop {
            match self.parse_value() {
                Some(v) => args.push(v),
                None => {
                    ok = false;
                    self.recover_in_list();
                }
            }
            match self.peek().map(|t| (&t.tok, t.span)) {
                Some((Tok::Comma, _)) => self.pos += 1,
                Some((Tok::RParen, s)) => {
                    end = s;
                    self.pos += 1;
                    break;
                }
                None => {
                    if ok {
                        self.err(open, ErrorCode::UnclosedParen);
                    }
                    return None;
                }
                Some(_) => {
                    if ok {
                        self.expected(Expect::ListSeparator);
                    }
                    ok = false;
                    self.recover_in_list();
                    match self.peek().map(|t| (&t.tok, t.span)) {
                        Some((Tok::Comma, _)) => self.pos += 1,
                        Some((Tok::RParen, s)) => {
                            end = s;
                            self.pos += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
            }
        }
        if !ok {
            return None;
        }
        Some(Value { loc: Loc(name.span().union(end)), kind: ValueKind::Func { name, args } })
    }

    // ----- ORDER BY -----

    /// `ORDER BY`/`SORT BY` samt Einträgen; das aktuelle Token ist `ORDER` oder `SORT`.
    fn parse_order_by(&mut self) {
        let first = self.at_span();
        self.pos += 1;
        let mut head = first;
        if self.peek_kw() == Some(Kw::By) {
            head = head.union(self.at_span());
            self.pos += 1;
        } else {
            self.expected(Expect::By);
        }
        let duplicate = self.order_seen;
        if duplicate {
            self.err(head, ErrorCode::DuplicateOrderBy);
        }
        self.order_seen = true;
        loop {
            let term = self.parse_order_term();
            if let Some(t) = term {
                if !duplicate {
                    self.order.push(t);
                }
            } else {
                self.recover_order();
            }
            if matches!(self.peek().map(|t| &t.tok), Some(Tok::Comma)) {
                self.pos += 1;
                continue;
            }
            break;
        }
    }

    fn parse_order_term(&mut self) -> Option<OrderTerm> {
        let Some(t) = self.peek().cloned() else {
            self.expected(Expect::OrderField);
            return None;
        };
        let name = match &t.tok {
            Tok::Word(w) if kw_of(w).is_none_or(|k| !is_reserved_kw(k)) => Name::at(w.clone(), t.span),
            Tok::Quoted { text, .. } => Name::at(text.clone(), t.span),
            _ => {
                self.expected(Expect::OrderField);
                return None;
            }
        };
        self.pos += 1;
        let dir = match self.peek_kw() {
            Some(Kw::Asc) => Some(SortDir::Asc),
            Some(Kw::Desc) => Some(SortDir::Desc),
            _ => None,
        };
        if dir.is_some() {
            self.pos += 1;
        }
        Some(OrderTerm { name, dir })
    }

    /// Nach einem Fehler in `ORDER BY` bis zum nächsten `,` oder einem weiteren `ORDER`/Ende überspringen.
    fn recover_order(&mut self) {
        while let Some(t) = self.peek() {
            if matches!(t.tok, Tok::Comma) || self.is_order_start() {
                return;
            }
            self.pos += 1;
        }
    }
}

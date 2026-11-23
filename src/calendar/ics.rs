//! iCal-Dateien (`.ics`, RFC 5545) für den Kalender (#164): Import und Export lokaler Dateien.
//!
//! **Der Export ist öffentliche API** (Format unten, nur additiv ändern):
//! `VCALENDAR` (`VERSION:2.0`, `PRODID:-//Paneldeck//Calendar <Version>//EN`, `CALSCALE:GREGORIAN`) mit einem
//! `VTIMEZONE` der Rechner-Zeitzone (`TZID` = IANA-Name, `STANDARD`/`DAYLIGHT` mit jährlicher `RRULE` aus den Wechseln
//! des laufenden Jahres) und je eigenem Termin ein `VEVENT` mit `UID`, `DTSTAMP`, `CREATED`, `LAST-MODIFIED`,
//! `DTSTART`/`DTEND` (mit `TZID`; ganztägig `VALUE=DATE`, Ende ausschließlich), `SUMMARY`, `DESCRIPTION` (Notiz),
//! `RRULE` (wenn sich die Wiederholung übersetzen lässt), `EXDATE` (Ausnahmen), `X-PANELDECK-CRON` (der Cron-Ausdruck,
//! immer bei Wiederholungen), `X-PANELDECK-COLOR` (`#RRGGBB`) und `VALARM` (`ACTION:DISPLAY`, `TRIGGER:-PT<n>M`).
//! Zeilen mit CRLF, gefaltet nach 75 Bytes.
//!
//! **Wiederholungen:** Cron → RRULE gelingt für alle Ausdrücke ohne ODER-Fall (Tag und Wochentag beide gesetzt): Minuten-
//! und Stundenschritte als `FREQ=MINUTELY/HOURLY;INTERVAL`, sonst `FREQ=DAILY` (bzw. `WEEKLY`/`MONTHLY`/`YEARLY` bei
//! einfachen Mustern, `MONTHLY` bei `#`) mit `BYMINUTE`/`BYHOUR`/`BYMONTHDAY`/`BYMONTH`/`BYDAY`. Der ODER-Fall hat keine
//! RRULE (nur `X-PANELDECK-CRON`, der Bericht nennt den Termin). RRULE → Cron beim Import: `MINUTELY` (Intervall teilt
//! 60), `HOURLY` (teilt 24), `DAILY`, `WEEKLY`, `MONTHLY` (Intervall teilt 12), `YEARLY` mit `BYMINUTE`, `BYHOUR`,
//! `BYDAY` (auch `1MO` … `5MO` bei `MONTHLY` und `YEARLY` mit `BYMONTH`), `BYMONTHDAY` (positiv), `BYMONTH`, `COUNT`,
//! `UNTIL`. **Grenzen:** Intervalle > 1 bei `DAILY`/`WEEKLY`/`YEARLY`, `BYSETPOS`, `BYWEEKNO`, `BYYEARDAY`, negative
//! Tage (`-1MO`, `BYMONTHDAY=-1`) und `BYMONTHDAY` zusammen mit `BYDAY` lassen sich nicht als Cron schreiben – solche
//! Termine kommen als Einzeltermin (erstes Vorkommen) und stehen im Bericht. `RDATE` wird nicht gelesen.
//!
//! **Import:** nur lokale Dateien bis [`MAX_FILE_BYTES`] (10 MB); doppelte `UID`s (schon vorhanden oder doppelt in der
//! Datei) werden übersprungen, kaputte Einträge einzeln übersprungen und im Bericht genannt. Zeiten mit `Z` werden in die
//! lokale Zeit umgerechnet, mit `TZID` über das `VTIMEZONE` der Datei (oder als lokale Zeit, wenn es die eigene Zone ist
//! bzw. keine Definition dabei ist – dann mit Hinweis). Geänderte Einzelvorkommen (`RECURRENCE-ID`) werden eigene
//! Termine, das Original bekommt an diesem Tag eine Ausnahme.

use super::cron::Cron;
use super::store::{self, EventInput, SRes};
use super::zone::{nth_weekday, DstRule, RuleZone, Transition, Zone};
use super::{Event, Recurrence};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashSet};

/// Größte Datei für den Import.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

const DOW_CODES: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

// ───────────────────────── Text

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            ';' => o.push_str("\\;"),
            ',' => o.push_str("\\,"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => o.push('\n'),
                Some(x) => o.push(x),
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

/// Eine Inhaltszeile nach 75 Bytes falten (Folgezeilen beginnen mit einem Leerzeichen).
fn fold(line: &str, out: &mut String) {
    let mut start = 0;
    let mut limit = 75;
    let b = line.as_bytes();
    while b.len() - start > limit {
        let mut cut = start + limit;
        while !line.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push_str(&line[start..cut]);
        out.push_str("\r\n ");
        start = cut;
        limit = 74;
    }
    out.push_str(&line[start..]);
    out.push_str("\r\n");
}

/// Eine entfaltete Inhaltszeile.
#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
    line: usize,
}

impl Prop {
    fn param(&self, k: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

/// Zeilen entfalten (CRLF oder LF; Folgezeilen mit Leerzeichen/Tab) und zerlegen; die Zeilennummer ist die der ersten Zeile.
fn unfold(text: &str) -> Vec<Prop> {
    let mut logical: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let l = raw.strip_suffix('\r').unwrap_or(raw);
        if (l.starts_with(' ') || l.starts_with('\t')) && !logical.is_empty() {
            logical.last_mut().unwrap().1.push_str(&l[1..]);
        } else if !l.trim().is_empty() {
            logical.push((i + 1, l.to_string()));
        }
    }
    logical.into_iter().filter_map(|(n, l)| parse_line(&l, n)).collect()
}

fn parse_line(l: &str, line: usize) -> Option<Prop> {
    // Name und Parameter bis zum ersten ':' außerhalb von Anführungszeichen
    let mut in_q = false;
    let mut colon = None;
    for (i, c) in l.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ':' if !in_q => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&l[..colon], &l[colon + 1..]);
    let mut parts = Vec::new();
    let mut cur = String::new();
    in_q = false;
    for c in head.chars() {
        match c {
            '"' => in_q = !in_q,
            ';' if !in_q => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.remove(0).trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts.into_iter().filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_string()))).collect();
    Some(Prop { name, params, value: value.to_string(), line })
}

// ───────────────────────── Zeiten

fn fmt_dt(t: NaiveDateTime) -> String {
    t.format("%Y%m%dT%H%M%S").to_string()
}

fn fmt_date(d: NaiveDate) -> String {
    d.format("%Y%m%d").to_string()
}

fn fmt_utc(secs: i64) -> String {
    format!("{}Z", fmt_dt(super::zone::naive_from_secs(secs)))
}

fn fmt_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    format!("{sign}{:02}{:02}", a / 3600, (a % 3600) / 60)
}

fn parse_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if rest.len() < 4 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = rest[0..2].parse().ok()?;
    let m: i32 = rest[2..4].parse().ok()?;
    let sec: i32 = if rest.len() >= 6 { rest[4..6].parse().ok()? } else { 0 };
    Some(sign * (h * 3600 + m * 60 + sec))
}

/// Eine gelesene Zeit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    Date(NaiveDate),
    /// Wanduhrzeit ohne Zone (oder in einer unbekannten bzw. der eigenen Zone).
    Floating(NaiveDateTime),
    Utc(NaiveDateTime),
    Zoned(NaiveDateTime, usize),
}

fn parse_raw(v: &str) -> Option<(NaiveDateTime, bool, bool)> {
    let v = v.trim();
    if v.len() == 8 {
        return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(|d| (d.and_time(NaiveTime::MIN), true, false));
    }
    let (body, utc) = match v.strip_suffix('Z').or_else(|| v.strip_suffix('z')) {
        Some(b) => (b, true),
        None => (v, false),
    };
    NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").or_else(|_| NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M")).ok().map(|t| (t, false, utc))
}

/// Zeitzonen der Datei (`TZID` → Zone; `None` = unbekannt, als lokale Zeit lesen).
struct Zones {
    defs: Vec<(String, Option<RuleZone>)>,
}

impl Zones {
    fn index(&self, tzid: &str) -> Option<usize> {
        self.defs.iter().position(|(n, _)| n == tzid)
    }
}

/// Ergebnis beim Lesen einer Zeit: Wert + ob eine Zone unbekannt war.
fn read_when(p: &Prop, zones: &mut Zones, local: &dyn Zone) -> Option<(When, bool)> {
    let first = p.value.split(',').next()?;
    let (t, is_date, utc) = parse_raw(first)?;
    if is_date || p.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) {
        return Some((When::Date(t.date()), false));
    }
    if utc {
        return Some((When::Utc(t), false));
    }
    match p.param("TZID") {
        None => Some((When::Floating(t), false)),
        Some(id) => {
            let id = id.trim_matches('"');
            if id == local.name() {
                return Some((When::Floating(t), false));
            }
            match zones.index(id) {
                Some(i) if zones.defs[i].1.is_some() => Some((When::Zoned(t, i), false)),
                Some(_) => Some((When::Floating(t), true)),
                None => {
                    zones.defs.push((id.to_string(), None));
                    Some((When::Floating(t), true))
                }
            }
        }
    }
}

/// Lokale Wanduhrzeit einer gelesenen Zeit.
fn to_local(w: When, zones: &Zones, local: &dyn Zone) -> NaiveDateTime {
    match w {
        When::Date(d) => d.and_time(NaiveTime::MIN),
        When::Floating(t) => t,
        When::Utc(t) => local.to_local(super::zone::secs(t)),
        When::Zoned(t, i) => match &zones.defs[i].1 {
            Some(z) => local.to_local(z.instant(t)),
            None => t,
        },
    }
}

/// ISO-8601-Dauer (`P1D`, `PT1H30M`, `P2W`) in Minuten.
fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            u => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (u, in_time) {
                    ('W', false) => n * 7 * 1440,
                    ('D', false) => n * 1440,
                    ('H', true) => n * 60,
                    ('M', true) => n,
                    ('S', true) => n / 60,
                    _ => return None,
                };
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(if neg { -total } else { total })
}

// ───────────────────────── Cron ↔ RRULE

fn join(v: &[u32]) -> String {
    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

fn even_step(v: &[u32], n: u32) -> Option<u32> {
    if v.len() < 2 || v[0] != 0 {
        return None;
    }
    let st = v[1] - v[0];
    (st > 0 && n.is_multiple_of(st) && v.len() as u32 == n / st && v.iter().enumerate().all(|(i, x)| *x == i as u32 * st)).then_some(st)
}

/// RRULE (ohne `UNTIL`/`COUNT`) zu einem Ausdruck; `None` im ODER-Fall. Die Uhrzeit einzelner Termine kommt aus
/// `DTSTART` (= erstes Vorkommen).
pub fn cron_to_rrule(c: &Cron, all_day: bool) -> Option<String> {
    if c.or_case() {
        return None;
    }
    let (mins, hours) = (c.minute_values(), c.hour_values());
    let doms = (!c.every_dom()).then(|| c.dom_values());
    let months = (!c.every_month()).then(|| c.month_values());
    let mut byday: Vec<String> = Vec::new();
    if !c.every_dow() {
        byday.extend(c.dow_values().iter().map(|d| DOW_CODES[*d as usize].to_string()));
        byday.extend(c.nth_values().iter().map(|(d, n)| format!("{n}{}", DOW_CODES[*d as usize])));
    }
    let has_nth = !c.nth_values().is_empty();
    let days_all = doms.is_none() && months.is_none() && byday.is_empty();
    let mut parts: Vec<String> = Vec::new();
    let single_time = all_day || (mins.len() == 1 && hours.len() == 1);
    if !all_day && days_all && hours.len() == 24
        && let Some(st) = even_step(&mins, 60) {
            parts.push("FREQ=MINUTELY".into());
            if st > 1 {
                parts.push(format!("INTERVAL={st}"));
            }
            return Some(parts.join(";"));
        }
    if !all_day && days_all && mins.len() == 1
        && let Some(st) = even_step(&hours, 24) {
            parts.push("FREQ=HOURLY".into());
            if st > 1 {
                parts.push(format!("INTERVAL={st}"));
            }
            return Some(parts.join(";"));
        }
    // Frequenz: einfache Muster lesbar, sonst DAILY (MONTHLY bei n-tem Wochentag)
    let freq = if has_nth {
        "MONTHLY"
    } else if single_time {
        match (&doms, &months, byday.is_empty()) {
            (Some(d), Some(m), true) if d.len() == 1 && m.len() == 1 => "YEARLY",
            (Some(_), None, true) => "MONTHLY",
            (None, None, false) => "WEEKLY",
            _ => "DAILY",
        }
    } else {
        "DAILY"
    };
    parts.push(format!("FREQ={freq}"));
    if let Some(m) = &months {
        parts.push(format!("BYMONTH={}", join(m)));
    }
    if let Some(d) = &doms {
        parts.push(format!("BYMONTHDAY={}", join(d)));
    }
    if !byday.is_empty() {
        parts.push(format!("BYDAY={}", byday.join(",")));
    }
    if !single_time {
        parts.push(format!("BYHOUR={}", join(&hours)));
        parts.push(format!("BYMINUTE={}", join(&mins)));
    }
    Some(parts.join(";"))
}

/// Warum sich eine RRULE nicht übersetzen ließ (zweisprachig).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untranslatable {
    pub de: String,
    pub en: String,
}

fn un(de: impl Into<String>, en: impl Into<String>) -> Untranslatable {
    Untranslatable { de: de.into(), en: en.into() }
}

/// Ergebnis von [`rrule_to_cron`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RruleCron {
    pub cron: String,
    pub count: Option<u32>,
    /// `UNTIL` als roher Wert (Datum oder Zeit), die Umrechnung macht der Aufrufer.
    pub until: Option<String>,
}

/// RRULE → Cron (Grenzen siehe Modulbeschreibung). `start` = `DTSTART` (lokale Wanduhrzeit) für fehlende Teile.
pub fn rrule_to_cron(rrule: &str, start: NaiveDateTime, all_day: bool) -> Result<RruleCron, Untranslatable> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for part in rrule.split(';').filter(|p| !p.trim().is_empty()) {
        let (k, v) = part.split_once('=').ok_or_else(|| un(format!("RRULE-Teil „{part}“ ohne Wert"), format!("RRULE part “{part}” without a value")))?;
        map.insert(k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase());
    }
    for k in ["BYSETPOS", "BYWEEKNO", "BYYEARDAY"] {
        if map.contains_key(k) {
            return Err(un(format!("{k} gibt es in Cron nicht"), format!("{k} has no cron equivalent")));
        }
    }
    if map.get("BYSECOND").is_some_and(|s| s != "0") {
        return Err(un("Sekunden gibt es in Cron nicht", "cron has no seconds"));
    }
    let freq = map.get("FREQ").cloned().ok_or_else(|| un("FREQ fehlt", "FREQ is missing"))?;
    let interval: u32 = match map.get("INTERVAL") {
        Some(v) => v.parse().ok().filter(|n| *n >= 1).ok_or_else(|| un("ungültiges INTERVAL", "invalid INTERVAL"))?,
        None => 1,
    };
    let nums = |k: &str, lo: i64, hi: i64| -> Result<Option<Vec<u32>>, Untranslatable> {
        let Some(v) = map.get(k) else { return Ok(None) };
        let mut out = Vec::new();
        for x in v.split(',') {
            let n: i64 = x.trim().parse().map_err(|_| un(format!("{k}: „{x}“ ist keine Zahl"), format!("{k}: “{x}” is not a number")))?;
            if n < 0 {
                return Err(un(format!("{k}: negative Werte (vom Ende gezählt) gibt es in Cron nicht"), format!("{k}: negative values (counted from the end) have no cron equivalent")));
            }
            if n < lo || n > hi {
                return Err(un(format!("{k}: {n} außerhalb von {lo}–{hi}"), format!("{k}: {n} outside {lo}-{hi}")));
            }
            out.push(n as u32);
        }
        Ok(Some(out))
    };
    let by_min = nums("BYMINUTE", 0, 59)?;
    let by_hour = nums("BYHOUR", 0, 23)?;
    let by_mday = nums("BYMONTHDAY", 1, 31)?;
    let by_month = nums("BYMONTH", 1, 12)?;
    let mut by_day: Vec<String> = Vec::new();
    let mut has_nth = false;
    if let Some(v) = map.get("BYDAY") {
        for x in v.split(',') {
            let x = x.trim();
            if x.len() < 2 {
                return Err(un(format!("BYDAY: „{x}“"), format!("BYDAY: “{x}”")));
            }
            let (n, code) = x.split_at(x.len() - 2);
            let d = DOW_CODES.iter().position(|c| *c == code).ok_or_else(|| un(format!("BYDAY: unbekannter Tag „{x}“"), format!("BYDAY: unknown day “{x}”")))?;
            let name = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"][d];
            if n.is_empty() {
                by_day.push(name.into());
            } else {
                let n: i32 = n.trim_start_matches('+').parse().map_err(|_| un(format!("BYDAY: „{x}“"), format!("BYDAY: “{x}”")))?;
                if !(1..=5).contains(&n) {
                    return Err(un("BYDAY: nur der 1. bis 5. Wochentag (vom Monatsanfang) geht als Cron", "BYDAY: only the 1st to 5th weekday (from the start of the month) works in cron"));
                }
                has_nth = true;
                by_day.push(format!("{name}#{n}"));
            }
        }
    }
    if by_mday.is_some() && !by_day.is_empty() {
        return Err(un("BYMONTHDAY zusammen mit BYDAY (Schnittmenge) gibt es in Cron nicht", "BYMONTHDAY together with BYDAY (intersection) has no cron equivalent"));
    }
    let minute_default = || if all_day { "0".to_string() } else { start.minute().to_string() };
    let hour_default = || if all_day { "0".to_string() } else { start.hour().to_string() };
    let list = |v: &Option<Vec<u32>>| v.as_ref().map(|v| join(v));
    let (mut minute, mut hour, mut dom, mut month, mut dow) = (list(&by_min).unwrap_or_else(minute_default), list(&by_hour).unwrap_or_else(hour_default), list(&by_mday), list(&by_month), (!by_day.is_empty()).then(|| by_day.join(",")));
    let step = |interval: u32, n: u32, first: u32, base: u32| -> Result<String, Untranslatable> {
        if !n.is_multiple_of(interval) {
            return Err(un(format!("INTERVAL={interval} teilt {n} nicht"), format!("INTERVAL={interval} does not divide {n}")));
        }
        Ok(if interval == 1 { "*".into() } else { format!("{}/{interval}", (first - base) % interval + base) })
    };
    let only_one = |what: &str| -> Result<(), Untranslatable> {
        if interval > 1 {
            Err(un(format!("{what} mit INTERVAL > 1 gibt es in Cron nicht"), format!("{what} with INTERVAL > 1 has no cron equivalent")))
        } else {
            Ok(())
        }
    };
    if has_nth && !(freq == "MONTHLY" || (freq == "YEARLY" && by_month.is_some())) {
        return Err(un("ein n-ter Wochentag geht nur monatlich (oder jährlich mit BYMONTH)", "an n-th weekday only works monthly (or yearly with BYMONTH)"));
    }
    match freq.as_str() {
        "MINUTELY" => {
            if by_min.is_none() {
                minute = step(interval, 60, start.minute(), 0)?;
            } else {
                only_one("MINUTELY")?;
            }
            if by_hour.is_none() {
                hour = "*".into();
            }
        }
        "HOURLY" => {
            if by_hour.is_none() {
                hour = step(interval, 24, start.hour(), 0)?;
            } else {
                only_one("HOURLY")?;
            }
        }
        "DAILY" => only_one("DAILY")?,
        "WEEKLY" => {
            only_one("WEEKLY")?;
            if dow.is_none() {
                dow = Some(["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"][start.weekday().num_days_from_sunday() as usize].into());
            }
        }
        "MONTHLY" => {
            if interval > 1 {
                if month.is_some() {
                    return Err(un("MONTHLY mit INTERVAL und BYMONTH gibt es in Cron nicht", "MONTHLY with INTERVAL and BYMONTH has no cron equivalent"));
                }
                month = Some(step(interval, 12, start.month(), 1)?);
            }
            if dom.is_none() && dow.is_none() {
                dom = Some(start.day().to_string());
            }
        }
        "YEARLY" => {
            only_one("YEARLY")?;
            if month.is_none() {
                month = Some(start.month().to_string());
            }
            if dom.is_none() && dow.is_none() {
                dom = Some(start.day().to_string());
            }
        }
        f => return Err(un(format!("FREQ={f} gibt es in Cron nicht"), format!("FREQ={f} has no cron equivalent"))),
    }
    let expr = format!("{minute} {hour} {} {} {}", dom.unwrap_or_else(|| "*".into()), month.unwrap_or_else(|| "*".into()), dow.unwrap_or_else(|| "*".into()));
    let c = Cron::parse(&expr).map_err(|e| un(e.de, e.en))?;
    let count = match map.get("COUNT") {
        Some(v) => Some(v.parse::<u32>().ok().filter(|n| *n >= 1 && *n <= super::MAX_COUNT).ok_or_else(|| un("ungültiges COUNT", "invalid COUNT"))?),
        None => None,
    };
    Ok(RruleCron { cron: c.text, count, until: map.get("UNTIL").cloned() })
}

// ───────────────────────── Export

/// Bericht des Exports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub events: usize,
    /// Titel der Termine, deren Wiederholung keine RRULE hat (nur `X-PANELDECK-CRON`).
    pub without_rrule: Vec<String>,
}

fn vtimezone(zone: &dyn Zone, year: i32, out: &mut String) {
    fold("BEGIN:VTIMEZONE", out);
    fold(&format!("TZID:{}", zone.name()), out);
    let tr = super::zone::transitions(zone, year);
    if tr.is_empty() {
        let off = zone.offset_at(super::zone::secs(NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or_default().and_time(NaiveTime::MIN)));
        for l in ["BEGIN:STANDARD", "DTSTART:19700101T000000", &format!("TZOFFSETFROM:{}", fmt_offset(off)), &format!("TZOFFSETTO:{}", fmt_offset(off)), "END:STANDARD"] {
            fold(l, out);
        }
    }
    for (utc, from, to) in tr {
        let kind = if to > from { "DAYLIGHT" } else { "STANDARD" };
        let local = super::zone::naive_from_secs(utc + from as i64);
        let d = local.date();
        let last = super::zone::last_day(d.year(), d.month()).map(|l| l.day()).unwrap_or(31);
        let nth: i32 = if d.day() + 7 > last { -1 } else { ((d.day() - 1) / 7 + 1) as i32 };
        fold(&format!("BEGIN:{kind}"), out);
        fold(&format!("DTSTART:{}", fmt_dt(local)), out);
        fold(&format!("TZOFFSETFROM:{}", fmt_offset(from)), out);
        fold(&format!("TZOFFSETTO:{}", fmt_offset(to)), out);
        fold(&format!("RRULE:FREQ=YEARLY;BYMONTH={};BYDAY={nth}{}", d.month(), DOW_CODES[d.weekday().num_days_from_sunday() as usize]), out);
        fold(&format!("END:{kind}"), out);
    }
    fold("END:VTIMEZONE", out);
}

fn stored_to_utc(s: &str, zone: &dyn Zone) -> String {
    match super::parse_when(s) {
        Some((t, _)) => fmt_utc(zone.instant(t)),
        None => fmt_utc(0),
    }
}

/// Termine als iCal-Text (öffentliches Format, siehe Modulbeschreibung). `now` = Unix-Sekunden für `DTSTAMP`.
pub fn export(events: &[Event], zone: &dyn Zone, now: i64) -> (String, ExportReport) {
    let mut out = String::new();
    let mut rep = ExportReport::default();
    let tzid = zone.name();
    for l in ["BEGIN:VCALENDAR", "VERSION:2.0", &format!("PRODID:-//Paneldeck//Calendar {}//EN", env!("CARGO_PKG_VERSION")), "CALSCALE:GREGORIAN"] {
        fold(l, &mut out);
    }
    vtimezone(zone, zone.to_local(now).year(), &mut out);
    let stamp = fmt_utc(now);
    for e in events {
        rep.events += 1;
        // DTSTART = erstes Vorkommen (iCal zählt DTSTART immer mit)
        let mut no_exc = e.clone();
        if let Some(r) = &mut no_exc.recur {
            r.except.clear();
        }
        let parsed = e.recur.as_ref().and_then(|r| Cron::parse(&r.cron).ok());
        let first = match &parsed {
            Some(_) => no_exc.next_occurrences(e.start, 1, zone).first().copied(),
            None => Some(e.start),
        };
        let start = first.unwrap_or(e.start);
        fold("BEGIN:VEVENT", &mut out);
        fold(&format!("UID:{}", escape(&e.uid)), &mut out);
        fold(&format!("DTSTAMP:{stamp}"), &mut out);
        fold(&format!("CREATED:{}", stored_to_utc(&e.created_at, zone)), &mut out);
        fold(&format!("LAST-MODIFIED:{}", stored_to_utc(&e.updated_at, zone)), &mut out);
        if e.all_day {
            fold(&format!("DTSTART;VALUE=DATE:{}", fmt_date(start.date())), &mut out);
            fold(&format!("DTEND;VALUE=DATE:{}", fmt_date(start.date() + Duration::days(e.days()))), &mut out);
        } else {
            fold(&format!("DTSTART;TZID={tzid}:{}", fmt_dt(start)), &mut out);
            fold(&format!("DTEND;TZID={tzid}:{}", fmt_dt(e.end_of(start))), &mut out);
        }
        fold(&format!("SUMMARY:{}", escape(&e.title)), &mut out);
        if !e.note.is_empty() {
            fold(&format!("DESCRIPTION:{}", escape(&e.note)), &mut out);
        }
        if let (Some(r), Some(c)) = (&e.recur, &parsed) {
            match cron_to_rrule(c, e.all_day).filter(|_| first.is_some()) {
                Some(mut rule) => {
                    if let Some(u) = r.until {
                        if e.all_day {
                            rule.push_str(&format!(";UNTIL={}", fmt_date(u)));
                        } else {
                            let last = u.and_hms_opt(23, 59, 59).unwrap_or_default();
                            rule.push_str(&format!(";UNTIL={}", fmt_utc(zone.instant(last))));
                        }
                    }
                    if let Some(n) = r.count {
                        rule.push_str(&format!(";COUNT={n}"));
                    }
                    fold(&format!("RRULE:{rule}"), &mut out);
                }
                None => rep.without_rrule.push(e.title.clone()),
            }
            fold(&format!("X-PANELDECK-CRON:{}", escape(&r.cron)), &mut out);
            if r.until.is_some() || r.count.is_some() {
                let mut x = Vec::new();
                if let Some(u) = r.until {
                    x.push(format!("UNTIL={}", fmt_date(u)));
                }
                if let Some(n) = r.count {
                    x.push(format!("COUNT={n}"));
                }
                fold(&format!("X-PANELDECK-RANGE:{}", x.join(";")), &mut out);
            }
            for d in &r.except {
                if e.all_day {
                    fold(&format!("EXDATE;VALUE=DATE:{}", fmt_date(*d)), &mut out);
                } else {
                    let day0 = d.and_time(NaiveTime::MIN);
                    let times: Vec<String> = no_exc.occurrences(day0, day0 + Duration::days(1), zone).into_iter().filter(|s| s.date() == *d).map(fmt_dt).collect();
                    if !times.is_empty() {
                        fold(&format!("EXDATE;TZID={tzid}:{}", times.join(",")), &mut out);
                    }
                    fold(&format!("X-PANELDECK-EXCEPT:{}", fmt_date(*d)), &mut out);
                }
            }
        }
        if !e.color.is_empty() {
            fold(&format!("X-PANELDECK-COLOR:{}", e.color), &mut out);
        }
        if let Some(m) = e.reminder {
            for l in ["BEGIN:VALARM", "ACTION:DISPLAY", &format!("DESCRIPTION:{}", escape(&e.title)), &format!("TRIGGER:-PT{m}M"), "END:VALARM"] {
                fold(l, &mut out);
            }
        }
        fold("END:VEVENT", &mut out);
    }
    fold("END:VCALENDAR", &mut out);
    (out, rep)
}

// ───────────────────────── Import

/// Ein übersprungener (kaputter) Eintrag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Zeile von `BEGIN:VEVENT` (ab 1).
    pub line: usize,
    pub title: String,
    pub de: String,
    pub en: String,
}

/// Ein Hinweis zu einem importierten Termin (Wiederholung nicht übersetzt, unbekannte Zeitzone …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub de: String,
    pub en: String,
}

/// Bericht des Imports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    /// Titel der übersprungenen Doppelten (gleiche `UID`).
    pub duplicates: Vec<String>,
    pub skipped: Vec<Skipped>,
    pub notes: Vec<Note>,
}

impl ImportReport {
    /// Zusammenfassung in einer Zeile.
    pub fn summary(&self, lang: crate::state::Lang) -> String {
        match lang {
            crate::state::Lang::De => format!("{} importiert, {} doppelt übersprungen, {} fehlerhaft übersprungen, {} Hinweise", self.imported, self.duplicates.len(), self.skipped.len(), self.notes.len()),
            crate::state::Lang::En => format!("{} imported, {} duplicates skipped, {} broken entries skipped, {} notes", self.imported, self.duplicates.len(), self.skipped.len(), self.notes.len()),
        }
    }
}

/// Ein gelesener Termin, bereit zum Anlegen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub uid: String,
    pub input: EventInput,
}

fn untitled(title: &str) -> String {
    if title.trim().is_empty() { "(untitled)".into() } else { title.to_string() }
}

/// Stabile Ersatz-`UID` für Einträge ohne `UID` (gleiche Datei → gleiche Kennung, Doppelte werden erkannt).
fn fallback_uid(props: &[Prop]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in props.iter().filter(|p| matches!(p.name.as_str(), "SUMMARY" | "DTSTART" | "DTEND" | "RRULE" | "DESCRIPTION")) {
        for b in p.name.bytes().chain(p.value.bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("import-{h:016x}@paneldeck")
}

/// Eine Beobachtung eines `VTIMEZONE`: Sommerzeit?, `DTSTART`, `TZOFFSETTO`, `RRULE`.
type Observance = (bool, Option<NaiveDateTime>, Option<i32>, Option<String>);

fn vtimezone_zone(id: &str, props: &[Prop]) -> Option<RuleZone> {
    // Beobachtungen: (Art, DTSTART, TZOFFSETTO, RRULE)
    let mut obs: Vec<Observance> = Vec::new();
    let mut cur: Option<Observance> = None;
    for p in props {
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "STANDARD") => cur = Some((false, None, None, None)),
            ("BEGIN", "DAYLIGHT") => cur = Some((true, None, None, None)),
            ("END", "STANDARD") | ("END", "DAYLIGHT") => obs.extend(cur.take()),
            ("DTSTART", _) => {
                if let Some(c) = &mut cur {
                    c.1 = parse_raw(&p.value).map(|x| x.0);
                }
            }
            ("TZOFFSETTO", _) => {
                if let Some(c) = &mut cur {
                    c.2 = parse_offset(&p.value);
                }
            }
            ("RRULE", _) => {
                if let Some(c) = &mut cur {
                    c.3 = Some(p.value.to_ascii_uppercase());
                }
            }
            _ => {}
        }
    }
    // jeweils die letzte (neueste) Beobachtung jeder Art
    let pick = |dst: bool| obs.iter().filter(|o| o.0 == dst && o.2.is_some()).max_by_key(|o| o.1).cloned();
    let std = pick(false);
    let dst = pick(true);
    let trans = |o: &Observance| -> Option<Transition> {
        let start = o.1?;
        let (mut month, mut nth, mut wd) = (start.month(), 0i32, start.weekday());
        if let Some(r) = &o.3 {
            for part in r.split(';') {
                match part.split_once('=') {
                    Some(("BYMONTH", v)) => month = v.parse().ok()?,
                    Some(("BYDAY", v)) => {
                        let (n, code) = v.split_at(v.len().checked_sub(2)?);
                        nth = n.trim_start_matches('+').parse().ok()?;
                        let d = DOW_CODES.iter().position(|c| *c == code)?;
                        wd = [Weekday::Sun, Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat][d];
                    }
                    _ => {}
                }
            }
        }
        if nth == 0 {
            let last = super::zone::last_day(start.year(), start.month())?.day();
            nth = if start.day() + 7 > last { -1 } else { ((start.day() - 1) / 7 + 1) as i32 };
        }
        nth_weekday(2000, month, wd, nth)?;
        Some(Transition { month, nth, weekday: wd, at_secs: start.num_seconds_from_midnight() as i32 })
    };
    match (std, dst) {
        (Some(s), Some(d)) => Some(RuleZone { name: id.into(), std: s.2?, dst: d.2?, rule: DstRule::Custom { start: trans(&d)?, end: trans(&s)? } }),
        (Some(s), None) => Some(RuleZone::fixed(id, s.2?)),
        (None, Some(d)) => Some(RuleZone::fixed(id, d.2?)),
        (None, None) => None,
    }
}

/// Liest iCal-Text. `exists(uid)` meldet schon vorhandene Termine (werden übersprungen). Reine Funktion.
pub fn parse(text: &str, exists: &dyn Fn(&str) -> bool, local: &dyn Zone) -> (Vec<Parsed>, ImportReport) {
    let props = unfold(text);
    let mut rep = ImportReport::default();
    // Komponenten zerlegen: VTIMEZONE und VEVENT (verschachtelte VALARM gehören zum VEVENT)
    let mut zones = Zones { defs: Vec::new() };
    let mut events: Vec<(usize, Vec<Prop>, bool)> = Vec::new(); // (Zeile, Eigenschaften, vollständig)
    let mut i = 0;
    let mut saw_calendar = false;
    while i < props.len() {
        let p = &props[i];
        if p.name == "BEGIN" && p.value.eq_ignore_ascii_case("VCALENDAR") {
            saw_calendar = true;
        }
        if p.name == "BEGIN" && (p.value.eq_ignore_ascii_case("VTIMEZONE") || p.value.eq_ignore_ascii_case("VEVENT")) {
            let comp = p.value.to_ascii_uppercase();
            let line = p.line;
            let mut body = Vec::new();
            let mut depth = 0;
            let mut closed = false;
            i += 1;
            while i < props.len() {
                let q = &props[i];
                if q.name == "BEGIN" {
                    if q.value.eq_ignore_ascii_case("VEVENT") || q.value.eq_ignore_ascii_case("VTIMEZONE") || q.value.eq_ignore_ascii_case("VCALENDAR") {
                        break; // neues Objekt ohne END: dieses ist kaputt
                    }
                    depth += 1;
                } else if q.name == "END" {
                    if depth == 0 && q.value.eq_ignore_ascii_case(&comp) {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if depth == 0 && q.value.eq_ignore_ascii_case("VCALENDAR") {
                        break;
                    }
                    depth -= 1;
                }
                body.push(q.clone());
                i += 1;
            }
            if comp == "VTIMEZONE" {
                if let Some(id) = body.iter().find(|q| q.name == "TZID").map(|q| q.value.trim().to_string()) {
                    let z = vtimezone_zone(&id, &body);
                    zones.defs.retain(|(n, _)| *n != id);
                    zones.defs.push((id, z));
                }
            } else {
                events.push((line, body, closed));
            }
            continue;
        }
        i += 1;
    }
    if !saw_calendar && events.is_empty() {
        rep.skipped.push(Skipped { line: 1, title: String::new(), de: "Keine iCal-Datei (BEGIN:VCALENDAR fehlt).".into(), en: "Not an iCal file (BEGIN:VCALENDAR is missing).".into() });
        return (vec![], rep);
    }
    let mut out: Vec<Parsed> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Ausnahmen aus RECURRENCE-ID für Originale in derselben Datei (UID → Tage)
    let mut overrides: Vec<(String, NaiveDate)> = Vec::new();
    for (line, body, closed) in events {
        let top: Vec<Prop> = top_level(&body);
        let title = untitled(&top.iter().find(|p| p.name == "SUMMARY").map(|p| unescape(&p.value)).unwrap_or_default());
        let skip = |rep: &mut ImportReport, de: &str, en: &str| rep.skipped.push(Skipped { line, title: title.clone(), de: de.into(), en: en.into() });
        if !closed {
            skip(&mut rep, "END:VEVENT fehlt", "END:VEVENT is missing");
            continue;
        }
        if top.iter().any(|p| p.name == "STATUS" && p.value.eq_ignore_ascii_case("CANCELLED")) {
            skip(&mut rep, "abgesagt (STATUS:CANCELLED)", "cancelled (STATUS:CANCELLED)");
            continue;
        }
        let Some(dtstart) = top.iter().find(|p| p.name == "DTSTART") else {
            skip(&mut rep, "DTSTART fehlt", "DTSTART is missing");
            continue;
        };
        let Some((sw, unknown_tz)) = read_when(dtstart, &mut zones, local) else {
            skip(&mut rep, &format!("DTSTART unlesbar ({})", dtstart.value), &format!("DTSTART unreadable ({})", dtstart.value));
            continue;
        };
        let all_day = matches!(sw, When::Date(_));
        let start = to_local(sw, &zones, local);
        let mut minutes = if all_day { 1440 } else { 0 };
        if let Some(p) = top.iter().find(|p| p.name == "DTEND") {
            match read_when(p, &mut zones, local) {
                Some((ew, _)) => {
                    let end = to_local(ew, &zones, local);
                    minutes = (end - start).num_minutes();
                }
                None => {
                    skip(&mut rep, &format!("DTEND unlesbar ({})", p.value), &format!("DTEND unreadable ({})", p.value));
                    continue;
                }
            }
        } else if let Some(p) = top.iter().find(|p| p.name == "DURATION") {
            match parse_duration(&p.value) {
                Some(m) => minutes = m,
                None => {
                    skip(&mut rep, &format!("DURATION unlesbar ({})", p.value), &format!("DURATION unreadable ({})", p.value));
                    continue;
                }
            }
        }
        if minutes < 0 {
            skip(&mut rep, "Ende liegt vor dem Beginn", "the end is before the start");
            continue;
        }
        if minutes > super::MAX_MINUTES {
            skip(&mut rep, "länger als 366 Tage", "longer than 366 days");
            continue;
        }
        let mut uid = top.iter().find(|p| p.name == "UID").map(|p| unescape(p.value.trim())).filter(|u| !u.is_empty()).unwrap_or_else(|| fallback_uid(&top));
        if let Some(rid) = top.iter().find(|p| p.name == "RECURRENCE-ID")
            && let Some((w, _)) = read_when(rid, &mut zones, local) {
                let d = to_local(w, &zones, local).date();
                overrides.push((uid.clone(), d));
                uid = format!("{uid}/{}", fmt_date(d));
            }
        if uid.chars().count() > 255 {
            skip(&mut rep, "UID ist zu lang", "UID is too long");
            continue;
        }
        if seen.contains(&uid) || exists(&uid) {
            rep.duplicates.push(title.clone());
            continue;
        }
        let note = top.iter().find(|p| p.name == "DESCRIPTION").map(|p| unescape(&p.value)).unwrap_or_default();
        let color = top.iter().find(|p| p.name == "X-PANELDECK-COLOR").map(|p| p.value.trim().to_string()).filter(|c| store::is_hex_color(c)).unwrap_or_default();
        let mut input = EventInput::new(&title, start, minutes);
        input.all_day = all_day;
        input.note = note.chars().take(super::MAX_NOTE).collect();
        input.color = color;
        input.title = title.chars().take(super::MAX_TITLE).collect();
        input.reminder = alarm_minutes(&body);
        if unknown_tz {
            let id = dtstart.param("TZID").unwrap_or("").trim_matches('"').to_string();
            rep.notes.push(Note { title: title.clone(), de: format!("Zeitzone „{id}“ unbekannt – als lokale Zeit gelesen"), en: format!("time zone “{id}” unknown – read as local time") });
        }
        // Wiederholung: eigener Cron-Ausdruck geht vor, sonst RRULE übersetzen
        let x_cron = top.iter().find(|p| p.name == "X-PANELDECK-CRON").map(|p| unescape(p.value.trim()));
        let rrule = top.iter().find(|p| p.name == "RRULE").map(|p| p.value.trim().to_string());
        let mut rec: Option<Recurrence> = None;
        if let Some(c) = x_cron.as_ref().and_then(|c| Cron::parse(c).ok()) {
            let mut r = Recurrence { cron: c.text, ..Default::default() };
            if let Some(x) = top.iter().find(|p| p.name == "X-PANELDECK-RANGE") {
                for part in x.value.split(';') {
                    match part.split_once('=') {
                        Some(("UNTIL", v)) => r.until = parse_raw(v).map(|t| t.0.date()),
                        Some(("COUNT", v)) => r.count = v.parse().ok().filter(|n| *n >= 1 && *n <= super::MAX_COUNT),
                        _ => {}
                    }
                }
            } else if let Some(rr) = &rrule {
                // ältere/fremde Datei: Bereich aus der RRULE
                if let Ok(t) = rrule_to_cron(rr, start, all_day) {
                    r.count = t.count;
                    r.until = t.until.as_deref().and_then(|u| until_date(u, &zones, local));
                }
            }
            rec = Some(r);
        } else if let Some(rr) = &rrule {
            match rrule_to_cron(rr, start, all_day) {
                Ok(t) => rec = Some(Recurrence { cron: t.cron, count: t.count, until: t.until.as_deref().and_then(|u| until_date(u, &zones, local)), except: vec![] }),
                Err(why) => rep.notes.push(Note { title: title.clone(), de: format!("Wiederholung nicht übersetzt ({}) – nur das erste Vorkommen importiert", why.de), en: format!("recurrence not translated ({}) – only the first occurrence imported", why.en) }),
            }
        }
        if let Some(r) = &mut rec {
            let mut exc: Vec<NaiveDate> = Vec::new();
            for p in top.iter().filter(|p| p.name == "EXDATE") {
                for v in p.value.split(',') {
                    let one = Prop { value: v.to_string(), ..p.clone() };
                    if let Some((w, _)) = read_when(&one, &mut zones, local) {
                        exc.push(to_local(w, &zones, local).date());
                    }
                }
            }
            for p in top.iter().filter(|p| p.name == "X-PANELDECK-EXCEPT") {
                exc.extend(parse_raw(&p.value).map(|t| t.0.date()));
            }
            r.except = exc;
            if r.until.is_some_and(|u| u < start.date()) {
                r.until = Some(start.date());
            }
        }
        input.recur = rec;
        match input.clone().validate() {
            Ok(v) => {
                seen.insert(uid.clone());
                out.push(Parsed { uid, input: v });
            }
            Err(e) => {
                let (de, en) = match e {
                    store::StoreError::Invalid { de, en } => (de, en),
                    other => (other.message(crate::state::Lang::De), other.message(crate::state::Lang::En)),
                };
                rep.skipped.push(Skipped { line, title: title.clone(), de, en });
            }
        }
    }
    // Ausnahmen der geänderten Einzelvorkommen beim Original eintragen
    for (uid, d) in overrides {
        if let Some(p) = out.iter_mut().find(|p| p.uid == uid)
            && let Some(r) = &mut p.input.recur
                && r.except.binary_search(&d).is_err() {
                    r.except.push(d);
                    r.except.sort();
                }
    }
    (out, rep)
}

/// Eigenschaften der obersten Ebene eines VEVENT (ohne VALARM).
fn top_level(body: &[Prop]) -> Vec<Prop> {
    let mut depth = 0;
    let mut out = Vec::new();
    for p in body {
        match p.name.as_str() {
            "BEGIN" => depth += 1,
            "END" => depth -= 1,
            _ if depth == 0 => out.push(p.clone()),
            _ => {}
        }
    }
    out
}

/// Minuten vor dem Beginn aus dem ersten `VALARM` mit relativem `TRIGGER` vor dem Start.
fn alarm_minutes(body: &[Prop]) -> Option<i64> {
    let mut in_alarm = false;
    for p in body {
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VALARM") => in_alarm = true,
            ("END", "VALARM") => in_alarm = false,
            ("TRIGGER", v) if in_alarm => {
                if p.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE-TIME")) || p.param("RELATED").is_some_and(|x| x.eq_ignore_ascii_case("END")) {
                    continue;
                }
                let m = parse_duration(v)?;
                if m <= 0 && -m <= super::MAX_REMINDER {
                    return Some(-m);
                }
            }
            _ => {}
        }
    }
    None
}

fn until_date(v: &str, zones: &Zones, local: &dyn Zone) -> Option<NaiveDate> {
    let (t, is_date, utc) = parse_raw(v)?;
    Some(if is_date {
        t.date()
    } else if utc {
        to_local(When::Utc(t), zones, local).date()
    } else {
        t.date()
    })
}

/// Fehler beim Import einer Datei.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    TooLarge(u64),
    Read(String),
    NotUtf8,
    Db(String),
}

impl FileError {
    pub fn message(&self, lang: crate::state::Lang) -> String {
        match (self, lang) {
            (FileError::TooLarge(n), crate::state::Lang::De) => format!("Die Datei ist zu groß ({} MB, höchstens 10 MB).", n / (1024 * 1024)),
            (FileError::TooLarge(n), crate::state::Lang::En) => format!("The file is too large ({} MB, at most 10 MB).", n / (1024 * 1024)),
            (FileError::Read(e), l) => format!("{}: {e}", l.t("Datei nicht lesbar", "Could not read the file")),
            (FileError::NotUtf8, l) => l.t("Die Datei ist kein UTF-8-Text.", "The file is not UTF-8 text.").into(),
            (FileError::Db(e), l) => format!("{}: {e}", l.t("Datenbankfehler", "Database error")),
        }
    }
}

/// Liest Bytes einer `.ics`-Datei als Text (UTF-8, BOM erlaubt).
pub fn decode(bytes: &[u8]) -> Result<String, FileError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(FileError::TooLarge(bytes.len() as u64));
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(b.to_vec()).map_err(|_| FileError::NotUtf8)
}

/// Importiert eine lokale Datei in die Datenbank (eine Transaktion).
pub fn import_file(conn: &Connection, path: &std::path::Path, zone: &dyn Zone) -> Result<ImportReport, FileError> {
    let meta = std::fs::metadata(path).map_err(|e| FileError::Read(e.to_string()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge(meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| FileError::Read(e.to_string()))?;
    let text = decode(&bytes)?;
    import_text(conn, &text, zone)
}

/// Importiert iCal-Text in die Datenbank.
pub fn import_text(conn: &Connection, text: &str, zone: &dyn Zone) -> Result<ImportReport, FileError> {
    let (items, mut rep) = parse(text, &|u| store::uid_exists(conn, u).unwrap_or(false), zone);
    conn.execute_batch("SAVEPOINT cal_import").map_err(|e| FileError::Db(e.to_string()))?;
    for p in items {
        match store::create_event(conn, p.input, Some(&p.uid), "import") {
            Ok(_) => rep.imported += 1,
            Err(e) => rep.skipped.push(Skipped { line: 0, title: p.uid.clone(), de: e.message(crate::state::Lang::De), en: e.message(crate::state::Lang::En) }),
        }
    }
    conn.execute_batch("RELEASE cal_import").map_err(|e| FileError::Db(e.to_string()))?;
    Ok(rep)
}

/// Exportiert alle Termine in eine Datei.
pub fn export_file(conn: &Connection, path: &std::path::Path, zone: &dyn Zone) -> SRes<ExportReport> {
    let events = store::all_events(conn)?;
    let (text, rep) = export(&events, zone, chrono::Utc::now().timestamp());
    crate::state::write_atomic(path, text.as_bytes()).map_err(|e| store::StoreError::Db(e.to_string()))?;
    Ok(rep)
}

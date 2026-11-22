//! Baukasten für Wiederholungen (#164): täglich, werktags, wöchentlich an Tagen, monatlich am n-ten Tag bzw. am n-ten
//! Wochentag, jährlich, alle X Minuten/Stunden → Cron-Ausdruck. Erzeugt nur eindeutige Ausdrücke (nie den ODER-Fall von
//! Tag und Wochentag; Minuten-/Stundenschritte nur als Teiler von 60 bzw. 24, damit die Abstände gleich bleiben) und
//! erkennt eigene Ausdrücke wieder ([`recognise`]), damit der Dialog sie als Baukasten zeigen kann.

use super::cron::Cron;
use crate::state::Lang;

/// Erlaubte Schritte für „alle X Minuten“ (Teiler von 60).
pub const MINUTE_STEPS: [u32; 11] = [1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30];
/// Erlaubte Schritte für „alle X Stunden“ (Teiler von 24).
pub const HOUR_STEPS: [u32; 7] = [1, 2, 3, 4, 6, 8, 12];

const DAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Ein Muster des Baukastens. Wochentage als Bits (Sonntag = Bit 0 … Samstag = Bit 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    Daily,
    /// Montag bis Freitag.
    Weekdays,
    Weekly(u8),
    /// Am Tag `n` jedes Monats (Monate ohne diesen Tag fallen aus).
    MonthlyDay(u32),
    /// Am `n`-ten (1..=5) Wochentag `wd` (0 = Sonntag) jedes Monats.
    MonthlyNth(u8, u8),
    Yearly { month: u32, day: u32 },
    EveryMinutes(u32),
    EveryHours(u32),
}

/// Ungültige Eingabe im Baukasten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    pub de: &'static str,
    pub en: &'static str,
}

fn bad(de: &'static str, en: &'static str) -> BuildError {
    BuildError { de, en }
}

/// Cron-Ausdruck zu einem Muster; `hour`/`minute` = Uhrzeit der Termine (bei „alle X Minuten“ ohne Bedeutung, bei „alle
/// X Stunden“ zählt nur die Minute).
pub fn to_cron(p: Pattern, hour: u32, minute: u32) -> Result<String, BuildError> {
    if hour > 23 || minute > 59 {
        return Err(bad("Ungültige Uhrzeit.", "Invalid time."));
    }
    Ok(match p {
        Pattern::Daily => format!("{minute} {hour} * * *"),
        Pattern::Weekdays => format!("{minute} {hour} * * MON-FRI"),
        Pattern::Weekly(bits) => {
            let days: Vec<&str> = (0..7).filter(|i| bits & (1 << i) != 0).map(|i| DAY_NAMES[i]).collect();
            if days.is_empty() {
                return Err(bad("Mindestens einen Wochentag wählen.", "Choose at least one day of the week."));
            }
            // Montag zuerst, Sonntag zuletzt (lesbarer)
            let mut ordered: Vec<&str> = days.iter().copied().filter(|d| *d != "SUN").collect();
            if days.contains(&"SUN") {
                ordered.push("SUN");
            }
            format!("{minute} {hour} * * {}", ordered.join(","))
        }
        Pattern::MonthlyDay(d) => {
            if !(1..=31).contains(&d) {
                return Err(bad("Der Tag muss zwischen 1 und 31 liegen.", "The day must be between 1 and 31."));
            }
            format!("{minute} {hour} {d} * *")
        }
        Pattern::MonthlyNth(n, wd) => {
            if !(1..=5).contains(&n) || wd > 6 {
                return Err(bad("Ungültiger Wochentag im Monat.", "Invalid weekday of the month."));
            }
            format!("{minute} {hour} * * {}#{n}", DAY_NAMES[wd as usize])
        }
        Pattern::Yearly { month, day } => {
            let max = match month {
                2 => 29,
                4 | 6 | 9 | 11 => 30,
                1..=12 => 31,
                _ => return Err(bad("Ungültiger Monat.", "Invalid month.")),
            };
            if !(1..=max).contains(&day) {
                return Err(bad("Diesen Tag gibt es in dem Monat nicht.", "That day does not exist in the month."));
            }
            format!("{minute} {hour} {day} {month} *")
        }
        Pattern::EveryMinutes(x) => {
            if !MINUTE_STEPS.contains(&x) {
                return Err(bad("Nur Schritte, die 60 teilen (1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30).", "Only steps that divide 60 (1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30)."));
            }
            if x == 1 { "* * * * *".into() } else { format!("*/{x} * * * *") }
        }
        Pattern::EveryHours(x) => {
            if !HOUR_STEPS.contains(&x) {
                return Err(bad("Nur Schritte, die 24 teilen (1, 2, 3, 4, 6, 8, 12).", "Only steps that divide 24 (1, 2, 3, 4, 6, 8, 12)."));
            }
            if x == 1 { format!("{minute} * * * *") } else { format!("{minute} */{x} * * *") }
        }
    })
}

fn single(v: &[u32]) -> Option<u32> {
    (v.len() == 1).then(|| v[0])
}

/// Schritt einer gleichmäßigen Folge ab 0 über den ganzen Bereich `0..n` (z. B. 0,15,30,45 → 15).
fn even_step(v: &[u32], n: u32) -> Option<u32> {
    let st = if v.len() == 1 { n } else { v.get(1)? - v[0] };
    (v.first() == Some(&0) && st > 0 && n % st == 0 && v.len() as u32 == n / st && v.iter().enumerate().all(|(i, x)| *x == i as u32 * st)).then_some(st)
}

/// Muster und Uhrzeit (Stunde, Minute) eines Ausdrucks, wenn der Baukasten genau ihn erzeugt hätte (gleiche Bedeutung).
pub fn recognise(c: &Cron) -> Option<(Pattern, u32, u32)> {
    let (mins, hours) = (c.minute_values(), c.hour_values());
    let all_days = c.every_dom() && c.every_dow() && c.every_month();
    let day_level = if let (Some(m), Some(h)) = (single(&mins), single(&hours)) {
        let dows = c.dow_values();
        let nth = c.nth_values();
        let p = if all_days {
            Some(Pattern::Daily)
        } else if c.every_dom() && c.every_month() && nth.is_empty() && !c.dow_unrestricted() {
            let bits = dows.iter().fold(0u8, |b, d| b | (1 << d));
            Some(if bits == 0b0011_1110 { Pattern::Weekdays } else { Pattern::Weekly(bits) })
        } else if c.every_dom() && c.every_month() && dows.is_empty() && nth.len() == 1 {
            Some(Pattern::MonthlyNth(nth[0].1, nth[0].0))
        } else if c.every_dow() && !c.dom_unrestricted() {
            match (single(&c.dom_values()), c.every_month(), single(&c.month_values())) {
                (Some(d), true, _) => Some(Pattern::MonthlyDay(d)),
                (Some(d), false, Some(mo)) => Some(Pattern::Yearly { month: mo, day: d }),
                _ => None,
            }
        } else {
            None
        };
        p.map(|p| (p, h, m))
    } else {
        None
    };
    let found = day_level
        .or_else(|| (all_days && hours.len() == 24).then(|| even_step(&mins, 60)).flatten().filter(|s| MINUTE_STEPS.contains(s)).map(|s| (Pattern::EveryMinutes(s), 0, 0)))
        .or_else(|| {
            let m = single(&mins).filter(|_| all_days)?;
            even_step(&hours, 24).filter(|s| HOUR_STEPS.contains(s)).map(|s| (Pattern::EveryHours(s), 0, m))
        });
    // Sicherheitsnetz: das Muster muss genau dieselben Zeiten ergeben
    found.filter(|(p, h, m)| to_cron(*p, *h, *m).ok().and_then(|e| Cron::parse(&e).ok()).is_some_and(|b| same_meaning(&b, c)))
}

fn same_meaning(a: &Cron, b: &Cron) -> bool {
    a.minute_values() == b.minute_values()
        && a.hour_values() == b.hour_values()
        && a.dom_values() == b.dom_values()
        && a.month_values() == b.month_values()
        && a.dow_values() == b.dow_values()
        && a.nth_values() == b.nth_values()
        && a.or_case() == b.or_case()
        && a.dom_unrestricted() == b.dom_unrestricted()
        && a.dow_unrestricted() == b.dow_unrestricted()
}

/// Hinweis zu einem Muster, das nicht in jedem Zeitraum vorkommt (Tag 29–31, 29. Februar, fünfter Wochentag).
pub fn hint(p: Pattern, lang: Lang) -> Option<&'static str> {
    match p {
        Pattern::MonthlyDay(d) if d >= 29 => Some(lang.t("Monate ohne diesen Tag werden übersprungen.", "Months without this day are skipped.")),
        Pattern::Yearly { month: 2, day: 29 } => Some(lang.t("Nur in Schaltjahren.", "Only in leap years.")),
        Pattern::MonthlyNth(5, _) => Some(lang.t("Nur in Monaten mit fünf solchen Wochentagen.", "Only in months with five such weekdays.")),
        _ => None,
    }
}

/// Name eines Wochentags (0 = Sonntag).
pub fn weekday_name(wd: u8, lang: Lang) -> &'static str {
    const DE: [&str; 7] = ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"];
    const EN: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let i = (wd % 7) as usize;
    match lang {
        Lang::De => DE[i],
        Lang::En => EN[i],
    }
}

/// Kurzname eines Wochentags (0 = Sonntag).
pub fn weekday_short(wd: u8, lang: Lang) -> &'static str {
    const DE: [&str; 7] = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];
    const EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let i = (wd % 7) as usize;
    match lang {
        Lang::De => DE[i],
        Lang::En => EN[i],
    }
}

/// Name eines Monats (1..=12).
pub fn month_name(m: u32, lang: Lang) -> &'static str {
    const DE: [&str; 12] = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"];
    const EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let i = (m.clamp(1, 12) - 1) as usize;
    match lang {
        Lang::De => DE[i],
        Lang::En => EN[i],
    }
}

fn ordinal(n: u8, lang: Lang) -> &'static str {
    match (lang, n) {
        (Lang::De, 1) => "ersten",
        (Lang::De, 2) => "zweiten",
        (Lang::De, 3) => "dritten",
        (Lang::De, 4) => "vierten",
        (Lang::De, _) => "fünften",
        (Lang::En, 1) => "first",
        (Lang::En, 2) => "second",
        (Lang::En, 3) => "third",
        (Lang::En, 4) => "fourth",
        (Lang::En, _) => "fifth",
    }
}

/// Lesbare Beschreibung eines Ausdrucks (Baukasten-Muster in Worten, sonst der Ausdruck selbst).
pub fn describe(c: &Cron, lang: Lang) -> String {
    let Some((p, h, m)) = recognise(c) else {
        return format!("{} {}", lang.t("Cron:", "Cron:"), c.text);
    };
    let at = format!("{h:02}:{m:02}");
    match (p, lang) {
        (Pattern::Daily, Lang::De) => format!("Täglich um {at}"),
        (Pattern::Daily, Lang::En) => format!("Daily at {at}"),
        (Pattern::Weekdays, Lang::De) => format!("Werktags (Mo–Fr) um {at}"),
        (Pattern::Weekdays, Lang::En) => format!("Weekdays (Mon–Fri) at {at}"),
        (Pattern::Weekly(bits), _) => {
            let days: Vec<&str> = [1u8, 2, 3, 4, 5, 6, 0].iter().filter(|d| bits & (1 << *d) != 0).map(|d| weekday_short(*d, lang)).collect();
            match lang {
                Lang::De => format!("Wöchentlich am {} um {at}", days.join(", ")),
                Lang::En => format!("Weekly on {} at {at}", days.join(", ")),
            }
        }
        (Pattern::MonthlyDay(d), Lang::De) => format!("Monatlich am {d}. um {at}"),
        (Pattern::MonthlyDay(d), Lang::En) => format!("Monthly on day {d} at {at}"),
        (Pattern::MonthlyNth(n, wd), Lang::De) => format!("Monatlich am {} {} um {at}", ordinal(n, lang), weekday_name(wd, lang)),
        (Pattern::MonthlyNth(n, wd), Lang::En) => format!("Monthly on the {} {} at {at}", ordinal(n, lang), weekday_name(wd, lang)),
        (Pattern::Yearly { month, day }, Lang::De) => format!("Jährlich am {day}. {} um {at}", month_name(month, lang)),
        (Pattern::Yearly { month, day }, Lang::En) => format!("Yearly on {} {day} at {at}", month_name(month, lang)),
        (Pattern::EveryMinutes(1), Lang::De) => "Jede Minute".into(),
        (Pattern::EveryMinutes(1), Lang::En) => "Every minute".into(),
        (Pattern::EveryMinutes(x), Lang::De) => format!("Alle {x} Minuten"),
        (Pattern::EveryMinutes(x), Lang::En) => format!("Every {x} minutes"),
        (Pattern::EveryHours(1), Lang::De) => format!("Stündlich zur Minute {m}"),
        (Pattern::EveryHours(1), Lang::En) => format!("Hourly at minute {m}"),
        (Pattern::EveryHours(x), Lang::De) => format!("Alle {x} Stunden zur Minute {m}"),
        (Pattern::EveryHours(x), Lang::En) => format!("Every {x} hours at minute {m}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(p: Pattern, h: u32, m: u32) -> String {
        let e = to_cron(p, h, m).unwrap();
        let c = Cron::parse(&e).unwrap();
        assert!(!c.or_case(), "{e}: der Baukasten erzeugt nie den ODER-Fall");
        let back = recognise(&c).unwrap_or_else(|| panic!("{e} nicht wiedererkannt"));
        let expect = match p {
            Pattern::EveryMinutes(_) => (p, 0, 0),
            Pattern::EveryHours(_) => (p, 0, m),
            Pattern::Weekly(0b0011_1110) => (Pattern::Weekdays, h, m),
            _ => (p, h, m),
        };
        assert_eq!(back, expect, "{e}");
        e
    }

    #[test]
    fn builder_produces_the_expected_expressions_and_recognises_them() {
        assert_eq!(rt(Pattern::Daily, 9, 0), "0 9 * * *");
        assert_eq!(rt(Pattern::Weekdays, 7, 30), "30 7 * * MON-FRI");
        assert_eq!(rt(Pattern::Weekly(0b0100_0001 | 0b10), 18, 5), "5 18 * * MON,SAT,SUN");
        assert_eq!(rt(Pattern::Weekly(0b0011_1110), 8, 0), "0 8 * * MON,TUE,WED,THU,FRI");
        assert_eq!(rt(Pattern::MonthlyDay(15), 12, 0), "0 12 15 * *");
        assert_eq!(rt(Pattern::MonthlyNth(1, 1), 9, 0), "0 9 * * MON#1");
        assert_eq!(rt(Pattern::MonthlyNth(5, 5), 9, 0), "0 9 * * FRI#5");
        assert_eq!(rt(Pattern::Yearly { month: 2, day: 29 }, 0, 0), "0 0 29 2 *");
        assert_eq!(rt(Pattern::EveryMinutes(15), 0, 0), "*/15 * * * *");
        assert_eq!(rt(Pattern::EveryMinutes(1), 0, 0), "* * * * *");
        assert_eq!(rt(Pattern::EveryHours(2), 0, 30), "30 */2 * * *");
        assert_eq!(rt(Pattern::EveryHours(1), 0, 10), "10 * * * *");
    }

    #[test]
    fn builder_refuses_ambiguous_or_invalid_input() {
        assert!(to_cron(Pattern::EveryMinutes(7), 0, 0).is_err(), "7 teilt 60 nicht – ungleiche Abstände");
        assert!(to_cron(Pattern::EveryHours(5), 0, 0).is_err());
        assert!(to_cron(Pattern::Weekly(0), 9, 0).is_err());
        assert!(to_cron(Pattern::MonthlyDay(32), 9, 0).is_err());
        assert!(to_cron(Pattern::MonthlyNth(6, 1), 9, 0).is_err());
        assert!(to_cron(Pattern::Yearly { month: 4, day: 31 }, 9, 0).is_err());
        assert!(to_cron(Pattern::Daily, 24, 0).is_err());
    }

    #[test]
    fn foreign_expressions_are_not_forced_into_the_builder() {
        for e in ["0 0 13 * FRI", "*/7 * * * *", "0 9-17 * * *", "0 9 1,15 * *", "0 9 * 1-6 *", "*/5 9 * * *", "0 9 * * MON#1,FRI#3"] {
            assert!(recognise(&Cron::parse(e).unwrap()).is_none(), "{e}");
        }
        // Kurzformen sind gewöhnliche Muster
        assert_eq!(recognise(&Cron::parse("@daily").unwrap()), Some((Pattern::Daily, 0, 0)));
        assert_eq!(recognise(&Cron::parse("@weekly").unwrap()), Some((Pattern::Weekly(1), 0, 0)));
        assert_eq!(recognise(&Cron::parse("@yearly").unwrap()), Some((Pattern::Yearly { month: 1, day: 1 }, 0, 0)));
        assert_eq!(recognise(&Cron::parse("0 9 * * 1-5").unwrap()), Some((Pattern::Weekdays, 9, 0)));
    }

    #[test]
    fn hints_and_descriptions_in_both_languages() {
        assert!(hint(Pattern::MonthlyDay(31), Lang::En).is_some() && hint(Pattern::MonthlyDay(28), Lang::En).is_none());
        assert!(hint(Pattern::Yearly { month: 2, day: 29 }, Lang::De).is_some());
        assert!(hint(Pattern::MonthlyNth(5, 1), Lang::De).is_some());
        let c = Cron::parse("0 9 * * MON#1").unwrap();
        assert_eq!(describe(&c, Lang::En), "Monthly on the first Monday at 09:00");
        assert_eq!(describe(&c, Lang::De), "Monatlich am ersten Montag um 09:00");
        assert_eq!(describe(&Cron::parse("*/15 * * * *").unwrap(), Lang::En), "Every 15 minutes");
        assert_eq!(describe(&Cron::parse("0 9 * * 1,3").unwrap(), Lang::De), "Wöchentlich am Mo, Mi um 09:00");
        assert_eq!(describe(&Cron::parse("0 0 13 * FRI").unwrap(), Lang::En), "Cron: 0 0 13 * FRI");
    }
}

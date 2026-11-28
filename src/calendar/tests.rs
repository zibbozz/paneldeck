//! Tests des Kalenders über die Module hinweg (#164): Vorkommen im Zeitraum (Anzahl, Ende, Ausnahmen, ganztägig,
//! Sommerzeit), Quellen aus der Planung, iCal (Rundreise, RRULE ↔ Cron, Bericht, Grenzen), Erinnerungen, MCP-Werkzeuge,
//! Migration alter Datenbanken, Sicherung und Leistung mit 1000 Terminen. Feste Uhr und feste Zeitzone.

use super::cron::Cron;
use super::ics;
use super::remind::{due_reminders, poll};
use super::sources::{self, Kind};
use super::store::{self, EventInput, Options, Sources};
use super::zone::{secs, RuleZone, Zone};
use super::{parse_when, Event, Recurrence};
use crate::state::Lang;
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{json, Value};

fn dt(s: &str) -> NaiveDateTime {
    parse_when(s).unwrap().0
}
fn d(s: &str) -> NaiveDate {
    super::parse_date(s).unwrap()
}

fn ev(start: &str, minutes: i64, cron: Option<&str>) -> Event {
    Event {
        id: 1,
        uid: "u".into(),
        title: "T".into(),
        note: String::new(),
        color: String::new(),
        all_day: false,
        start: dt(start),
        minutes,
        recur: cron.map(|c| Recurrence { cron: c.into(), ..Default::default() }),
        reminder: None,
        project_id: None,
        origin: "local".into(),
        created_at: "2026-11-01T00:00:00".into(),
        updated_at: "2026-11-01T00:00:00".into(),
    }
}

fn fmt(v: &[NaiveDateTime]) -> Vec<String> {
    v.iter().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).collect()
}

// ───────────────────────── Vorkommen

#[test]
fn occurrences_only_in_the_range_with_count_until_and_exceptions() {
    let z = RuleZone::berlin();
    let mut e = ev("2026-11-02T09:00", 30, Some("0 9 * * MON-FRI"));
    let occ = e.occurrences(dt("2026-11-09"), dt("2026-11-14"), &z);
    assert_eq!(occ.len(), 5, "Mo–Fr einer Woche");
    // Anzahl zählt ab dem Beginn der Serie (auch Ausnahmen zählen mit, wie bei iCal)
    let r = e.recur.as_mut().unwrap();
    r.count = Some(7);
    r.except = vec![d("2026-11-03")];
    let all = e.occurrences(dt("2026-11-01"), dt("2026-12-01"), &z);
    assert_eq!(fmt(&all), ["2026-11-02 09:00", "2026-11-04 09:00", "2026-11-05 09:00", "2026-11-06 09:00", "2026-11-09 09:00", "2026-11-10 09:00"]);
    assert_eq!(e.occurrences(dt("2026-11-10"), dt("2026-12-01"), &z).len(), 1, "auch mit Anzahl nur im Zeitraum");
    let r = e.recur.as_mut().unwrap();
    r.count = None;
    r.until = Some(d("2026-11-04"));
    assert_eq!(e.occurrences(dt("2026-11-01"), dt("2026-12-01"), &z).len(), 2, "bis einschließlich 4.11., ohne die Ausnahme am 3.");
    // vor dem Beginn nichts
    assert!(e.occurrences(dt("2026-10-01"), dt("2026-11-01"), &z).is_empty());
}

#[test]
fn long_and_point_events_overlap_correctly() {
    let z = RuleZone::utc();
    let e = ev("2026-11-10T22:00", 240, None);
    assert_eq!(e.occurrences(dt("2026-11-11"), dt("2026-11-12"), &z).len(), 1, "reicht über Mitternacht");
    assert!(e.occurrences(dt("2026-11-11T02:00"), dt("2026-11-12"), &z).is_empty(), "endet genau um 02:00");
    let p = ev("2026-11-11T00:00", 0, None);
    assert_eq!(p.occurrences(dt("2026-11-11"), dt("2026-11-12"), &z).len(), 1, "Zeitpunkt am Anfang des Zeitraums");
    // Serie, deren Vorkommen vor dem Zeitraum beginnt und hineinreicht
    let s = ev("2026-11-01T23:00", 120, Some("0 23 * * *"));
    assert_eq!(fmt(&s.occurrences(dt("2026-11-05"), dt("2026-11-06"), &z)), ["2026-11-04 23:00", "2026-11-05 23:00"]);
}

/// Kurzer Takt mit langer Dauer (z. B. importiert: `RRULE:FREQ=MINUTELY`, `DURATION:P366D`, Beginn vor einem Jahr):
/// Die Suche rechnet nicht jedes hineinreichende Vorkommen seit einem Jahr durch, liefert höchstens die Höchstzahl vor
/// dem Zeitraum (die spätesten) plus die Höchstzahl darin und ist schnell. Lange Vorkommen mit langem Takt (wöchentlich
/// drei Tage) bleiben vollständig.
#[test]
fn short_step_with_long_duration_is_capped_and_fast() {
    let z = RuleZone::berlin();
    let e = ev("2025-11-27T00:00", super::MAX_MINUTES, Some("* * * * *"));
    let (from, to) = (dt("2026-11-01"), dt("2026-12-01"));
    let t0 = std::time::Instant::now();
    let occ = e.occurrences(from, to, &z);
    let took = t0.elapsed();
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    let before = occ.iter().filter(|s| **s < from).count();
    assert_eq!(before, super::MAX_OCC_PER_RANGE, "die spätesten hineinreichenden");
    assert_eq!(occ.len(), 2 * super::MAX_OCC_PER_RANGE);
    assert_eq!(fmt(&occ[before - 1..before + 1]), ["2026-10-31 23:59", "2026-11-01 00:00"], "lückenlos bis in den Zeitraum");
    assert!(occ.windows(2).all(|w| w[0] < w[1]));
    // Ausgelassene Tage kosten keine Minute je Schritt (fast alle Tage vor dem Zeitraum ausgelassen)
    let mut x = e.clone();
    x.recur.as_mut().unwrap().except = (0..300).map(|i| super::add_days(d("2026-01-01"), i)).collect();
    let t0 = std::time::Instant::now();
    let occ = x.occurrences(from, to, &z);
    assert!(t0.elapsed() < std::time::Duration::from_secs(2));
    assert!(occ.len() <= 2 * super::MAX_OCC_PER_RANGE && occ.iter().all(|s| x.recur.as_ref().unwrap().except.binary_search(&s.date()).is_err()));
    // Mit Anzahl (vom Beginn an gezählt): ebenso begrenzt
    let mut c = e.clone();
    c.recur.as_mut().unwrap().count = Some(super::MAX_COUNT);
    let occ = c.occurrences(dt("2025-12-10"), dt("2025-12-11"), &z);
    assert!(occ.len() <= 2 * super::MAX_OCC_PER_RANGE, "{}", occ.len());
    // Wöchentlich drei Tage: das Vorkommen von vor zwei Tagen reicht hinein und bleibt
    let w = ev("2026-11-02T09:00", 3 * 1440, Some("0 9 * * MON"));
    assert_eq!(fmt(&w.occurrences(dt("2026-11-11"), dt("2026-11-12"), &z)), ["2026-11-09 09:00"]);
    assert_eq!(w.occurrences(dt("2026-11-01"), dt("2026-12-01"), &z).len(), 5);
}

/// Ausnahmen gelten ganzen Tagen: Termine mit mehreren Vorkommen am Tag erkennt die Oberfläche („Diesen Tag
/// überspringen“).
#[test]
fn several_occurrences_per_day_are_recognised() {
    assert!(ev("2026-11-02T09:00", 30, Some("0 * * * *")).several_per_day(), "stündlich");
    assert!(ev("2026-11-02T09:00", 30, Some("0,30 9 * * *")).several_per_day());
    assert!(ev("2026-11-02T09:00", 30, Some("0 9,17 * * MON-FRI")).several_per_day());
    assert!(!ev("2026-11-02T09:00", 30, Some("0 9 * * MON-FRI")).several_per_day());
    assert!(!ev("2026-11-02T09:00", 30, None).several_per_day());
    assert!(!ev("2026-11-02T09:00", 30, Some("kaputt")).several_per_day());
    let mut a = ev("2026-11-01", 1440, Some("0 0 * * *"));
    a.all_day = true;
    assert!(!a.several_per_day());
}

#[test]
fn all_day_series_and_multi_day() {
    let z = RuleZone::berlin();
    let mut e = ev("2026-11-01", 1440, Some("0 0 * * SAT,SUN"));
    e.all_day = true;
    let occ = e.occurrences(dt("2026-11-01"), dt("2026-12-01"), &z);
    assert_eq!(occ.len(), 9, "November 2026: 4 Samstage + 5 Sonntage");
    let mut b = ev("2028-02-29", 1440, Some("0 0 29 2 *"));
    b.all_day = true;
    assert_eq!(fmt(&b.next_occurrences(dt("2028-03-01"), 2, &z)), ["2032-02-29 00:00", "2036-02-29 00:00"], "29. Februar nur in Schaltjahren");
    let mut trip = ev("2026-11-27", 3 * 1440, None);
    trip.all_day = true;
    assert_eq!(trip.occurrences(dt("2026-11-29"), dt("2026-11-30"), &z).len(), 1);
    assert!(trip.occurrences(dt("2026-11-30"), dt("2026-12-01"), &z).is_empty());
}

#[test]
fn series_across_dst_in_local_time() {
    let z = RuleZone::berlin();
    let e = ev("2026-03-27T02:30", 30, Some("30 2 * * *"));
    let occ = e.occurrences(dt("2026-03-27"), dt("2026-03-31"), &z);
    assert_eq!(fmt(&occ), ["2026-03-27 02:30", "2026-03-28 02:30", "2026-03-29 03:00", "2026-03-30 02:30"], "Lücke → 03:00");
    let e = ev("2026-10-24T02:30", 30, Some("30 2 * * *"));
    assert_eq!(e.occurrences(dt("2026-10-25"), dt("2026-10-26"), &z).len(), 1, "doppelte Stunde einmal");
    // die Uhrzeit bleibt lokal gleich: 09:00 vor und nach der Umstellung, verschiedene UTC-Zeitpunkte
    let e = ev("2026-10-23T09:00", 30, Some("0 9 * * *"));
    let o = e.occurrences(dt("2026-10-24"), dt("2026-10-27"), &z);
    assert_eq!(z.instant(o[2]) - z.instant(o[0]), 2 * 86400 + 3600, "25-Stunden-Tag dazwischen");
}

// ───────────────────────── Quellen

#[test]
fn sources_follow_the_options_globally_and_per_project() {
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::releases::{create_version, NewRelease};
    use crate::planning::tickets::fixture::Fx;
    let f = Fx::new();
    update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
    let t = f.mk(f.task, "Fix login");
    let due = f.field(f.task, "due").id;
    f.c.execute("INSERT INTO ticket_values (ticket_id, field_id, pos, text) VALUES (?1, ?2, 0, '2026-11-20')", rusqlite::params![t.id, due]).unwrap();
    let t2 = f.mk(f.task, "Release prep");
    f.c.execute("INSERT INTO ticket_values (ticket_id, field_id, pos, text) VALUES (?1, ?2, 0, '2026-11-21T14:30')", rusqlite::params![t2.id, due]).unwrap();
    f.c.execute("INSERT INTO todo_items (title, body, done, position, created_at, updated_at, due) VALUES ('Buy milk', '', 0, 1, 'x', 'x', '2026-11-22')", []).unwrap();
    f.c.execute("INSERT INTO sprints (project_id, name, start_date, end_date, state, created_at) VALUES (?1, 'Sprint 7', '2026-11-16', '2026-11-27', 'planned', 'x')", [f.project]).unwrap();
    let mut nv = NewRelease::new(f.project, "1.2");
    nv.due_date = Some(d("2026-11-25"));
    create_version(&f.c, &nv).unwrap();
    f.c.execute("INSERT INTO time_entries (ticket_id, start, minutes, comment, created_at) VALUES (?1, '2026-11-18T10:00:00', 90, '', 'x')", [t.id]).unwrap();
    let kinds = |o: &Options| {
        let mut k: Vec<Kind> = sources::source_items(&f.c, o, d("2026-11-01"), d("2026-12-01")).iter().map(|i| i.kind).collect();
        k.sort();
        k
    };
    // erstes Öffnen: nur Fälligkeiten
    assert_eq!(kinds(&Options::default()), [Kind::Due, Kind::Due, Kind::Todo]);
    let all = Options { sources: Sources { due: true, sprints: true, versions: true, time: true }, ..Default::default() };
    assert_eq!(kinds(&all), [Kind::Due, Kind::Due, Kind::Todo, Kind::Sprint, Kind::Version, Kind::Time]);
    // je Projekt abschalten; Todo-Liste (0) eigens
    let mut o = all.clone();
    o.per_project.insert(f.project, Sources { due: false, sprints: true, versions: false, time: false });
    o.per_project.insert(0, Sources { due: false, ..Default::default() });
    assert_eq!(kinds(&o), [Kind::Sprint]);
    let items = sources::source_items(&f.c, &all, d("2026-11-01"), d("2026-12-01"));
    let sprint = items.iter().find(|i| i.kind == Kind::Sprint).unwrap();
    assert!(sprint.multi_day() && sprint.day_span() == (d("2026-11-16"), d("2026-11-27")));
    let d1 = items.iter().find(|i| i.kind == Kind::Due && i.all_day).unwrap();
    assert_eq!((d1.key.as_str(), d1.title.as_str()), ("PAN-1", "Fix login"));
    let d2 = items.iter().find(|i| i.kind == Kind::Due && !i.all_day).unwrap();
    assert_eq!(d2.start, dt("2026-11-21T14:30"));
    let time = items.iter().find(|i| i.kind == Kind::Time).unwrap();
    assert_eq!((time.start, time.end), (dt("2026-11-18T10:00"), dt("2026-11-18T11:30")));
    // archivierte Tickets fehlen; ohne Sprints im Projekt keine Sprints
    f.c.execute("UPDATE tickets SET archived = 1 WHERE id = ?1", [t.id]).unwrap();
    update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(false), ..Default::default() }).unwrap();
    assert_eq!(kinds(&all), [Kind::Due, Kind::Todo, Kind::Version], "auch die Zeiteinträge des archivierten Tickets fehlen");
    // außerhalb des Zeitraums nichts
    assert!(sources::source_items(&f.c, &all, d("2027-01-01"), d("2027-02-01")).is_empty());
}

#[test]
fn collect_merges_events_and_sources_sorted() {
    let c = crate::db::open_memory();
    store::create_event(&c, EventInput::new("Late", dt("2026-11-10T15:00"), 60), None, "local").unwrap();
    let mut i = EventInput::new("Early", dt("2026-11-10T08:00"), 60);
    i.recur = Some(Recurrence { cron: "0 8 * * *".into(), ..Default::default() });
    store::create_event(&c, i, None, "local").unwrap();
    c.execute("INSERT INTO todo_items (title, body, done, position, created_at, updated_at, due) VALUES ('Card', '', 0, 1, 'x', 'x', '2026-11-10')", []).unwrap();
    let items = sources::collect(&c, &Options::default(), d("2026-11-10"), d("2026-11-11"), &RuleZone::utc());
    let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Card", "Early", "Late"], "ganztägig zuerst, dann nach Uhrzeit");
    assert!(items[1].recurring && !items[2].recurring);
}

// ───────────────────────── iCal

#[test]
fn rrule_from_cron_for_every_feature() {
    let r = |c: &str| ics::cron_to_rrule(&Cron::parse(c).unwrap(), false);
    assert_eq!(r("0 9 * * *").as_deref(), Some("FREQ=DAILY"));
    assert_eq!(r("0 9 * * MON-FRI").as_deref(), Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"));
    assert_eq!(r("0 9 15 * *").as_deref(), Some("FREQ=MONTHLY;BYMONTHDAY=15"));
    assert_eq!(r("0 9 29 2 *").as_deref(), Some("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29"));
    assert_eq!(r("0 9 * * MON#1").as_deref(), Some("FREQ=MONTHLY;BYDAY=1MO"));
    assert_eq!(r("0 9 * * MON#1,FRI").as_deref(), Some("FREQ=MONTHLY;BYDAY=FR,1MO"));
    assert_eq!(r("*/15 * * * *").as_deref(), Some("FREQ=MINUTELY;INTERVAL=15"));
    assert_eq!(r("* * * * *").as_deref(), Some("FREQ=MINUTELY"));
    assert_eq!(r("30 */2 * * *").as_deref(), Some("FREQ=HOURLY;INTERVAL=2"));
    assert_eq!(r("0,30 9-11 * * MON").as_deref(), Some("FREQ=DAILY;BYDAY=MO;BYHOUR=9,10,11;BYMINUTE=0,30"));
    assert_eq!(r("0 9 * 1-3 *").as_deref(), Some("FREQ=DAILY;BYMONTH=1,2,3"));
    assert_eq!(r("*/7 * * * *").as_deref(), Some("FREQ=DAILY;BYHOUR=0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23;BYMINUTE=0,7,14,21,28,35,42,49,56"));
    assert_eq!(r("0 0 13 * FRI"), None, "ODER-Fall hat keine RRULE");
    assert_eq!(ics::cron_to_rrule(&Cron::parse("0 0 * * SAT,SUN").unwrap(), true).as_deref(), Some("FREQ=WEEKLY;BYDAY=SU,SA"));
}

#[test]
fn cron_from_rrule_and_its_limits() {
    let start = dt("2026-11-02T09:30");
    let c = |r: &str| ics::rrule_to_cron(r, start, false).map(|x| x.cron);
    assert_eq!(c("FREQ=DAILY").unwrap(), "30 9 * * *");
    assert_eq!(c("FREQ=WEEKLY").unwrap(), "30 9 * * MON", "Wochentag aus DTSTART");
    assert_eq!(c("FREQ=WEEKLY;BYDAY=MO,WE,FR").unwrap(), "30 9 * * MON,WED,FRI");
    assert_eq!(c("FREQ=MONTHLY").unwrap(), "30 9 2 * *");
    assert_eq!(c("FREQ=MONTHLY;BYDAY=1MO").unwrap(), "30 9 * * MON#1");
    assert_eq!(c("FREQ=MONTHLY;INTERVAL=3").unwrap(), "30 9 2 2/3 *", "alle 3 Monate ab November: 2, 5, 8, 11");
    assert_eq!(c("FREQ=YEARLY").unwrap(), "30 9 2 11 *");
    assert_eq!(c("FREQ=YEARLY;BYMONTH=3;BYDAY=2SU").unwrap(), "30 9 * 3 SUN#2");
    assert_eq!(c("FREQ=MINUTELY;INTERVAL=15").unwrap(), "0/15 * * * *");
    assert_eq!(c("FREQ=HOURLY;INTERVAL=6").unwrap(), "30 3/6 * * *");
    assert_eq!(c("FREQ=DAILY;BYHOUR=9,17;BYMINUTE=0").unwrap(), "0 9,17 * * *");
    let x = ics::rrule_to_cron("FREQ=DAILY;COUNT=5;UNTIL=20261231T225959Z", start, false).unwrap();
    assert_eq!((x.count, x.until.as_deref()), (Some(5), Some("20261231T225959Z")));
    assert_eq!(ics::rrule_to_cron("FREQ=YEARLY", dt("2026-11-02"), true).unwrap().cron, "0 0 2 11 *", "ganztägig: 00:00");
    for bad in ["FREQ=DAILY;INTERVAL=2", "FREQ=WEEKLY;INTERVAL=2", "FREQ=MONTHLY;BYDAY=-1FR", "FREQ=MONTHLY;BYMONTHDAY=-1", "FREQ=MONTHLY;BYSETPOS=1;BYDAY=MO", "FREQ=SECONDLY", "FREQ=MINUTELY;INTERVAL=7", "FREQ=WEEKLY;BYDAY=1MO", "FREQ=MONTHLY;BYMONTHDAY=13;BYDAY=FR", "INTERVAL=2"] {
        let e = ics::rrule_to_cron(bad, start, false).unwrap_err();
        assert!(!e.de.is_empty() && !e.en.is_empty(), "{bad}");
    }
}

#[test]
fn rrule_round_trip_gives_the_same_occurrences() {
    let z = RuleZone::utc();
    for c in ["0 9 * * *", "0 9 * * MON-FRI", "0 9 15 * *", "0 9 29 2 *", "0 9 * * MON#1", "*/15 * * * *", "30 */2 * * *", "0,30 9-11 * * MON", "0 9 * 1-3 *", "*/7 * * * *", "0 9 * * MON#1,FRI"] {
        let cron = Cron::parse(c).unwrap();
        let start = cron.preview(dt("2026-01-01"), 1, &z)[0];
        let rule = ics::cron_to_rrule(&cron, false).unwrap();
        let back = ics::rrule_to_cron(&rule, start, false).unwrap_or_else(|e| panic!("{c} → {rule}: {}", e.en)).cron;
        let a = cron.preview(start, 40, &z);
        let b = Cron::parse(&back).unwrap().preview(start, 40, &z);
        assert_eq!(a, b, "{c} → {rule} → {back}");
    }
}

fn store_with_events() -> rusqlite::Connection {
    let c = crate::db::open_memory();
    let mut i = EventInput::new("Standup; daily, short", dt("2026-11-02T09:00"), 15);
    i.recur = Some(Recurrence { cron: "0 9 * * MON-FRI".into(), until: Some(d("2026-12-18")), count: None, except: vec![d("2026-11-11")] });
    i.reminder = Some(10);
    i.color = "#E06C75".into();
    i.note = "Line 1\nLine 2 with a very long text that will certainly need folding because it is longer than seventy-five bytes".into();
    store::create_event(&c, i, Some("standup@test"), "local").unwrap();
    let mut i = EventInput::new("Vacation", dt("2026-12-21"), 5 * 1440);
    i.all_day = true;
    store::create_event(&c, i, Some("vacation@test"), "local").unwrap();
    let mut i = EventInput::new("Odd one", dt("2026-11-13T00:00"), 60);
    i.recur = Some(Recurrence { cron: "0 0 13 * FRI".into(), count: Some(4), ..Default::default() });
    store::create_event(&c, i, Some("odd@test"), "local").unwrap();
    let mut i = EventInput::new("Board", dt("2026-11-02T18:00"), 60);
    i.recur = Some(Recurrence { cron: "0 18 * * MON#1".into(), ..Default::default() });
    store::create_event(&c, i, Some("board@test"), "local").unwrap();
    c
}

#[test]
fn export_format_is_stable_public_api() {
    let c = store_with_events();
    let events = store::all_events(&c).unwrap();
    let z = RuleZone::berlin();
    let (text, rep) = ics::export(&events, &z, secs(dt("2026-11-22T09:00")));
    assert_eq!(rep.events, 4);
    assert_eq!(rep.without_rrule, ["Odd one"], "ODER-Fall ohne RRULE");
    assert!(text.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Paneldeck//Calendar "));
    assert!(text.ends_with("END:VCALENDAR\r\n"));
    assert!(text.split("\r\n").all(|l| l.len() <= 75), "gefaltet");
    for must in [
        "BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\n",
        "BEGIN:DAYLIGHT\r\nDTSTART:19700329T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT",
        "BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD",
        "UID:standup@test\r\n",
        "DTSTAMP:20261122T090000Z\r\n",
        "DTSTART;TZID=Europe/Berlin:20261102T090000\r\n",
        "DTEND;TZID=Europe/Berlin:20261102T091500\r\n",
        "SUMMARY:Standup\\; daily\\, short\r\n",
        "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;UNTIL=20261218T225959Z\r\n",
        "X-PANELDECK-CRON:0 9 * * MON-FRI\r\n",
        "EXDATE;TZID=Europe/Berlin:20261111T090000\r\n",
        "X-PANELDECK-COLOR:#E06C75\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\n",
        "TRIGGER:-PT10M\r\n",
        "DTSTART;VALUE=DATE:20261221\r\nDTEND;VALUE=DATE:20261226\r\n",
        "RRULE:FREQ=MONTHLY;BYDAY=1MO\r\n",
        "X-PANELDECK-CRON:0 0 13 * FRI\r\n",
    ] {
        assert!(text.contains(must), "fehlt: {must:?}\n{text}");
    }
    assert!(text.contains("DESCRIPTION:Line 1\\nLine 2"));
}

#[test]
fn ics_round_trip_keeps_everything() {
    let c = store_with_events();
    let events = store::all_events(&c).unwrap();
    let z = RuleZone::berlin();
    let (text, _) = ics::export(&events, &z, secs(dt("2026-11-22T10:00")));
    let target = crate::db::open_memory();
    let rep = ics::import_text(&target, &text, &z).unwrap();
    assert_eq!((rep.imported, rep.skipped.len(), rep.duplicates.len()), (4, 0, 0), "{rep:?}");
    let back = store::all_events(&target).unwrap();
    for e in &events {
        let b = back.iter().find(|b| b.uid == e.uid).unwrap();
        assert_eq!((&b.title, &b.note, &b.color, b.all_day, b.minutes, b.reminder), (&e.title, &e.note, &e.color, e.all_day, e.minutes, e.reminder), "{}", e.title);
        assert_eq!(b.recur, e.recur, "{}", e.title);
        let range = (dt("2026-11-01"), dt("2027-03-01"));
        assert_eq!(b.occurrences(range.0, range.1, &z), e.occurrences(range.0, range.1, &z), "{}", e.title);
        assert_eq!(b.origin, "import");
    }
    // erneuter Import: alles doppelt
    let rep = ics::import_text(&target, &text, &z).unwrap();
    assert_eq!((rep.imported, rep.duplicates.len()), (0, 4));
}

#[test]
fn import_of_foreign_files_with_time_zones_and_a_report() {
    let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:America/New_York\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:20070311T020000\r\nTZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\r\nEND:DAYLIGHT\r\n\
BEGIN:STANDARD\r\nDTSTART:20071104T020000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nRRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:ny@x\r\nDTSTART;TZID=America/New_York:20261120T090000\r\nDTEND;TZID=America/New_York:20261120T100000\r\nSUMMARY:New York call\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:utc@x\r\nDTSTART:20260701T120000Z\r\nDURATION:PT45M\r\nSUMMARY:UTC meeting\r\nBEGIN:VALARM\r\nTRIGGER:-PT1H\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@x\r\nDTSTART:20261102T080000\r\nDTEND:20261102T083000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=3\r\nEXDATE:20261109T080000\r\nSUMMARY:Gym\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@x\r\nRECURRENCE-ID:20261116T080000\r\nDTSTART:20261116T090000\r\nDTEND:20261116T093000\r\nSUMMARY:Gym (later)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:every2@x\r\nDTSTART;VALUE=DATE:20261101\r\nRRULE:FREQ=DAILY;INTERVAL=2\r\nSUMMARY:Every other day\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:nostart@x\r\nSUMMARY:Broken: no start\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:baddate@x\r\nDTSTART:2026-11-01\r\nSUMMARY:Broken: bad date\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:backwards@x\r\nDTSTART:20261101T100000\r\nDTEND:20261101T090000\r\nSUMMARY:Broken: backwards\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:dup@x\r\nDTSTART:20261101T100000\r\nSUMMARY:Dup A\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:dup@x\r\nDTSTART:20261102T100000\r\nSUMMARY:Dup B\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:tz@x\r\nDTSTART;TZID=Mars/Olympus:20261101T100000\r\nSUMMARY:Unknown zone\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:cancel@x\r\nDTSTART:20261101T100000\r\nSTATUS:CANCELLED\r\nSUMMARY:Cancelled\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:open@x\r\nDTSTART:20261101T100000\r\nSUMMARY:Never closed\r\n\
BEGIN:VEVENT\r\nUID:after@x\r\nDTSTART:20261103T100000\r\nSUMMARY:After the broken one\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
    let c = crate::db::open_memory();
    let z = RuleZone::berlin();
    let rep = ics::import_text(&c, text, &z).unwrap();
    let all = store::all_events(&c).unwrap();
    let by = |t: &str| all.iter().find(|e| e.title == t).unwrap_or_else(|| panic!("{t} fehlt: {rep:?}"));
    assert_eq!(by("New York call").start, dt("2026-11-20T15:00"), "09:00 EST = 14:00 UTC = 15:00 Berlin");
    assert_eq!(by("New York call").minutes, 60);
    let utc = by("UTC meeting");
    assert_eq!((utc.start, utc.minutes, utc.reminder), (dt("2026-07-01T14:00"), 45, Some(60)), "Sommerzeit: UTC+2");
    let gym = by("Gym");
    let r = gym.recur.as_ref().unwrap();
    assert_eq!((r.cron.as_str(), r.count), ("0 8 * * MON", Some(3)));
    assert_eq!(r.except, [d("2026-11-09"), d("2026-11-16")], "EXDATE und das geänderte Vorkommen");
    assert_eq!(by("Gym (later)").uid, "weekly@x/20261116");
    assert!(by("Every other day").recur.is_none(), "INTERVAL=2 täglich geht nicht als Cron");
    by("Unknown zone");
    by("After the broken one");
    assert_eq!(rep.imported, 8, "{rep:?}");
    assert_eq!(rep.duplicates, ["Dup B"]);
    let skipped: Vec<&str> = rep.skipped.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(skipped, ["Broken: no start", "Broken: bad date", "Broken: backwards", "Cancelled", "Never closed"]);
    assert!(rep.skipped.iter().all(|s| s.line > 0 && !s.en.is_empty() && !s.de.is_empty()));
    assert_eq!(rep.notes.len(), 2, "INTERVAL und unbekannte Zone: {:?}", rep.notes);
    assert!(rep.summary(Lang::En).starts_with("8 imported, 1 duplicates skipped, 5 broken entries skipped"));
    assert!(rep.summary(Lang::De).starts_with("8 importiert"));
}

#[test]
fn import_limits_and_non_ics_input() {
    let z = RuleZone::utc();
    let big = vec![b'x'; (ics::MAX_FILE_BYTES + 1) as usize];
    assert!(matches!(ics::decode(&big), Err(ics::FileError::TooLarge(_))));
    assert!(matches!(ics::decode(&[0xff, 0xfe, 0x00]), Err(ics::FileError::NotUtf8)));
    assert_eq!(ics::decode(b"\xEF\xBB\xBFBEGIN:VCALENDAR").unwrap(), "BEGIN:VCALENDAR", "BOM erlaubt");
    let c = crate::db::open_memory();
    let rep = ics::import_text(&c, "hello world", &z).unwrap();
    assert_eq!((rep.imported, rep.skipped.len()), (0, 1));
    // Datei: zu groß wird vor dem Lesen abgelehnt
    let dir = std::env::temp_dir().join(format!("paneldeck-test-cal-ics-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("big.ics");
    std::fs::write(&p, &big).unwrap();
    let e = ics::import_file(&c, &p, &z).unwrap_err();
    assert!(e.message(Lang::En).contains("10 MB") && e.message(Lang::De).contains("10 MB"));
    let p2 = dir.join("ok.ics");
    std::fs::write(&p2, "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART:20261101T100000\nSUMMARY:LF only\nEND:VEVENT\nEND:VCALENDAR\n").unwrap();
    assert_eq!(ics::import_file(&c, &p2, &z).unwrap().imported, 1, "LF-Zeilenenden gehen auch");
    // Export in eine Datei
    let out = dir.join("out.ics");
    assert_eq!(ics::export_file(&c, &out, &z).unwrap().events, 1);
    assert!(std::fs::read_to_string(&out).unwrap().contains("SUMMARY:LF only"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ───────────────────────── Erinnerungen

#[test]
fn reminders_fire_once_in_the_window_and_report_missed_ones() {
    let z = RuleZone::berlin();
    let mut e = ev("2026-11-23T09:00", 30, Some("0 9 * * *"));
    e.reminder = Some(15);
    let at = |s: &str| z.instant(dt(s));
    // 08:45 fällig
    let r = due_reminders(std::slice::from_ref(&e), at("2026-11-23T08:45"), at("2026-11-23T08:44"), &z);
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].start, r[0].missed, r[0].minutes_before), (dt("2026-11-23T09:00"), false, 15));
    assert!(!r[0].text(Lang::En).is_empty() && !r[0].text(Lang::De).is_empty());
    // nicht doppelt
    assert!(due_reminders(std::slice::from_ref(&e), at("2026-11-23T08:46"), at("2026-11-23T08:45"), &z).is_empty());
    // Programm war zwei Tage zu: verpasst
    let r = due_reminders(std::slice::from_ref(&e), at("2026-11-25T12:00"), at("2026-11-23T12:00"), &z);
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|x| x.missed) && r[0].text(Lang::En).starts_with("Missed:"));
    // ohne Erinnerung nichts
    let plain = ev("2026-11-23T09:00", 30, None);
    assert!(due_reminders(&[plain], at("2026-11-23T09:00"), at("2026-11-23T08:00"), &z).is_empty());
    // doppelte Stunde: 02:30 am 25.10. löst einmal aus
    let mut n = ev("2026-10-20T02:30", 30, Some("30 2 * * *"));
    n.reminder = Some(0);
    let r = due_reminders(&[n], at("2026-10-25T04:00"), at("2026-10-25T00:00"), &z);
    assert_eq!(r.len(), 1);
}

#[test]
fn poll_respects_the_option_and_remembers_the_last_check() {
    let c = crate::db::open_memory();
    let z = RuleZone::utc();
    let mut i = EventInput::new("Call", dt("2026-11-23T09:00"), 30);
    i.reminder = Some(5);
    store::create_event(&c, i, None, "local").unwrap();
    let at = |s: &str| z.instant(dt(s));
    assert!(poll(&c, at("2026-11-23T08:00"), &z).is_empty(), "erster Aufruf holt nichts nach");
    assert!(poll(&c, at("2026-11-23T08:56"), &z).is_empty(), "Erinnerungen an Termine sind standardmäßig aus");
    let mut o = store::load_options(&c);
    o.reminders = true;
    store::save_options(&c, &o).unwrap();
    store::set_last_reminder_check(&c, at("2026-11-23T08:50")).unwrap();
    let r = poll(&c, at("2026-11-23T08:56"), &z);
    assert_eq!(r.len(), 1);
    assert!(poll(&c, at("2026-11-23T08:57"), &z).is_empty());
}

// ───────────────────────── MCP

fn call(c: &rusqlite::Connection, tool: &str, args: Value) -> Result<crate::planning::agent::Done, crate::planning::agent::ToolError> {
    let z = RuleZone::berlin();
    let ctx = super::agent::Ctx { conn: c, lang: Lang::En, now: dt("2026-11-22T10:00"), zone: &z, view: None };
    super::agent::call(&ctx, tool, &args)
}

#[test]
fn mcp_tools_are_registered_with_levels_and_descriptions() {
    use crate::mcp::McpLevel;
    for (n, l) in [("list_calendar_events", McpLevel::Read), ("create_calendar_event", McpLevel::Control), ("update_calendar_event", McpLevel::Control)] {
        assert_eq!(crate::mcp::tool_level(n), Some(l), "{n}");
        let (de, en) = super::agent::short_desc(n).unwrap();
        assert!(!de.is_empty() && !en.is_empty() && de != en);
    }
    assert!(crate::mcp::tools().iter().all(|t| !t.name.contains("delete_calendar")), "kein Löschen");
}

#[test]
fn mcp_create_list_update() {
    let c = crate::db::open_memory();
    let d1 = call(&c, "create_calendar_event", json!({"title": "Standup", "start": "2026-11-23T09:00", "duration_minutes": 15, "cron": "0 9 * * MON-FRI", "count": 10, "reminder_minutes": 5, "color": "#61AFEF"})).unwrap();
    assert!(d1.change.is_some());
    let e = &d1.result["event"];
    assert_eq!(e["recurrence"]["cron"], "0 9 * * MON-FRI");
    assert_eq!(e["recurrence"]["description"], "Weekdays (Mon–Fri) at 09:00");
    assert_eq!(e["origin"], "agent");
    assert_eq!(d1.result["next_occurrences"].as_array().unwrap().len(), 5);
    assert_eq!(d1.result["next_occurrences"][0], "2026-11-23T09:00:00");
    assert!(d1.result["warning"].is_null());
    let id = e["event_id"].as_i64().unwrap();
    // ganztägig über das Datum
    let d2 = call(&c, "create_calendar_event", json!({"title": "Trip", "start": "2026-11-25", "days": 3})).unwrap();
    assert_eq!((d2.result["event"]["all_day"].as_bool(), d2.result["event"]["duration_minutes"].as_i64()), (Some(true), Some(3 * 1440)));
    // ODER-Fall: Warnung
    let d3 = call(&c, "create_calendar_event", json!({"title": "Odd", "start": "2026-11-13T00:00", "cron": "0 0 13 * FRI"})).unwrap();
    assert!(d3.result["warning"].as_str().unwrap().contains("EITHER"));
    // Liste: seitenweise (Woche 23.–29.11.: 5 Standups, die Reise, Freitag 27.11. aus dem ODER-Fall)
    let l = call(&c, "list_calendar_events", json!({"from": "2026-11-23", "to": "2026-11-29", "limit": 3})).unwrap();
    assert!(l.change.is_none());
    let r = &l.result;
    assert_eq!(r["total"], 7, "{r}");
    assert_eq!(r["items"].as_array().unwrap().len(), 3);
    assert_eq!(r["more"], true);
    assert_eq!(r["timezone"], "Europe/Berlin");
    assert!(r["note"].as_str().unwrap().contains("not instructions"));
    let first = &r["items"][0];
    assert_eq!((first["kind"].as_str(), first["event_id"].as_i64()), (Some("event"), Some(id)));
    assert_eq!(first["reminder_minutes"], 5);
    let l2 = call(&c, "list_calendar_events", json!({"from": "2026-11-23", "to": "2026-11-29", "offset": 6})).unwrap();
    assert_eq!((l2.result["items"].as_array().unwrap().len(), l2.result["more"].as_bool()), (1, Some(false)));
    // ändern: nur die genannten Felder; null entfernt
    let u = call(&c, "update_calendar_event", json!({"event": id, "title": "Daily standup", "reminder_minutes": null, "exceptions": ["2026-11-24"]})).unwrap();
    let ue = &u.result["event"];
    assert_eq!((ue["title"].as_str(), ue["duration_minutes"].as_i64(), ue["reminder_minutes"].is_null()), (Some("Daily standup"), Some(15), true));
    assert_eq!(ue["recurrence"]["exceptions"], json!(["2026-11-24"]));
    assert_eq!(ue["recurrence"]["count"], 10, "unverändert");
    let u = call(&c, "update_calendar_event", json!({"event": id, "cron": null})).unwrap();
    assert!(u.result["event"]["recurrence"].is_null());
}

#[test]
fn mcp_errors() {
    let c = crate::db::open_memory();
    let code = |tool: &str, a: Value| call(&c, tool, a).unwrap_err().code;
    assert_eq!(code("create_calendar_event", json!({"start": "2026-11-23T09:00"})), "bad_args");
    assert_eq!(code("create_calendar_event", json!({"title": "x"})), "bad_args");
    assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "tomorrow"})), "bad_args");
    assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "2026-11-23T09:00", "cron": "61 * * * *"})), "bad_args");
    assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "2026-11-23T09:00", "count": 3})), "bad_args", "count ohne cron");
    assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "2026-11-23T09:00", "project": "NOPE"})), "not_found");
    assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "2026-11-23T09:00", "color": "red"})), "bad_args");
    assert_eq!(code("update_calendar_event", json!({"event": 999, "title": "y"})), "not_found");
    assert_eq!(code("update_calendar_event", json!({"title": "y"})), "bad_args");
    assert_eq!(code("list_calendar_events", json!({"from": "2026-11-30", "to": "2026-11-01"})), "bad_args");
    assert_eq!(code("list_calendar_events", json!({"from": "2026-01-01", "to": "2027-06-01"})), "bad_args", "höchstens 366 Tage");
    assert_eq!(code("list_calendar_events", json!({"from": "11/30/2026"})), "bad_args");
    assert_eq!(code("delete_calendar_event", json!({})), "not_found");
}

#[test]
fn mcp_schemas_match_the_handlers() {
    for t in super::agent::defs() {
        assert_eq!(t.schema["additionalProperties"], false, "{}", t.name);
        assert!(t.description.len() > 80, "{}", t.name);
    }
    let upd = super::agent::defs().into_iter().find(|t| t.name == "update_calendar_event").unwrap();
    for k in ["event", "title", "start", "all_day", "duration_minutes", "days", "note", "color", "reminder_minutes", "cron", "until", "count", "exceptions", "project"] {
        assert!(upd.schema["properties"].get(k).is_some(), "{k}");
    }
    assert_eq!(super::agent::log_info("create_calendar_event", &json!({"title": "A", "start": "2026-11-23", "note": "secret"})), "2026-11-23 A");
    assert_eq!(super::agent::log_info("update_calendar_event", &json!({"event": 4, "note": "secret"})), "event 4");
}

// ───────────────────────── Migration, Sicherung, Leistung

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("paneldeck-test-cal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn migration_adds_the_calendar_to_an_old_database() {
    // Datenbank im Stand vor dem Kalender (alle Migrationen davor, egal an welcher Stelle er steht) mit Daten
    let dir = tmp("mig");
    let latest = crate::db::schema_version();
    let idx = crate::db::migration_index(store::MIGRATION);
    {
        let old = crate::db::open_prefix(&dir, &dir.join("backups"), idx).unwrap();
        assert!(old.conn.prepare("SELECT * FROM cal_events").is_err(), "noch kein Kalender");
        old.conn.execute("INSERT INTO todo_items (title, body, done, position, created_at, updated_at) VALUES ('keep me', '', 0, 1, 'x', 'x')", []).unwrap();
    }
    let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
    let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(v, latest);
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM todo_items WHERE title = 'keep me'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "alte Daten bleiben");
    let e = store::create_event(&db.conn, EventInput::new("New", dt("2026-11-23T09:00"), 60), None, "local").unwrap();
    assert_eq!(store::get_event(&db.conn, e.id).unwrap().title, "New");
    assert_eq!(store::load_options(&db.conn), Options::default());
    assert_eq!(crate::backup::list(&dir.join("backups")).len(), 1, "Sicherung vor der Migration");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn calendar_data_is_part_of_backup_and_restore() {
    let dir = tmp("bak");
    let bdir = dir.join("backups");
    let db = crate::db::open(&dir, &bdir).unwrap();
    store::create_event(&db.conn, EventInput::new("Before backup", dt("2026-11-23T09:00"), 60), None, "local").unwrap();
    let mut o = store::load_options(&db.conn);
    o.reminders = true;
    store::save_options(&db.conn, &o).unwrap();
    let b = crate::backup::create(&db.path, &bdir, crate::backup::Kind::Manual).unwrap();
    store::create_event(&db.conn, EventInput::new("After backup", dt("2026-11-24T09:00"), 60), None, "local").unwrap();
    drop(db);
    crate::backup::schedule_restore(&dir, &b.path).unwrap();
    crate::backup::apply_pending_restore(&dir, crate::db::FILE).unwrap().unwrap();
    let db = crate::db::open(&dir, &bdir).unwrap();
    let titles: Vec<String> = store::all_events(&db.conn).unwrap().into_iter().map(|e| e.title).collect();
    assert_eq!(titles, ["Before backup"]);
    assert!(store::load_options(&db.conn).reminders);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thousand_events_with_recurrences_expand_fast_for_a_month() {
    let c = crate::db::open_memory();
    let crons = ["0 9 * * MON-FRI", "*/30 8-18 * * *", "0 12 15 * *", "0 18 * * MON#1", "0 0 * * SAT,SUN"];
    c.execute_batch("BEGIN").unwrap();
    for i in 0..1000i64 {
        let start = dt("2026-01-01T08:00") + chrono::Duration::hours(i * 7);
        let mut inp = EventInput::new(&format!("Event {i}"), start, 30 + (i % 5) * 15);
        if i % 4 == 0 {
            inp.recur = Some(Recurrence { cron: crons[(i / 4) as usize % crons.len()].into(), count: if i % 8 == 0 { Some(200) } else { None }, ..Default::default() });
        }
        store::create_event(&c, inp, None, "local").unwrap();
    }
    c.execute_batch("COMMIT").unwrap();
    let t = std::time::Instant::now();
    let items = sources::collect(&c, &Options::default(), d("2026-11-23"), d("2027-01-04"), &RuleZone::berlin());
    let ms = t.elapsed().as_millis();
    assert!(items.len() > 1000, "{}", items.len());
    assert!(ms < 1500, "Monatsansicht mit 1000 Terminen: {ms} ms");
}

#[test]
fn bilingual_texts_differ() {
    for k in [Kind::Event, Kind::Due, Kind::Todo, Kind::Sprint, Kind::Version, Kind::Milestone, Kind::Time] {
        assert_ne!(k.label(Lang::De), "", "{k:?}");
        assert!(!k.as_str().is_empty());
    }
    let e = store::StoreError::NotFound;
    assert_ne!(e.message(Lang::De), e.message(Lang::En));
}

// ───────────────────────── Härtung (Review): Grenzen, kaputte Eingaben, Leistung

#[test]
fn extreme_years_are_refused_everywhere() {
    for s in ["+262142-12-31", "262142-12-31", "-0001-01-01", "1899-12-31", "2201-01-01", "+2026-11-23", "99999-01-01T00:00"] {
        assert!(parse_when(s).is_none() && super::parse_date(s).is_none(), "{s}");
    }
    assert!(super::parse_date("2200-12-31").is_some() && super::parse_date("1900-01-01").is_some());
    assert_eq!(super::add_minutes(NaiveDateTime::MAX, 10), NaiveDateTime::MAX, "gesättigt statt Panik");
    assert_eq!(super::add_days(NaiveDate::MIN, -10), NaiveDate::MIN);
    let c = crate::db::open_memory();
    let code = |tool: &str, a: Value| call(&c, tool, a).map(|_| "ok").unwrap_or_else(|e| e.code);
    for bad in ["+262142-12-31", "262142-12-31", "2201-01-01"] {
        assert_eq!(code("list_calendar_events", json!({"from": bad})), "bad_args", "{bad}");
        assert_eq!(code("list_calendar_events", json!({"from": "2026-11-01", "to": bad})), "bad_args", "{bad}");
        assert_eq!(code("create_calendar_event", json!({"title": "x", "start": format!("{bad}T00:00")})), "bad_args", "{bad}");
        assert_eq!(code("create_calendar_event", json!({"title": "x", "start": bad})), "bad_args", "{bad}");
        assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "2026-11-23T09:00", "cron": "@daily", "until": bad})), "bad_args", "{bad}");
        assert_eq!(code("create_calendar_event", json!({"title": "x", "start": "2026-11-23T09:00", "cron": "@daily", "exceptions": [bad]})), "bad_args", "{bad}");
    }
    // am Rand des erlaubten Bereichs: rechnet ohne Panik
    let d = call(&c, "create_calendar_event", json!({"title": "Edge", "start": "2200-12-31T23:30", "duration_minutes": 527040, "cron": "* * * * *"})).unwrap();
    let id = d.result["event"]["event_id"].as_i64().unwrap();
    assert_eq!(code("list_calendar_events", json!({"from": "2200-12-01"})), "ok", "to = from + 30 Tage liegt hinter 2200");
    assert_eq!(code("list_calendar_events", json!({"from": "1900-01-01", "to": "1900-12-31"})), "ok");
    assert_eq!(code("update_calendar_event", json!({"event": id, "start": "+262142-12-31T00:00"})), "bad_args");
    assert_eq!(code("update_calendar_event", json!({"event": id, "start": "1900-01-01", "all_day": true, "days": 366})), "ok");
    // Dialog
    let mut f = crate::app::calendar_ui::test_form(dt("2026-11-23T09:00"));
    f.title = "x".into();
    f.date = "+262142-12-31".into();
    assert!(f.input(Lang::En).is_err());
    f.date = "2200-12-31".into();
    f.time = "23:59".into();
    f.minutes = 527040;
    assert!(f.input(Lang::En).is_ok());
    let z = RuleZone::berlin();
    let env = crate::app::calendar_ui::CalEnv { conn: Some(&c), lang: Lang::En, now: dt("2026-11-23T09:00"), zone: &z };
    let _ = f.preview(&env);
}

/// Einfacher Pseudo-Zufall (fest, wiederholbar).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[(self.next() as usize) % v.len()]
    }
}

#[test]
fn malformed_rrules_and_timezones_never_panic() {
    let parts = [
        "FREQ=", "DAILY", "WEEKLY", "MONTHLY", "YEARLY", "MINUTELY", "HOURLY", "SECONDLY", ";", "=", ",", "BYDAY=", "ÄM", "MO", "1MO", "-1SU", "+5FR", "99999999999MO", "BYMONTH=", "13", "0", "-5",
        "BYMONTHDAY=", "BYHOUR=", "BYMINUTE=", "INTERVAL=", "COUNT=", "UNTIL=", "99991231T235959Z", "ß", "€", "Ö1", "BYSETPOS=", "2147483647", "9223372036854775807", "", " ", "\u{1F600}",
    ];
    let mut r = Lcg(42);
    let start = dt("2026-11-02T09:30");
    for _ in 0..20_000 {
        let n = 1 + r.next() % 8;
        let s: String = (0..n).map(|_| r.pick(&parts)).collect();
        let _ = ics::rrule_to_cron(&s, start, r.next().is_multiple_of(2));
    }
    for s in ["BYDAY=ÄM", "FREQ=MONTHLY;BYDAY=Ä", "FREQ=YEARLY;BYDAY=1ÄÖ", "FREQ=WEEKLY;BYDAY=€", "FREQ=DAILY;BYDAY=ßß"] {
        assert!(ics::rrule_to_cron(s, start, false).is_err(), "{s}");
    }
    // ganze Dateien mit kaputten VTIMEZONE- und VEVENT-Werten
    let tz_rrule = ["FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU", "FREQ=YEARLY;BYMONTH=99;BYDAY=2147483647SU", "BYDAY=ÄM", "BYDAY=", "BYMONTH=x", "FREQ=YEARLY;BYDAY=-9SU", ""];
    let tz_off = ["+0100", "+9999", "-99", "abc", "+01ÄÖ", ""];
    let tz_start = ["19700329T020000", "99991231T235959", "00000101T000000", "x", ""];
    let ev_start = ["DTSTART:20261123T090000", "DTSTART:99991231T235959Z", "DTSTART;VALUE=DATE:00010101", "DTSTART;TZID=Z1:20261123T090000", "DTSTART:2026", "DTSTART;TZID=\"Ä\":20261123T090000"];
    let ev_more = [
        "DURATION:P99999999999999W", "DURATION:-PT1H", "DURATION:PÄ", "DTEND:99991231T235959", "EXDATE:ÄÖÜ,20261124T090000", "RRULE:FREQ=YEARLY;BYMONTHDAY=1", "RRULE:BYDAY=ÄM", "RECURRENCE-ID:20261124T090000",
        "STATUS:CANCELLED", "X-PANELDECK-RANGE:UNTIL=Ä;COUNT=99999999999", "X-PANELDECK-CRON:Ä", "X-PANELDECK-EXCEPT:Ä",
    ];
    let z = RuleZone::berlin();
    for i in 0..1500 {
        let text = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Z1\r\nBEGIN:DAYLIGHT\r\nDTSTART:{}\r\nTZOFFSETTO:{}\r\nRRULE:{}\r\nEND:DAYLIGHT\r\nBEGIN:STANDARD\r\nDTSTART:{}\r\nTZOFFSETTO:{}\r\nRRULE:{}\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:u{i}\r\n{}\r\n{}\r\n{}\r\nSUMMARY:Ä{i}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            r.pick(&tz_start),
            r.pick(&tz_off),
            r.pick(&tz_rrule),
            r.pick(&tz_start),
            r.pick(&tz_off),
            r.pick(&tz_rrule),
            r.pick(&ev_start),
            r.pick(&ev_more),
            r.pick(&ev_more)
        );
        let (items, _) = ics::parse(&text, &|_| false, &z);
        for p in items {
            // was durchkommt, ist gültig und lässt sich ausrechnen
            let e = Event {
                id: 1,
                uid: p.uid,
                title: p.input.title,
                note: String::new(),
                color: String::new(),
                all_day: p.input.all_day,
                start: p.input.start,
                minutes: p.input.minutes,
                recur: p.input.recur,
                reminder: None,
                project_id: None,
                origin: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
            };
            let _ = e.next_occurrences(e.start, 3, &z);
        }
    }
}

#[test]
fn yearly_rules_without_bymonth_keep_every_month() {
    let start = dt("2026-11-02T09:30");
    let c = |r: &str| ics::rrule_to_cron(r, start, false).map(|x| x.cron).unwrap();
    assert_eq!(c("FREQ=YEARLY;BYMONTHDAY=1"), "30 9 1 * *", "jeder Monatserste");
    assert_eq!(c("FREQ=YEARLY;BYDAY=MO"), "30 9 * * MON", "jeder Montag");
    assert_eq!(c("FREQ=YEARLY;BYMONTH=6"), "30 9 2 6 *");
}

#[test]
fn cancelled_instances_become_exceptions_of_the_series() {
    let text = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:w@x\r\nDTSTART:20261102T080000\r\nDTEND:20261102T083000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\nSUMMARY:Weekly\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:w@x\r\nRECURRENCE-ID:20261109T080000\r\nDTSTART:20261109T080000\r\nSTATUS:CANCELLED\r\nSUMMARY:Weekly\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:c@x\r\nDTSTART:20261110T080000\r\nSTATUS:CANCELLED\r\nSUMMARY:Cancelled single\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
    let (items, rep) = ics::parse(text, &|_| false, &RuleZone::utc());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].input.recur.as_ref().unwrap().except, [d("2026-11-09")]);
    assert_eq!(rep.skipped.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Cancelled single"], "abgesagtes Vorkommen ist kein Fehler");
}

#[test]
fn export_names_the_time_when_dtstart_falls_into_the_dst_gap() {
    let c = crate::db::open_memory();
    let mut i = EventInput::new("Night job", dt("2026-03-29T02:30"), 15);
    i.recur = Some(Recurrence { cron: "30 2 * * *".into(), ..Default::default() });
    store::create_event(&c, i, Some("night@test"), "local").unwrap();
    let z = RuleZone::berlin();
    let (text, _) = ics::export(&store::all_events(&c).unwrap(), &z, secs(dt("2026-03-01T00:00")));
    assert!(text.contains("DTSTART;TZID=Europe/Berlin:20260329T030000\r\n"), "{text}");
    assert!(text.contains("RRULE:FREQ=DAILY;BYHOUR=2;BYMINUTE=30\r\n"), "{text}");
    // Rundreise: gleiche Vorkommen
    let target = crate::db::open_memory();
    ics::import_text(&target, &text, &z).unwrap();
    let a = store::all_events(&c).unwrap()[0].occurrences(dt("2026-03-28"), dt("2026-04-05"), &z);
    let b = store::all_events(&target).unwrap()[0].occurrences(dt("2026-03-28"), dt("2026-04-05"), &z);
    assert_eq!(a, b);
}

#[test]
fn next_occurrences_of_frequent_long_events_are_found() {
    let z = RuleZone::utc();
    let e = ev("2026-11-01T00:00", 3 * 1440, Some("*/5 * * * *"));
    let n = e.next_occurrences(dt("2026-11-20T10:00"), 5, &z);
    assert_eq!(fmt(&n), ["2026-11-20 10:00", "2026-11-20 10:05", "2026-11-20 10:10", "2026-11-20 10:15", "2026-11-20 10:20"]);
    let occ = e.occurrences(dt("2026-11-20"), dt("2026-12-20"), &z);
    assert_eq!(occ.iter().filter(|s| **s >= dt("2026-11-20")).count(), super::MAX_OCC_PER_RANGE, "Grenze nur für Vorkommen ab `from`");
}

#[test]
fn exhausted_count_series_are_skipped_by_the_query() {
    let c = crate::db::open_memory();
    let mut i = EventInput::new("Short series", dt("2026-01-05T09:00"), 30);
    i.recur = Some(Recurrence { cron: "0 9 * * MON".into(), count: Some(3), ..Default::default() });
    let short = store::create_event(&c, i, None, "local").unwrap().id;
    let mut i = EventInput::new("Every minute", dt("2026-01-01T00:00"), 1);
    i.recur = Some(Recurrence { cron: "* * * * *".into(), count: Some(10_000), ..Default::default() });
    let minutes = store::create_event(&c, i, None, "local").unwrap().id;
    let ids = |from: &str, to: &str| store::events_for_range(&c, dt(from), dt(to)).unwrap().iter().map(|e| e.id).collect::<Vec<_>>();
    assert!(ids("2026-01-01", "2026-02-01").contains(&short));
    assert!(!ids("2026-03-01", "2026-04-01").contains(&short), "nach dem dritten Vorkommen vorbei");
    assert!(ids("2026-01-07", "2026-01-08").contains(&minutes), "10 000 Minuten ≈ 7 Tage");
    assert!(!ids("2026-02-01", "2026-03-01").contains(&minutes));
}

#[test]
fn list_paging_uses_the_cache_and_sees_changes() {
    let c = crate::db::open_memory();
    call(&c, "create_calendar_event", json!({"title": "A", "start": "2026-11-23T09:00", "cron": "*/30 * * * *"})).unwrap();
    let p1 = call(&c, "list_calendar_events", json!({"from": "2026-11-23", "to": "2026-11-29", "limit": 10})).unwrap();
    let p2 = call(&c, "list_calendar_events", json!({"from": "2026-11-23", "to": "2026-11-29", "limit": 10, "offset": 10})).unwrap();
    assert_eq!(p1.result["total"], p2.result["total"]);
    assert_ne!(p1.result["items"][0]["start"], p2.result["items"][0]["start"]);
    call(&c, "create_calendar_event", json!({"title": "B", "start": "2026-11-23T09:01"})).unwrap();
    let p3 = call(&c, "list_calendar_events", json!({"from": "2026-11-23", "to": "2026-11-29", "limit": 10})).unwrap();
    assert_eq!(p3.result["total"].as_u64().unwrap(), p1.result["total"].as_u64().unwrap() + 1, "Änderung leert den Zwischenspeicher");
}

#[test]
fn poller_writes_the_database_rarely() {
    use super::remind::{Poller, PERSIST_EVERY_SECS};
    let c = crate::db::open_memory();
    let z = RuleZone::utc();
    let mut i = EventInput::new("Call", dt("2026-11-23T09:00"), 30);
    i.reminder = Some(0);
    store::create_event(&c, i, None, "local").unwrap();
    let mut o = store::load_options(&c);
    o.reminders = true;
    store::save_options(&c, &o).unwrap();
    let at = |s: &str| z.instant(dt(s));
    let mut p = Poller::default();
    assert!(p.poll(&c, at("2026-11-23T08:50"), &z).is_empty());
    assert_eq!(store::last_reminder_check(&c), Some(at("2026-11-23T08:50")), "erster Zeitpunkt gespeichert");
    assert!(p.poll(&c, at("2026-11-23T08:50:30"), &z).is_empty());
    assert_eq!(store::last_reminder_check(&c), Some(at("2026-11-23T08:50")), "nicht bei jedem Aufruf geschrieben");
    let r = p.poll(&c, at("2026-11-23T09:00:10"), &z);
    assert_eq!(r.len(), 1);
    assert_eq!(store::last_reminder_check(&c), Some(at("2026-11-23T09:00:10")), "nach einer Erinnerung sofort");
    assert!(p.poll(&c, at("2026-11-23T09:00:40"), &z).is_empty());
    let later = at("2026-11-23T09:00:10") + PERSIST_EVERY_SECS;
    assert!(p.poll(&c, later, &z).is_empty());
    assert_eq!(store::last_reminder_check(&c), Some(later), "spätestens alle {PERSIST_EVERY_SECS} s");
    // nach einem Neustart nichts doppelt
    let mut p2 = Poller::default();
    assert!(p2.poll(&c, later + 30, &z).is_empty());
}

#[test]
fn a_thousand_events_expand_fast_in_the_system_zone_too() {
    let c = crate::db::open_memory();
    c.execute_batch("BEGIN").unwrap();
    for i in 0..1000i64 {
        let start = dt("2026-01-01T08:00") + chrono::Duration::hours(i * 7);
        let mut inp = EventInput::new(&format!("Event {i}"), start, 45);
        if i % 4 == 0 {
            inp.recur = Some(Recurrence { cron: ["0 9 * * MON-FRI", "*/30 8-18 * * *", "0 18 * * MON#1"][(i / 4) as usize % 3].into(), ..Default::default() });
        }
        store::create_event(&c, inp, None, "local").unwrap();
    }
    c.execute_batch("COMMIT").unwrap();
    let z = super::zone::SystemZone;
    let t = std::time::Instant::now();
    let items = sources::collect(&c, &Options::default(), d("2026-11-23"), d("2027-01-04"), &z);
    let ms = t.elapsed().as_millis();
    assert!(items.len() > 1000);
    assert!(ms < 2500, "Monatsansicht mit 1000 Terminen in der Rechner-Zone: {ms} ms");
    let ctx = super::agent::Ctx { conn: &c, lang: Lang::En, now: dt("2026-11-22T10:00"), zone: &z, view: None };
    let t = std::time::Instant::now();
    for k in 0..5 {
        super::agent::call(&ctx, "list_calendar_events", &json!({"from": "2026-11-01", "to": "2027-10-31", "limit": 500, "offset": k * 500})).unwrap();
    }
    assert!(t.elapsed().as_millis() < 8000, "fünf Seiten eines Jahres: {} ms", t.elapsed().as_millis());
}

#[test]
fn agents_see_events_and_sources_only_of_reachable_areas() {
    use crate::access::AgentView;
    use crate::mcp::McpLevel;
    let c = crate::db::open_memory();
    let pid: i64 = c.query_row("SELECT id FROM projects WHERE system = 0 ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap_or_else(|_| {
        crate::planning::projects::create_project(&c, &crate::planning::projects::NewProject::new("CAL", "Cal", "#336699")).unwrap()
    });
    let key: String = c.query_row("SELECT key FROM projects WHERE id = ?1", [pid], |r| r.get(0)).unwrap();
    call(&c, "create_calendar_event", json!({"title": "Open", "start": "2026-11-23T09:00"})).unwrap();
    call(&c, "create_calendar_event", json!({"title": "Secret", "start": "2026-11-23T10:00", "project": key})).unwrap();
    let (card, _) = crate::tasks::create_item_ext(&c, "Private due", "", Some("2026-11-24"), &[], &crate::tasks::ItemMeta { private: true, ..Default::default() }).unwrap();
    let z = RuleZone::berlin();
    let view = AgentView { card_general: McpLevel::Control, general: McpLevel::Control, ..Default::default() };
    let ctx = super::agent::Ctx { conn: &c, lang: Lang::En, now: dt("2026-11-22T10:00"), zone: &z, view: Some(&view) };
    let d = super::agent::call(&ctx, "list_calendar_events", &json!({"from": "2026-11-22", "to": "2026-11-30"})).unwrap();
    let titles: Vec<&str> = d.result["items"].as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert_eq!(titles, vec!["Open"], "Termin des gesperrten Projekts und private Karte fehlen");
    assert_eq!(d.result["total"], 1);
    let _ = card;
    // Schreiben in das gesperrte Projekt: project_locked; nur lesen in „Allgemein“: forbidden
    let e = super::agent::call(&ctx, "create_calendar_event", &json!({"title": "x", "start": "2026-11-23T09:00", "project": key})).unwrap_err();
    assert_eq!(e.code, crate::access::LOCKED_CODE);
    let ro = AgentView { general: McpLevel::Read, ..Default::default() };
    let ctx = super::agent::Ctx { conn: &c, lang: Lang::En, now: dt("2026-11-22T10:00"), zone: &z, view: Some(&ro) };
    assert_eq!(super::agent::call(&ctx, "create_calendar_event", &json!({"title": "x", "start": "2026-11-23T09:00"})).unwrap_err().code, "forbidden");
    let locked = AgentView::default();
    let ctx = super::agent::Ctx { conn: &c, lang: Lang::En, now: dt("2026-11-22T10:00"), zone: &z, view: Some(&locked) };
    assert_eq!(super::agent::call(&ctx, "create_calendar_event", &json!({"title": "x", "start": "2026-11-23T09:00"})).unwrap_err().code, crate::access::LOCKED_CODE);
}

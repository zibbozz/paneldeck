use super::*;
use std::time::{Duration, SystemTime};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pd-cleanup-{tag}-{}-{}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(p: &Path, n: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![b'x'; n]).unwrap();
}

fn make_db(p: &Path) {
    let c = rusqlite::Connection::open(p).unwrap();
    c.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES ('marker');").unwrap();
}

fn ctx(d: &Path) -> Ctx {
    Ctx {
        data_dir: d.to_path_buf(),
        backup_dir: d.join("backups"),
        safety_dir: None,
        base_dir: d.to_path_buf(),
        open_ids: vec![1, 2],
        exe: PathBuf::from(r"C:\pd\paneldeck.exe"),
        temp_dir: d.join("tmp"),
        settings: Some(crate::state::Settings::default()),
        system_integration: false,
        integ_override: None,
    }
}

fn sel(items: &[Item]) -> Selection {
    Selection { items: items.iter().copied().collect(), profiles: Default::default() }
}

fn sample(d: &Path) {
    put(&d.join("clipboard").join("clip-1-0.png"), 1000);
    put(&d.join("clipboard").join("clip-2-0.png"), 500);
    put(&d.join("state.json.bak-20260101-000000"), 10);
    put(&d.join("settings.defekt-123.json"), 20);
    put(&d.join("crash.log"), 30);
    put(&d.join("mcp-log.jsonl"), 40);
    put(&d.join("state.json"), 50);
    put(&d.join("prompts.json"), 60);
    // Sitzungen 1 und 2 sind offen, 7 und 9 verwaist; notes.txt gehört keiner Sitzung und bleibt immer.
    for (f, n) in [("1.txt", 100), ("7.txt", 200), ("notes.txt", 5)] {
        put(&d.join("scrollback").join(f), n);
    }
    for (f, n) in [("2.json", 300), ("9.json", 400), ("2.orphan-5.json", 7)] {
        put(&d.join("chats").join(f), n);
    }
}

#[test]
fn ids_are_unique_and_round_trip() {
    let mut seen = std::collections::HashSet::new();
    for i in Item::ALL {
        assert!(seen.insert(i.id()), "doppelte Kennung {}", i.id());
        assert_eq!(Item::from_id(i.id()), Some(i));
        for l in [Lang::De, Lang::En] {
            assert!(!i.label(l).is_empty() && !i.hint(l).is_empty());
        }
    }
    assert_eq!(Item::from_id("nope"), None);
    let pre: Vec<Item> = Item::ALL.into_iter().filter(|i| i.preselected()).collect();
    assert_eq!(pre, [Item::Clipboard, Item::OldBackups, Item::ScrollbackOrphaned, Item::ChatsOrphaned, Item::CrashLog]);
    // Unwiderruflich: Datenbank, alle Chats, alle Einstellungen, Sitzungen/Layout, Vorlagen, Prompts, Zurücksetzen
    for i in [Item::Database, Item::ChatsAll, Item::SettingsAll, Item::Sessions, Item::Templates, Item::Prompts, Item::FactoryReset, Item::ScrollbackAll] {
        assert!(i.needs_typing(false), "{}", i.id());
    }
    assert!(!Item::DbBackups.needs_typing(false) && Item::DbBackups.needs_typing(true));
    assert!(!Item::Clipboard.needs_typing(true) && !Item::McpLog.needs_typing(false));
    assert!(Group::ALL.iter().all(|g| Item::ALL.iter().any(|i| i.group() == *g)));
}

#[test]
fn confirm_word_and_typing() {
    assert!(confirm_ok("DELETE") && confirm_ok(" löschen ") && confirm_ok("Löschen") && confirm_ok("delete"));
    assert!(!confirm_ok("") && !confirm_ok("DEL") && !confirm_ok("ja"));
    assert_eq!(confirm_word(Lang::De), "LÖSCHEN");
    assert_eq!(confirm_word(Lang::En), "DELETE");
    let o = Opts::default();
    assert!(!sel(&[Item::Clipboard, Item::CrashLog]).needs_typing(&o));
    assert!(sel(&[Item::Clipboard, Item::Prompts]).needs_typing(&o));
    assert!(sel(&[Item::DbBackups]).needs_typing(&Opts { keep_backups: 0, ..o.clone() }));
    let mut p = sel(&[]);
    p.profiles.insert("work".into());
    assert!(p.needs_typing(&o) && !p.needs_restart());
    assert!(sel(&[Item::Database]).needs_restart() && !sel(&[Item::ChatsAll]).needs_restart());
    assert_eq!(sel(&[Item::FactoryReset]).expanded().len(), Item::ALL.len());
}

#[test]
fn sizes_counts_and_orphans() {
    let d = tmp("scan");
    sample(&d);
    let c = ctx(&d);
    let s = scan(&c);
    let now = SystemTime::now();
    let o = Opts::default();
    let amt = |i: Item| amount(i, s.found(i).unwrap(), &o, now);
    assert_eq!(amt(Item::Clipboard), (1500, 2, 0));
    assert_eq!(amt(Item::OldBackups), (30, 2, 0));
    assert_eq!(amt(Item::CrashLog), (30, 1, 0));
    assert_eq!(amt(Item::McpLog), (40, 1, 0));
    assert_eq!(amt(Item::ScrollbackOrphaned), (200, 1, 0), "nur 7.txt; notes.txt gehört keiner Sitzung");
    assert_eq!(amt(Item::ScrollbackAll), (300, 2, 0));
    assert_eq!(amt(Item::ChatsOrphaned), (407, 2, 0), "9.json und der beiseitegelegte Verlauf");
    assert_eq!(amt(Item::ChatsAll), (707, 3, 0));
    assert_eq!(amt(Item::Sessions).1, 6, "state.json + Scrollback + Chats");
    assert!(!s.visible(Item::RunTemp) && !s.visible(Item::HotExit) && !s.visible(Item::MaintenanceBackups), "optionale Einträge ohne Ordner fehlen");
    put(&d.join("hot-exit").join("a").join("buf.txt"), 3);
    put(&d.join("tmp").join(RUN_TEMP).join("1").join("paneldeck.exe"), 9);
    put(&d.join("backups").join("maintenance-20260101-120000.db"), 11);
    // fremde Dateien mit „maintenance“ im Namen gehören nicht dazu
    put(&d.join("backups").join("maintenance-20260101.zip"), 13);
    put(&d.join("backups").join("my-maintenance-notes.db"), 13);
    let s = scan(&c);
    assert!(s.visible(Item::RunTemp) && s.visible(Item::HotExit) && s.visible(Item::MaintenanceBackups));
    assert_eq!(amount(Item::HotExit, s.found(Item::HotExit).unwrap(), &o, now), (3, 1, 0));
    assert_eq!(amount(Item::MaintenanceBackups, s.found(Item::MaintenanceBackups).unwrap(), &o, now), (11, 1, 0));
    assert!(!s.found(Item::DbBackups).unwrap().files.iter().any(|f| f.path.ends_with("maintenance-20260101-120000.db")), "Wartungs-Sicherungen nicht unter den Datenbank-Sicherungen");
    // Doppelt gewählte Dateien zählen einmal
    let (bytes, files) = total(&s, &sel(&[Item::ScrollbackAll, Item::ScrollbackOrphaned]), &o, now);
    assert_eq!((bytes, files), (300, 2));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn age_filter_and_backup_keep() {
    let now = SystemTime::now();
    let old = now - Duration::from_secs(40 * 86_400);
    let f = |p: &str, m: SystemTime| FileEntry { path: PathBuf::from(p), bytes: 10, modified: Some(m) };
    let found = Found { files: vec![f("1.txt", now), f("2.txt", old)], ..Default::default() };
    let o = Opts { older_than_days: Some(30), ..Opts::default() };
    let names: Vec<&Path> = selected_files(Item::ScrollbackAll, &found, &o, now).iter().map(|e| e.path.as_path()).collect();
    assert_eq!(names, [Path::new("2.txt")]);
    assert_eq!(selected_files(Item::ScrollbackAll, &found, &Opts::default(), now).len(), 2);
    // Der Filter gilt nur für den Verlauf
    assert_eq!(selected_files(Item::Clipboard, &found, &o, now).len(), 2);
    let b = Found { files: (0..5).map(|i| f(&format!("b{i}"), now)).collect(), ..Default::default() };
    let keep = |n| selected_files(Item::DbBackups, &b, &Opts { keep_backups: n, ..Opts::default() }, now).iter().map(|e| e.path.to_string_lossy().to_string()).collect::<Vec<_>>();
    assert_eq!(keep(3), ["b3", "b4"], "neueste zuerst: die ersten N bleiben");
    assert_eq!(keep(0).len(), 5);
    assert!(keep(9).is_empty());
}

#[test]
fn live_run_deletes_only_the_selection() {
    let d = tmp("live");
    sample(&d);
    let c = ctx(&d);
    let s = scan(&c);
    let r = execute(&c, &s, &sel(&[Item::Clipboard, Item::OldBackups, Item::CrashLog, Item::ScrollbackOrphaned, Item::ChatsOrphaned]), &Opts::default(), Phase::Live);
    assert!(r.iter().all(|x| x.ok()), "{r:?}");
    assert!(!d.join("clipboard").join("clip-1-0.png").exists() && d.join("clipboard").is_dir(), "Ordner bleibt");
    assert!(!d.join("crash.log").exists() && !d.join("state.json.bak-20260101-000000").exists());
    assert!(!d.join("scrollback").join("7.txt").exists() && d.join("scrollback").join("1.txt").exists() && d.join("scrollback").join("notes.txt").exists());
    assert!(!d.join("chats").join("9.json").exists() && d.join("chats").join("2.json").exists());
    assert!(d.join("mcp-log.jsonl").exists() && d.join("state.json").exists() && d.join("prompts.json").exists());
    let (bytes, files) = report_total(&r);
    assert_eq!((bytes, files), (1500 + 30 + 30 + 200 + 407, 2 + 2 + 1 + 1 + 2));
    // Im laufenden Betrieb fehlen Datenbank/Sitzungen (Neustart) und Einstellungen (die Oberfläche)
    let r = execute(&c, &scan(&c), &sel(&[Item::Sessions, Item::SettingsKeys]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert!(r.is_empty(), "{r:?}");
    assert!(d.join("state.json").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn partial_failure_lists_the_file_and_the_rest_continues() {
    let d = tmp("partial");
    sample(&d);
    let c = ctx(&d);
    let mut s = scan(&c);
    // Eine „Datei“, die sich nicht löschen lässt (ein Ordner mit Inhalt), mitten in der Liste
    let blocker = d.join("clipboard").join("busy.png");
    put(&blocker.join("inner"), 1);
    if let Some((_, f)) = s.items.iter_mut().find(|(i, _)| *i == Item::Clipboard) {
        f.files.insert(1, FileEntry { path: blocker.clone(), bytes: 1, modified: None });
    }
    let r = execute(&c, &s, &sel(&[Item::Clipboard, Item::CrashLog]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    let clip = r.iter().find(|x| x.id == "clipboard_images").unwrap();
    assert_eq!((clip.files, clip.errors.len()), (2, 1), "{clip:?}");
    assert!(clip.errors[0].contains("busy.png"));
    assert!(r.iter().find(|x| x.id == "crash_log").unwrap().ok());
    assert!(!d.join("crash.log").exists() && blocker.exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn database_is_backed_up_first_and_kept_when_the_backup_fails() {
    let d = tmp("db");
    sample(&d);
    make_db(&d.join(crate::db::FILE));
    let c = ctx(&d);
    let r = execute(&c, &scan(&c), &sel(&[Item::Database, Item::CrashLog]), &Opts { backup_db: false, ..Opts::default() }, Phase::Start);
    let b = r.iter().find(|x| x.id == "db_backup").expect("Sicherung erzwungen, auch wenn abgeschaltet");
    assert!(b.ok(), "{b:?}");
    assert!(!d.join(crate::db::FILE).exists());
    let list = crate::backup::list(&d.join("backups"));
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, crate::backup::Kind::PreCleanup);

    // Sicherungsordner unbrauchbar (eine Datei): Datenbank bleibt, die übrigen Einträge laufen weiter
    let d2 = tmp("db-fail");
    sample(&d2);
    make_db(&d2.join(crate::db::FILE));
    put(&d2.join("not-a-dir"), 1);
    let c2 = Ctx { backup_dir: d2.join("not-a-dir"), ..ctx(&d2) };
    let r = execute(&c2, &scan(&c2), &sel(&[Item::Database, Item::CrashLog]), &Opts::default(), Phase::Start);
    assert!(!r.iter().find(|x| x.id == "db_backup").unwrap().ok());
    let db = r.iter().find(|x| x.id == "database").unwrap();
    assert!(!db.ok() && db.files == 0, "{db:?}");
    assert!(d2.join(crate::db::FILE).exists(), "ohne Sicherung kein Löschen");
    assert!(r.iter().find(|x| x.id == "crash_log").unwrap().ok() && !d2.join("crash.log").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn backup_switch_and_all_backups_keep_this_runs_copy() {
    let d = tmp("bk");
    make_db(&d.join(crate::db::FILE));
    let bk = d.join("backups");
    for n in ["auto-20250101-000000.db", "manual-20250102-000000.db", "auto-20250103-000000.db"] {
        put(&bk.join(n), 5);
    }
    let c = ctx(&d);
    // Abgeschaltet und ohne Datenbank-Eintrag: keine Sicherung
    let r = execute(&c, &scan(&c), &sel(&[Item::Clipboard]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert!(r.iter().all(|x| x.id != "db_backup"));
    // „Alle“ löscht alle alten, die Sicherung dieses Laufs bleibt
    let r = execute(&c, &scan(&c), &sel(&[Item::DbBackups]), &Opts { keep_backups: 0, ..Opts::default() }, Phase::Live);
    assert!(r.iter().find(|x| x.id == "db_backup").unwrap().ok());
    assert_eq!(r.iter().find(|x| x.id == "db_backups").unwrap().files, 3);
    let left = crate::backup::list(&bk);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].kind, crate::backup::Kind::PreCleanup);
    assert!(d.join(crate::db::FILE).exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pending_runs_at_start_without_integration() {
    let d = tmp("pending");
    sample(&d);
    make_db(&d.join(crate::db::FILE));
    put(&d.join("settings.json"), 0);
    std::fs::write(d.join("settings.json"), r#"{"keys": {"new": "Ctrl+Alt+N"}, "theme": "Dracula"}"#).unwrap();
    let done = ItemReport { id: "autostart".into(), entries: 1, ..Default::default() };
    let p = Pending {
        version: 1,
        items: ["sessions_layout", "database", "settings_shortcuts", "autostart", "unknown_thing"].iter().map(|s| s.to_string()).collect(),
        opts: Opts::default(),
        backup_dir: Some(d.join("bk")),
        done: vec![done.clone()],
    };
    schedule(&d, &p).unwrap();
    let r = apply_pending(&d, || d.join("bk")).expect("vorgemerkt");
    assert!(!d.join(PENDING_FILE).exists(), "Vormerkung verbraucht");
    assert!(apply_pending(&d, || d.join("never")).is_none());
    assert_eq!(r[0], done, "Ergebnisse des laufenden Teils stehen vorn");
    for refused in ["autostart", "unknown_thing"] {
        assert!(r.iter().any(|x| x.id == refused && !x.ok()), "{refused} nie aus der Vormerkung");
    }
    assert!(!d.join("state.json").exists() && !d.join("scrollback").join("1.txt").exists() && !d.join("chats").join("2.json").exists());
    assert!(!d.join(crate::db::FILE).exists() && crate::backup::list(&d.join("bk")).len() == 1);
    let s: crate::state::Settings = crate::state::load_json(&d.join("settings.json")).unwrap();
    assert!(s.keys.is_empty() && s.theme == "Dracula", "nur die Tastenkürzel zurückgesetzt");
    assert!(d.join("prompts.json").exists() && d.join("clipboard").join("clip-1-0.png").exists());
    // Unlesbare Vormerkung: Fehler, nichts gelöscht
    std::fs::write(d.join(PENDING_FILE), "{kaputt").unwrap();
    let r = apply_pending(&d, || d.join("never")).unwrap();
    assert!(r.len() == 1 && !r[0].ok() && d.join("prompts.json").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn factory_reset_keeps_lock_ipc_and_other_profiles() {
    let d = tmp("reset");
    sample(&d);
    make_db(&d.join(crate::db::FILE));
    put(&d.join("instance.lock"), 0);
    put(&d.join("ipc.json"), 3);
    put(&d.join("profiles").join("work").join("settings.json"), 2);
    put(&d.join("themes").join("mine.json"), 2);
    put(&d.join("backups").join("auto-20250101-000000.db"), 4);
    schedule(&d, &Pending { version: 1, items: vec!["factory_reset".into()], ..Default::default() }).unwrap();
    let r = apply_pending(&d, || d.join("backups")).unwrap();
    assert!(r.iter().all(|x| x.ok()), "{r:?}");
    let mut left: Vec<String> = walk(&d).iter().map(|f| f.path.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect();
    left.sort();
    assert_eq!(left, ["instance.lock", "ipc.json", "profiles/work/settings.json"]);
    assert!(!d.join("scrollback").exists() && !d.join("backups").exists(), "leere Ordner weg");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn settings_items_reset_only_their_part() {
    let mut s = crate::state::Settings { theme: "Dracula".into(), language: Lang::De, ..Default::default() };
    s.keys.insert("new".into(), "Ctrl+Alt+N".into());
    s.llm_servers.list.push(serde_json::from_value(serde_json::json!({"id": "a", "name": "Local", "kind": "ollama", "url": "http://127.0.0.1:11434", "api_key": "QUJD"})).unwrap());
    let mut k = s.clone();
    reset_setting(Item::SettingsKeys, &mut k, true);
    assert!(k.keys.is_empty() && k.theme == "Dracula" && k.llm_servers.len() == 1);
    let mut m = s.clone();
    reset_setting(Item::SettingsModels, &mut m, true);
    assert!(m.llm_servers.is_blank() && m.last_chat_model.is_none() && m.keys.len() == 1);
    let mut a = s.clone();
    reset_setting(Item::SettingsAll, &mut a, true);
    assert!(a.theme == "One Dark" && a.keys.is_empty() && a.language == Lang::De);
    let mut a = s.clone();
    reset_setting(Item::SettingsAll, &mut a, false);
    assert_eq!(a.language, Lang::En);
    // Kopie vor „alle Einstellungen“
    let d = tmp("sbak");
    assert_eq!(backup_settings(&d).unwrap(), None);
    std::fs::write(d.join("settings.json"), "{}").unwrap();
    let b = backup_settings(&d).unwrap().unwrap();
    assert!(b.file_name().unwrap().to_string_lossy().starts_with("settings.json.bak-") && d.join("settings.json").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn custom_themes_leave_the_example() {
    let d = tmp("themes");
    put(&d.join("themes").join(EXAMPLE_THEME), 5);
    put(&d.join("themes").join("mine.json"), 6);
    put(&d.join("themes").join("readme.txt"), 7);
    let c = ctx(&d);
    let r = execute(&c, &scan(&c), &sel(&[Item::Themes]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert_eq!(r[0].files, 1);
    assert!(d.join("themes").join(EXAMPLE_THEME).exists() && d.join("themes").join("readme.txt").exists() && !d.join("themes").join("mine.json").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn other_profiles_only_when_not_running() {
    let d = tmp("profiles");
    put(&d.join("profiles").join("work").join("settings.json"), 2);
    put(&d.join("profiles").join("work").join("scrollback").join("1.txt"), 3);
    put(&d.join("profiles").join("busy").join("settings.json"), 2);
    put(&d.join("profiles").join("nosettings").join("x.txt"), 2);
    // Die Instanz von „busy“ läuft (hält die Sperre)
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(d.join("profiles").join("busy").join("instance.lock")).unwrap();
    lock.try_lock().unwrap();
    let c = ctx(&d);
    let s = scan(&c);
    let names: Vec<(&str, bool)> = s.profiles.iter().map(|p| (p.name.as_str(), p.running)).collect();
    assert_eq!(names, [("busy", true), ("work", false)]);
    assert_eq!((s.profiles[1].bytes, s.profiles[1].files), (5, 2));
    let mut pick = Selection::default();
    pick.profiles.extend(["work".to_string(), "busy".to_string()]);
    let r = execute(&c, &s, &pick, &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert!(r.iter().find(|x| x.id == "profile:work").unwrap().ok() && !d.join("profiles").join("work").exists());
    assert!(!r.iter().find(|x| x.id == "profile:busy").unwrap().ok() && d.join("profiles").join("busy").join("settings.json").exists());
    // Das eigene Profil steht nie in der Liste
    let own = Ctx { data_dir: d.join("profiles").join("busy"), ..ctx(&d) };
    assert!(profiles(&own).is_empty());
    drop(lock);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn integration_entries_point_to_this_exe_only() {
    let exe = PathBuf::from(r"C:\pd\paneldeck.exe");
    let vals = vec![
        ("Paneldeck".to_string(), r#""C:\pd\paneldeck.exe" --tray"#.to_string()),
        ("Paneldeck (work)".to_string(), r#""c:\PD\paneldeck.exe" --profile Work --tray"#.to_string()),
        ("Paneldeck (old)".to_string(), r#""D:\old\paneldeck.exe" --profile old --tray"#.to_string()),
        ("OneDrive".to_string(), r#""C:\pd\paneldeck.exe""#.to_string()),
    ];
    let ours: Vec<String> = integ::our_run_values(&vals, &exe).into_iter().map(|(n, _)| n).collect();
    assert_eq!(ours, ["Paneldeck", "Paneldeck (work)"]);
    let prof: Vec<String> = integ::profile_run_values(&vals, &exe, "work").into_iter().map(|(n, _)| n).collect();
    assert_eq!(prof, ["Paneldeck (work)"]);
    assert!(integ::profile_run_values(&vals, &exe, "old").is_empty(), "andere .exe");
    let reg = integ::reg_export(&[
        (crate::integration::RUN_KEY.to_string(), Some("Paneldeck (work)".into()), r#""C:\pd\paneldeck.exe" --tray"#.into()),
        (r"Software\Classes\Directory\shell\Paneldeck".into(), None, "Open in Paneldeck".into()),
    ]);
    assert!(reg.starts_with("Windows Registry Editor Version 5.00\r\n"));
    assert!(reg.contains("[HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run]\r\n\"Paneldeck (work)\"=\"\\\"C:\\\\pd\\\\paneldeck.exe\\\" --tray\"\r\n"), "{reg}");
    assert!(reg.contains("@=\"Open in Paneldeck\""));
}

#[test]
fn claude_json_entries_removed_after_a_backup() {
    let d = tmp("cj");
    let exe = PathBuf::from(r"C:\pd\paneldeck.exe");
    let p = d.join(".claude.json");
    let text = "{\r\n  \"mcpServers\": {\r\n    \"paneldeck\": {\"command\": \"C:\\\\pd\\\\paneldeck.exe\", \"args\": [\"mcp\"]},\r\n    \"github\": {\"command\": \"npx\", \"args\": [\"x\"]}\r\n  },\r\n  \"other\": 1\r\n}\r\n";
    std::fs::write(&p, text).unwrap();
    assert_eq!(integ::servers_in(&p, &exe), ["paneldeck"]);
    // Über den Eintrag (wie im Programm), mit Testzustand statt System
    let c = Ctx { integ_override: Some(integ::State { claude_json: Some(p.clone()), claude_json_servers: vec!["paneldeck".into()], ..Default::default() }), exe: exe.clone(), ..ctx(&d) };
    let s = scan(&c);
    assert_eq!(s.found(Item::ClaudeJson).unwrap().entries, ["paneldeck"]);
    let r = execute(&c, &s, &sel(&[Item::ClaudeJson]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert!(r[0].ok() && r[0].entries == 1, "{r:?}");
    assert!(integ::servers_in(&p, &exe).is_empty());
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert!(v["mcpServers"]["github"].is_object() && v["other"] == 1);
    let baks: Vec<_> = walk(&d.join("backups")).into_iter().filter(|f| f.path.to_string_lossy().contains("claude.json")).collect();
    assert_eq!(baks.len(), 1);
    assert_eq!(std::fs::read_to_string(&baks[0].path).unwrap(), text, "Sicherung = Datei vorher");
    // Registry-Einträge ohne System: nichts
    let c = Ctx { integ_override: Some(integ::State { run_values: vec![("Paneldeck".into(), "x".into())], ..Default::default() }), ..ctx(&d) };
    let r = execute(&c, &scan(&c), &sel(&[Item::Autostart]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert_eq!(r[0].entries, 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn zip_backup_round_trip_without_credentials() {
    let d = tmp("zip");
    sample(&d);
    std::fs::write(d.join("settings.json"), r#"{"llm_servers": [{"id": "a", "name": "L", "kind": "openai", "url": "http://127.0.0.1:1", "api_key": "U0VDUkVU"}], "theme": "Dracula"}"#).unwrap();
    put(&d.join("instance.lock"), 0);
    put(&d.join("ipc.json"), 3);
    put(&d.join("profiles").join("w").join("settings.json"), 2);
    make_db(&d.join(crate::db::FILE));
    let copy = d.join("db-copy.db");
    std::fs::copy(d.join(crate::db::FILE), &copy).unwrap();
    let entries = zip::data_folder_entries(&d, Some(&copy)).unwrap();
    let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
    let has = |x: &str| names.iter().any(|n| n == x);
    assert!(has("scrollback/7.txt") && has("paneldeck.db") && has("settings.json"));
    assert!(!names.iter().any(|n| n == "instance.lock" || n == "ipc.json" || n.starts_with("profiles/")));
    let out = d.join("out").join("b.zip");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let (raw, n) = zip::write(&out, entries).unwrap();
    assert!(raw > 0 && n as usize == names.len());
    let bytes = std::fs::read(&out).unwrap();
    let s = crate::app::update::zip::extract(&bytes, "settings.json", 1 << 20).unwrap();
    let s = String::from_utf8(s).unwrap();
    assert!(!s.contains("api_key") && !s.contains("U0VDUkVU") && s.contains("Dracula"), "{s}");
    assert_eq!(crate::app::update::zip::extract(&bytes, "clipboard/clip-1-0.png", 1 << 20).unwrap(), vec![b'x'; 1000]);
    assert!(!out.with_extension("zip.part").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn formatting_in_both_languages() {
    assert_eq!(fmt_bytes(512), "512 B");
    assert_eq!(fmt_bytes(2048), "2 KB");
    assert_eq!(fmt_bytes(312 * 1024 * 1024), "312.0 MB");
    assert_eq!(fmt_amount(2048, 48, 0, Lang::En), "2 KB, 48 files");
    assert_eq!(fmt_amount(2048, 1, 0, Lang::De), "2 KB, 1 Datei");
    assert_eq!(fmt_amount(0, 0, 2, Lang::En), "2 entries");
    assert_eq!(fmt_amount(0, 0, 0, Lang::De), "0 B, 0 Dateien");
    assert_eq!(ItemReport::new("profile:work").label(Lang::De), "Profil „work“");
    assert_eq!(ItemReport::new("db_backup").label(Lang::En), "Database backup first");
}

#[test]
fn maintenance_backups_match_only_paneldecks_own_names() {
    for ok in ["maintenance-20260101-120000.db", "maintenance-20261231-235959-2.db"] {
        assert!(is_maintenance_backup_name(ok), "{ok}");
    }
    for bad in [
        "maintenance-20260101.zip",
        "maintenance-20260101-120000.zip",
        "Maintenance-20260101-120000.db",
        "my-maintenance-20260101-120000.db",
        "maintenance-20261301-120000.db",
        "maintenance-20260101-120000-.db",
        "maintenance-20260101-120000-2-3.db",
        "maintenance-2026011-120000.db",
        "notes-maintenance.db",
    ] {
        assert!(!is_maintenance_backup_name(bad), "{bad}");
    }
}

#[test]
fn pending_never_takes_the_backup_dir_from_the_file() {
    let d = tmp("pending-bk");
    make_db(&d.join(crate::db::FILE));
    // Ein fremder Ordner mit einer Sicherung, die beim Zurücksetzen gelöscht würde
    let foreign = tmp("pending-foreign");
    put(&foreign.join("auto-20250101-000000.db"), 4);
    let p = Pending { version: 1, items: vec!["database".into()], backup_dir: Some(foreign.clone()), ..Default::default() };
    schedule(&d, &p).unwrap();
    let here = d.join("backups");
    let r = apply_pending(&d, || here.clone()).unwrap();
    assert!(r.iter().all(|x| x.ok()), "{r:?}");
    assert!(r.iter().any(|x| x.id == "pending" && !x.notes.is_empty()), "Abweichung wird genannt");
    assert_eq!(crate::backup::list(&here).len(), 1, "Sicherung im Ordner dieses Starts");
    assert_eq!(crate::backup::list(&foreign).len(), 1, "fremder Ordner unberührt");
    // Zurücksetzen mit fremdem Ordner in der Datei: dessen Dateien bleiben
    schedule(&d, &Pending { version: 1, items: vec!["factory_reset".into()], backup_dir: Some(foreign.clone()), ..Default::default() }).unwrap();
    let r = apply_pending(&d, || d.join("backups")).unwrap();
    assert!(r.iter().filter(|x| x.id != "pending").all(|x| x.ok()), "{r:?}");
    assert!(foreign.join("auto-20250101-000000.db").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&foreign);
}

#[test]
fn zip_backup_strips_credentials_from_copies_and_the_process_list() {
    let d = tmp("zip-cred");
    let key = r#"{"llm_servers": [{"id": "a", "name": "L", "kind": "openai", "url": "http://127.0.0.1:1", "api_key": "U0VDUkVU"}], "theme": "Dracula"}"#;
    std::fs::write(d.join("settings.json"), key).unwrap();
    std::fs::write(d.join("settings.json.bak-20260101-000000"), key).unwrap();
    std::fs::write(d.join("settings.json.bak"), key).unwrap();
    // eine kaputte Kopie mit Schlüssel: bleibt draußen
    std::fs::write(d.join("settings.defekt-123.json"), r#"{"llm_servers": [{"api_key": "U0VDUkVU""#).unwrap();
    std::fs::write(d.join("processes.json"), r#"{"version":1,"processes":[{"id":1,"name":"x","command":"y","env":[{"name":"TOKEN","value_protected":"QkxPQg=="}]}]}"#).unwrap();
    std::fs::write(d.join("processes.defekt-5.json"), "kaputt QkxPQg==").unwrap();
    // eine andere JSON-Datei mit Server-Liste
    std::fs::write(d.join("other.json"), key).unwrap();
    std::fs::write(d.join("notes.txt"), "api_key bleibt als Text").unwrap();
    let entries = zip::data_folder_entries(&d, None).unwrap();
    let get = |n: &str| entries.iter().find(|(x, _)| x == n).map(|(_, b)| String::from_utf8(b.clone()).unwrap());
    for n in ["settings.json", "settings.json.bak-20260101-000000", "settings.json.bak", "other.json"] {
        let s = get(n).unwrap_or_else(|| panic!("{n} fehlt"));
        assert!(!s.contains("U0VDUkVU") && !s.contains("api_key") && s.contains("Dracula"), "{n}: {s}");
    }
    assert!(get("settings.defekt-123.json").is_none() && get("processes.defekt-5.json").is_none(), "unlesbare Kopien bleiben draußen");
    let p = get("processes.json").unwrap();
    assert!(!p.contains("QkxPQg==") && !p.contains("value_protected") && p.contains("TOKEN"), "{p}");
    assert_eq!(get("notes.txt").unwrap(), "api_key bleibt als Text");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn reset_writes_integration_copies_next_to_the_zip() {
    let d = tmp("safety");
    let out = tmp("safety-out");
    let exe = PathBuf::from(r"C:\pd\paneldeck.exe");
    let cj = out.join("src.claude.json");
    std::fs::write(&cj, r#"{"mcpServers": {"paneldeck": {"command": "C:\\pd\\paneldeck.exe", "args": ["mcp"]}}}"#).unwrap();
    let c = Ctx {
        integ_override: Some(integ::State { claude_json: Some(cj.clone()), claude_json_servers: vec!["paneldeck".into()], ..Default::default() }),
        exe,
        safety_dir: Some(out.clone()),
        ..ctx(&d)
    };
    let r = execute(&c, &scan(&c), &sel(&[Item::ClaudeJson]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert!(r[0].ok(), "{r:?}");
    let baks: Vec<_> = walk(&out).into_iter().filter(|f| f.path.file_name().unwrap().to_string_lossy().starts_with("claude.json-")).collect();
    assert_eq!(baks.len(), 1, "Kopie neben der ZIP-Datei");
    assert!(!d.join("backups").exists(), "nicht im Datenordner (der wird gelöscht)");
    let shown = baks[0].path.display().to_string();
    assert!(r[0].notes.iter().any(|(de, en)| de.contains(&shown) && en.contains(&shown)), "Bericht nennt den echten Ort");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&out);
}

/// Einen Ordner-Link anlegen (Windows: Junction, sonst Symlink). `false`, wenn die Plattform das hier nicht erlaubt.
fn make_dir_link(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && link.exists()
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

#[test]
fn linked_root_folders_are_not_followed() {
    let d = tmp("link");
    let outside = tmp("link-target");
    put(&outside.join("keep.png"), 10);
    put(&outside.join("sub").join("7.txt"), 10);
    put(&outside.join("1.txt"), 10);
    if !make_dir_link(&outside, &d.join("clipboard")) || !make_dir_link(&outside, &d.join("scrollback")) {
        eprintln!("Ordner-Links hier nicht möglich - Test übersprungen");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
        return;
    }
    assert!(root_is_link(&d.join("clipboard")));
    assert!(walk(&d.join("clipboard")).is_empty());
    let c = ctx(&d);
    let s = scan(&c);
    let clip = s.found(Item::Clipboard).unwrap();
    assert!(clip.files.is_empty() && clip.linked == [d.join("clipboard")]);
    assert!(s.found(Item::ScrollbackAll).unwrap().files.is_empty());
    let r = execute(&c, &s, &sel(&[Item::Clipboard, Item::ScrollbackAll]), &Opts { backup_db: false, ..Opts::default() }, Phase::Live);
    assert!(r.iter().all(|x| x.ok() && x.files == 0), "{r:?}");
    assert!(r.iter().all(|x| x.notes.iter().any(|(_, en)| en.contains("is a link"))), "als nicht berührt gemeldet: {r:?}");
    assert!(outside.join("keep.png").exists() && outside.join("1.txt").exists() && outside.join("sub").join("7.txt").exists(), "Ziel unberührt");
    // Zurücksetzen beim Start: das Ziel bleibt ebenfalls
    schedule(&d, &Pending { version: 1, items: vec!["factory_reset".into()], ..Default::default() }).unwrap();
    let _ = apply_pending(&d, || d.join("backups"));
    assert!(outside.join("keep.png").exists() && outside.join("sub").join("7.txt").exists(), "Ziel unberührt nach dem Zurücksetzen");
    #[cfg(windows)]
    {
        let _ = std::fs::remove_dir(d.join("clipboard"));
        let _ = std::fs::remove_dir(d.join("scrollback"));
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

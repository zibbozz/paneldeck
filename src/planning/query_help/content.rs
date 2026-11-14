//! Der Inhalt der Hilfe: Abschnitte, Texte (Englisch/Deutsch) und die Beispiele. Referenz und Fehlersuche stehen in `reference.rs`.

use super::dataset::shop;
use super::eval::REFERENCE_TODAY;
use super::{Block, Example, HelpDoc, Level, Section, Text};

pub(super) fn t(en: &'static str, de: &'static str) -> Text {
    Text::new(en, de)
}

pub(super) fn same(s: &'static str) -> Text {
    Text::same(s)
}

pub(super) fn p(en: &'static str, de: &'static str) -> Block {
    Block::Paragraph(t(en, de))
}

pub(super) fn note(en: &'static str, de: &'static str) -> Block {
    Block::Note(t(en, de))
}

pub(super) fn code(s: &str) -> Block {
    Block::Code(s.to_string())
}

pub(super) fn table(header: &[Text], rows: Vec<Vec<Text>>) -> Block {
    Block::Table { header: header.to_vec(), rows }
}

/// Ein Beispiel auf den gemeinsamen Beispieldaten „Web shop“.
fn ex(id: &'static str, level: Level, title: Text, jql: &str, explanation: Text, expect: &[&str]) -> Example {
    Example {
        id,
        title,
        jql: jql.to_string(),
        explanation,
        level,
        demo: shop(),
        expect: expect.iter().map(|s| s.to_string()).collect(),
        expect_none: false,
    }
}

fn section(id: &'static str, title: Text, blocks: Vec<Block>) -> Section {
    Section { id, title, blocks }
}

pub(super) fn build() -> HelpDoc {
    use Level::*;
    let e = Block::Example;

    let start = section(
        "start",
        t("Getting started", "Erste Schritte"),
        vec![
            p(
                "The query language finds tickets with a short line of text instead of clicking through filters. Type the query into the search field; the question mark next to it opens this guide. While you type, the field shows hints and error messages in your language.",
                "Mit der Abfragesprache findest du Tickets über eine kurze Textzeile, ohne dich durch Filter zu klicken. Tippe die Abfrage ins Suchfeld; das Fragezeichen daneben öffnet diese Anleitung. Beim Tippen zeigt das Feld Hinweise und Fehlermeldungen in deiner Sprache.",
            ),
            note(
                "Every example below shows the query, an explanation and the result for small example data (the demo project “Web shop”), as a screenshot made from the application itself. The examples use the English default names (Defect, To Do, …); the autocomplete shows which names your project knows.",
                "Jedes Beispiel unten zeigt die Abfrage, eine Erklärung und das Ergebnis für kleine Beispieldaten (das Demo-Projekt „Web shop“) als Screenshot aus der Anwendung selbst. Die Beispiele nutzen die englischen Standardnamen (Defect, To Do, …); welche Namen dein Projekt kennt, zeigt die Autovervollständigung.",
            ),
            p(
                "A query is made of **conditions**. A condition has three parts: a **field**, an **operator** and a **value**.",
                "Eine Abfrage besteht aus **Bedingungen**. Eine Bedingung hat drei Teile: ein **Feld**, einen **Operator** und einen **Wert**.",
            ),
            table(
                &[t("Part", "Teil"), t("Example", "Beispiel"), t("Meaning", "Bedeutung")],
                vec![
                    vec![t("Field", "Feld"), same("`status`"), t("What to look at: a field of the ticket.", "Worauf geschaut wird: ein Feld des Tickets.")],
                    vec![t("Operator", "Operator"), same("`!=`"), t("How to compare: equal, not equal, contains, smaller, …", "Wie verglichen wird: gleich, ungleich, enthält, kleiner, …")],
                    vec![t("Value", "Wert"), same("`Done`"), t("What to compare with: a word, a number, a date, a text in quotes or a function like `currentDate()`.", "Womit verglichen wird: ein Wort, eine Zahl, ein Datum, ein Text in Anführungszeichen oder eine Funktion wie `currentDate()`.")],
                ],
            ),
            e(ex(
                "basics-field-value",
                Beginner,
                t("Find all defects", "Alle Defects finden"),
                "type = Defect",
                t(
                    "The simplest query: a field (`type`), an operator (`=`) and a value (`Defect`). It finds every ticket whose type is Defect.",
                    "Die einfachste Abfrage: ein Feld (`type`), ein Operator (`=`) und ein Wert (`Defect`). Sie findet jedes Ticket vom Typ Defect.",
                ),
                &["WEB-14", "WEB-15", "WEB-16", "WEB-17", "WEB-18"],
            )),
            p(
                "Several conditions are joined with `AND`, `OR` and `NOT`, grouped with parentheses, and the result is sorted with `ORDER BY` at the very end. An empty query shows all tickets.",
                "Mehrere Bedingungen verbindest du mit `AND`, `OR` und `NOT`, gruppierst sie mit Klammern und sortierst das Ergebnis ganz am Ende mit `ORDER BY`. Eine leere Abfrage zeigt alle Tickets.",
            ),
            code("condition AND condition OR condition ...\nORDER BY field [ASC|DESC], field [ASC|DESC]"),
            p(
                "**Quotes.** A field name or a value that contains a space or a special character goes in quotes: `\"Release version\"`, `\"In Progress\"`. Single quotes work as well. Words made of letters, digits, `_`, `-`, `.` and `:` need none (`Done`, `2.0.0`, `WEB-12`).",
                "**Anführungszeichen.** Ein Feldname oder Wert mit Leerzeichen oder Sonderzeichen steht in Anführungszeichen: `\"Release version\"`, `\"In Progress\"`. Einfache Anführungszeichen gehen auch. Wörter aus Buchstaben, Ziffern, `_`, `-`, `.` und `:` brauchen keine (`Done`, `2.0.0`, `WEB-12`).",
            ),
            p(
                "**Upper and lower case do not matter** – for keywords (`and`, `AND`), field names and values: `Type = defect` finds the same tickets as `type = Defect`.",
                "**Groß- und Kleinschreibung ist egal** – bei Schlüsselwörtern (`and`, `AND`), Feldnamen und Werten: `Type = defect` findet dieselben Tickets wie `type = Defect`.",
            ),
            e(ex(
                "basics-quotes",
                Beginner,
                t("A field name with a space", "Ein Feldname mit Leerzeichen"),
                r#""Release version" = 2.0.0"#,
                t(
                    "The field name has a space, so it is quoted. The value `2.0.0` needs no quotes: dots and hyphens are allowed in words.",
                    "Der Feldname enthält ein Leerzeichen und steht deshalb in Anführungszeichen. Der Wert `2.0.0` braucht keine: Punkte und Bindestriche sind in Wörtern erlaubt.",
                ),
                &["WEB-1", "WEB-2", "WEB-3"],
            )),
        ],
    );

    let data = section(
        "data",
        t("The example data", "Die Beispieldaten"),
        vec![
            p(
                "All examples run on the same small project “Web shop” (key `WEB`) with 22 tickets: five epics, user stories, dev tasks, subtasks and defects. These are its fields:",
                "Alle Beispiele laufen auf demselben kleinen Projekt „Web shop“ (Kürzel `WEB`) mit 22 Tickets: fünf Epics sowie User Stories, Dev Tasks, Subtasks und Defects. Das sind seine Felder:",
            ),
            table(
                &[t("Field", "Feld"), t("Kind", "Art"), t("Values", "Werte")],
                vec![
                    vec![same("`type`"), t("Dropdown", "Auswahl"), same("Epic, User story, Dev task, Subtask, Defect")],
                    vec![same("`status`"), t("Dropdown", "Auswahl"), t("To Do, In Progress, Development finished (a status of this project), Done", "To Do, In Progress, Development finished (ein Status dieses Projekts), Done")],
                    vec![same("`priority`"), t("Dropdown (most important first)", "Auswahl (wichtigste zuerst)"), same("Highest, High, Medium, Low, Lowest")],
                    vec![same("`summary`"), t("Text (the title)", "Text (der Titel)"), t("Free text", "Freier Text")],
                    vec![same("`parent`"), t("Ticket key", "Ticket-Schlüssel"), t("The epic or story above, e.g. WEB-2", "Das übergeordnete Ticket, z. B. WEB-2")],
                    vec![same("`sprint`"), t("Dropdown", "Auswahl"), t("Sprint 14 (closed), Sprint 15 (active), Sprint 16 (planned)", "Sprint 14 (abgeschlossen), Sprint 15 (aktiv), Sprint 16 (geplant)")],
                    vec![same("`\"Release version\"`"), t("Version (epics)", "Version (Epics)"), same("1.0.0, 1.1.0, 2.0.0, 2.1.0")],
                    vec![same("`\"Story points\"`"), t("Number", "Zahl"), t("On stories, tasks, subtasks and defects", "Bei Stories, Tasks, Subtasks und Defects")],
                    vec![same("`due`"), t("Date", "Datum"), t("Due date; the example data are relative to today", "Fälligkeit; die Beispieldaten sind relativ zu heute")],
                    vec![same("`labels`"), t("Labels (several values)", "Labels (mehrere Werte)"), same("frontend, backend, api, auth, payments, legacy")],
                    vec![same("`terminal`"), t("Terminal", "Terminal"), same("api-work, login-fix, docs")],
                    vec![same("`changedBy`"), t("Dropdown", "Auswahl"), t("Who made the last change: `ui`, `agent` or `import`", "Wer zuletzt geändert hat: `ui`, `agent` oder `import`")],
                ],
            ),
            p(
                "Your own fields work the same way: use their name as the field. The terminal in which the search is started is `login-fix`, and “today” is the day you run the query.",
                "Deine eigenen Felder funktionieren genauso: Nimm ihren Namen als Feld. Das Terminal, aus dem die Suche gestartet wird, ist `login-fix`, und „heute“ ist der Tag, an dem du die Abfrage ausführst.",
            ),
        ],
    );

    let filters = section(
        "filters",
        t("Simple filters", "Einfache Filter"),
        vec![
            p(
                "`=` finds tickets with exactly this value, `!=` the ones with a different value. `IN` takes a list of values in parentheses and finds tickets that match any of them; `NOT IN` finds tickets that match none. Numbers and dates can also be compared with `<`, `>`, `<=` and `>=`.",
                "`=` findet Tickets mit genau diesem Wert, `!=` die mit einem anderen. `IN` nimmt eine Liste von Werten in Klammern und findet Tickets, die einen davon haben; `NOT IN` findet Tickets, die keinen davon haben. Zahlen und Daten lassen sich auch mit `<`, `>`, `<=` und `>=` vergleichen.",
            ),
            e(ex(
                "filters-not-equal",
                Beginner,
                t("Open epics", "Offene Epics"),
                "type = Epic AND status != Done",
                t(
                    "`!=` finds tickets whose value is different. The two conditions are joined with `AND`: both must be true.",
                    "`!=` findet Tickets mit einem anderen Wert. Die beiden Bedingungen sind mit `AND` verbunden: Beide müssen zutreffen.",
                ),
                &["WEB-1", "WEB-3", "WEB-4"],
            )),
            e(ex(
                "filters-in",
                Beginner,
                t("Important defects", "Wichtige Defects"),
                "type = Defect AND priority IN (High, Highest)",
                t(
                    "`IN` takes a list in parentheses, separated by commas. It is shorter than `priority = High OR priority = Highest`.",
                    "`IN` nimmt eine Liste in Klammern, durch Kommas getrennt. Das ist kürzer als `priority = High OR priority = Highest`.",
                ),
                &["WEB-14", "WEB-15", "WEB-18"],
            )),
            e(ex(
                "filters-not-in",
                Beginner,
                t("Work in progress, without epics and subtasks", "Laufende Arbeit, ohne Epics und Subtasks"),
                r#"type NOT IN (Epic, Subtask) AND status = "In Progress""#,
                t(
                    "`NOT IN` leaves out every value of the list. The status `In Progress` contains a space, so the value is put in quotes.",
                    "`NOT IN` schließt jeden Wert der Liste aus. Der Status `In Progress` enthält ein Leerzeichen, deshalb steht der Wert in Anführungszeichen.",
                ),
                &["WEB-8", "WEB-11", "WEB-14"],
            )),
            e(ex(
                "filters-range",
                Intermediate,
                t("Tickets of medium size", "Tickets mittlerer Größe"),
                r#""Story points" > 3 AND "Story points" <= 5"#,
                t(
                    "Numbers can be compared with `<`, `>`, `<=` and `>=`. Two conditions on the same field make a range: more than 3 and at most 5 story points.",
                    "Zahlen lassen sich mit `<`, `>`, `<=` und `>=` vergleichen. Zwei Bedingungen auf dasselbe Feld ergeben einen Bereich: mehr als 3 und höchstens 5 Story Points.",
                ),
                &["WEB-6", "WEB-11", "WEB-21", "WEB-22"],
            )),
        ],
    );

    let text = section(
        "text",
        t("Searching text and labels", "Text und Labels durchsuchen"),
        vec![
            p(
                "`~` means “contains” and `!~` “does not contain”. Both ignore upper and lower case and work on text fields and labels. A plain `=` on a text field only matches if the whole text is equal.",
                "`~` heißt „enthält“ und `!~` „enthält nicht“. Beide ignorieren Groß- und Kleinschreibung und funktionieren bei Textfeldern und Labels. Ein einfaches `=` bei einem Textfeld trifft nur, wenn der ganze Text gleich ist.",
            ),
            e(ex(
                "text-contains",
                Beginner,
                t("Titles that mention “login”", "Titel, die „login“ enthalten"),
                r#"summary ~ "login""#,
                t(
                    "`summary` is the title of a ticket. `~` finds every title that contains the text, anywhere inside.",
                    "`summary` ist der Titel eines Tickets. `~` findet jeden Titel, der den Text irgendwo enthält.",
                ),
                &["WEB-12", "WEB-14", "WEB-17"],
            )),
            e(ex(
                "text-not-contains",
                Intermediate,
                t("Defects that are not about login", "Defects, bei denen es nicht um Login geht"),
                r#"type = Defect AND summary !~ "login""#,
                t(
                    "`!~` is the opposite of `~`. Combined with `AND` it removes the login defects from the list of all defects.",
                    "`!~` ist das Gegenteil von `~`. Mit `AND` verbunden nimmt es die Login-Defects aus der Liste aller Defects heraus.",
                ),
                &["WEB-15", "WEB-16", "WEB-18"],
            )),
            e(ex(
                "text-free",
                Beginner,
                t("Free text search", "Freitext-Suche"),
                r#""coupon""#,
                t(
                    "A text in quotes without a field and operator searches the title and all text fields, such as the description. WEB-15 is found through its description, WEB-18 through its title.",
                    "Ein Text in Anführungszeichen ohne Feld und Operator durchsucht den Titel und alle Textfelder, etwa die Beschreibung. WEB-15 wird über die Beschreibung gefunden, WEB-18 über den Titel.",
                ),
                &["WEB-15", "WEB-18"],
            )),
            e(ex(
                "text-labels",
                Intermediate,
                t("Open tickets with a label", "Offene Tickets mit einem Label"),
                "labels = auth AND status != Done",
                t(
                    "A ticket can have several labels. For such fields `=` means “has this value among its values”, so `labels = auth` finds every ticket that carries the label `auth`, whatever else it has. `labels != auth` finds the tickets without it.",
                    "Ein Ticket kann mehrere Labels haben. Bei solchen Feldern heißt `=` „hat diesen Wert unter seinen Werten“: `labels = auth` findet jedes Ticket mit dem Label `auth`, egal was es sonst noch hat. `labels != auth` findet die Tickets ohne dieses Label.",
                ),
                &["WEB-12", "WEB-14", "WEB-17", "WEB-19"],
            )),
        ],
    );

    let empty = section(
        "empty",
        t("Empty fields", "Leere Felder"),
        vec![
            p(
                "`IS EMPTY` finds tickets where a field has no value, `IS NOT EMPTY` the ones where it has. `IS NULL` and `IS NOT NULL` mean the same. A field that does not exist on a ticket's type counts as empty.",
                "`IS EMPTY` findet Tickets, bei denen ein Feld keinen Wert hat, `IS NOT EMPTY` die, bei denen es einen hat. `IS NULL` und `IS NOT NULL` bedeuten dasselbe. Ein Feld, das es beim Typ des Tickets nicht gibt, gilt als leer.",
            ),
            note(
                "`!=`, `!~` and `NOT IN` also match tickets whose field is empty: `status != Done` includes tickets without any status. Add `AND status IS NOT EMPTY` if you do not want them. `=`, `~`, `<`, `>`, `<=`, `>=` and `IN` never match an empty field.",
                "`!=`, `!~` und `NOT IN` treffen auch Tickets, bei denen das Feld leer ist: `status != Done` enthält auch Tickets ganz ohne Status. Ergänze `AND status IS NOT EMPTY`, wenn du sie nicht willst. `=`, `~`, `<`, `>`, `<=`, `>=` und `IN` treffen ein leeres Feld nie.",
            ),
            e(ex(
                "empty-is-empty",
                Beginner,
                t("Epics without a release", "Epics ohne Release"),
                r#"type = Epic AND "Release version" IS EMPTY"#,
                t(
                    "`IS EMPTY` needs no value. It finds the epics for which no release version has been chosen yet.",
                    "`IS EMPTY` braucht keinen Wert. Es findet die Epics, für die noch keine Release-Version gewählt wurde.",
                ),
                &["WEB-4", "WEB-5"],
            )),
            e(ex(
                "empty-parent",
                Intermediate,
                t("Stories and tasks without a parent", "Stories und Tasks ohne übergeordnetes Ticket"),
                r#"type IN ("User story", "Dev task") AND parent IS EMPTY"#,
                t(
                    "Combines `IN` with `IS EMPTY`. `User story` and `Dev task` contain a space, so both values are quoted.",
                    "Verbindet `IN` mit `IS EMPTY`. `User story` und `Dev task` enthalten ein Leerzeichen, deshalb stehen beide Werte in Anführungszeichen.",
                ),
                &["WEB-7", "WEB-13", "WEB-21"],
            )),
        ],
    );

    let mut dates_range = ex(
        "dates-range",
        Intermediate,
        t("Due next week", "Fällig nächste Woche"),
        "due >= 2026-11-17 AND due <= 2026-11-23",
        t(
            "A date is written like 2026-11-17 (with a time, for date-and-time fields: `2026-11-17T09:30`). Both ends of the range are included here because of `>=` and `<=`. This example uses fixed dates, so its example data do too.",
            "Ein Datum schreibst du wie 2026-11-17 (mit Uhrzeit, bei Feldern mit Datum und Uhrzeit: `2026-11-17T09:30`). Wegen `>=` und `<=` gehören beide Enden zum Bereich. Dieses Beispiel nutzt feste Daten, deshalb auch seine Beispieldaten.",
        ),
        &["WEB-8", "WEB-9", "WEB-10", "WEB-12", "WEB-15", "WEB-19"],
    );
    // Feste Daten: „heute“ der Prüfung ist der Bezugstag, damit die Abfrage mit den festen Daten an jedem Tag dasselbe trifft.
    dates_range.demo = shop().resolved(REFERENCE_TODAY);

    let dates = section(
        "dates",
        t("Dates", "Datum"),
        vec![
            p(
                "Dates are written `YYYY-MM-DD`, for example `2026-11-14`; with a time `2026-11-14T10:30`. A date without a time counts as 00:00. `currentDate()` is today. Compare dates with `=`, `<`, `>`, `<=`, `>=`; a ticket with an empty date never matches a comparison.",
                "Daten schreibst du `JJJJ-MM-TT`, zum Beispiel `2026-11-14`; mit Uhrzeit `2026-11-14T10:30`. Ein Datum ohne Uhrzeit zählt als 00:00. `currentDate()` ist heute. Vergleiche Daten mit `=`, `<`, `>`, `<=`, `>=`; ein Ticket mit leerem Datum trifft nie einen Vergleich.",
            ),
            e(ex(
                "dates-overdue",
                Intermediate,
                t("Overdue tickets", "Überfällige Tickets"),
                "due < currentDate() AND status != Done",
                t(
                    "`currentDate()` is a function that returns today's date. Everything due before today and not done yet is overdue. Tickets without a due date do not appear.",
                    "`currentDate()` ist eine Funktion, die das heutige Datum liefert. Alles, was vor heute fällig war und noch nicht erledigt ist, ist überfällig. Tickets ohne Fälligkeit erscheinen nicht.",
                ),
                &["WEB-11", "WEB-14", "WEB-20"],
            )),
            e(dates_range),
        ],
    );

    let sprints = section(
        "sprints",
        t("Sprints", "Sprints"),
        vec![
            p(
                "`sprint = activeSprint()` finds the tickets of the project's active sprint – without typing its name, so the same query works in every sprint. Projects that do not use sprints have no `sprint` field.",
                "`sprint = activeSprint()` findet die Tickets des aktiven Sprints des Projekts – ohne dessen Namen zu tippen, die Abfrage funktioniert also in jedem Sprint. Projekte ohne Sprints haben kein Feld `sprint`.",
            ),
            e(ex(
                "sprint-active",
                Beginner,
                t("Everything in the current sprint", "Alles im aktuellen Sprint"),
                "sprint = activeSprint()",
                t(
                    "`activeSprint()` is replaced by the name of the active sprint (here “Sprint 15”).",
                    "`activeSprint()` wird durch den Namen des aktiven Sprints ersetzt (hier „Sprint 15“).",
                ),
                &["WEB-8", "WEB-9", "WEB-10", "WEB-11", "WEB-12", "WEB-14", "WEB-15", "WEB-19", "WEB-20"],
            )),
            e(ex(
                "sprint-backlog",
                Intermediate,
                t("The backlog: work in no sprint", "Das Backlog: Arbeit ohne Sprint"),
                "sprint IS EMPTY AND type != Epic AND status != Done",
                t(
                    "A ticket that is in no sprint has an empty `sprint` field. Epics are left out because they are never planned into a sprint directly.",
                    "Ein Ticket, das in keinem Sprint liegt, hat ein leeres Feld `sprint`. Epics sind ausgenommen, weil sie nie direkt in einen Sprint geplant werden.",
                ),
                &["WEB-16", "WEB-17", "WEB-21", "WEB-22"],
            )),
        ],
    );

    let terminals = section(
        "terminals",
        t("Terminals", "Terminals"),
        vec![
            p(
                "Tickets can be linked to terminals. `terminal IS NOT EMPTY` finds the linked ones. `linkedTerminal()` is the terminal the search was started from, so `terminal = linkedTerminal()` finds “the tickets of the terminal I am in”.",
                "Tickets lassen sich mit Terminals verknüpfen. `terminal IS NOT EMPTY` findet die verknüpften. `linkedTerminal()` ist das Terminal, aus dem die Suche gestartet wurde; `terminal = linkedTerminal()` findet also „die Tickets des Terminals, in dem ich gerade bin“.",
            ),
            e(ex(
                "terminal-any",
                Beginner,
                t("Tickets linked to any terminal", "Tickets mit einem verknüpften Terminal"),
                "terminal IS NOT EMPTY",
                t(
                    "`IS NOT EMPTY` finds tickets where the field has a value – here the ones that are linked to a terminal.",
                    "`IS NOT EMPTY` findet Tickets, bei denen das Feld einen Wert hat – hier die mit einem verknüpften Terminal.",
                ),
                &["WEB-11", "WEB-12", "WEB-14"],
            )),
            e(ex(
                "terminal-linked",
                Intermediate,
                t("The tickets of this terminal", "Die Tickets dieses Terminals"),
                "terminal = linkedTerminal()",
                t(
                    "The search was started from the terminal `login-fix`, so `linkedTerminal()` stands for it. Tickets of other terminals do not appear.",
                    "Die Suche wurde aus dem Terminal `login-fix` gestartet, `linkedTerminal()` steht also dafür. Tickets anderer Terminals erscheinen nicht.",
                ),
                &["WEB-12", "WEB-14"],
            )),
        ],
    );

    let flags = section(
        "flags",
        t("Favorites, flags and who changed it", "Favoriten, Markierungen und wer geändert hat"),
        vec![
            p(
                "Some conditions have no field and no value. `IS favorite` finds starred tickets, `IS blocked` the ones marked as blocked, `IS NOT blocked` all others. These are **predicates**; you can define your own in the keyword editor (the built-in ones cannot be deleted). The field `changedBy` tells who made the last change: `ui` (you in the window), `agent` or `import`.",
                "Manche Bedingungen haben weder Feld noch Wert. `IS favorite` findet Tickets mit Stern, `IS blocked` die als blockiert markierten, `IS NOT blocked` alle anderen. Das sind **Prädikate**; eigene definierst du im Schlüsselwort-Editor (die eingebauten lassen sich nicht löschen). Das Feld `changedBy` sagt, wer zuletzt geändert hat: `ui` (du im Fenster), `agent` oder `import`.",
            ),
            e(ex(
                "flags-favorite",
                Beginner,
                t("My starred tickets", "Meine Tickets mit Stern"),
                "IS favorite",
                t(
                    "`IS favorite` is a complete condition on its own: no field, no operator, no value.",
                    "`IS favorite` ist für sich schon eine vollständige Bedingung: kein Feld, kein Operator, kein Wert.",
                ),
                &["WEB-1", "WEB-8", "WEB-11", "WEB-14"],
            )),
            e(ex(
                "flags-not-blocked",
                Intermediate,
                t("Open defects that are not blocked", "Offene Defects, die nicht blockiert sind"),
                "type = Defect AND status != Done AND IS NOT blocked",
                t(
                    "`IS NOT blocked` turns the predicate around. WEB-15 is an open defect but marked as blocked, so it is left out.",
                    "`IS NOT blocked` dreht das Prädikat um. WEB-15 ist ein offenes Defect, aber als blockiert markiert und fällt deshalb heraus.",
                ),
                &["WEB-14", "WEB-16", "WEB-17"],
            )),
            e(ex(
                "flags-changed-by",
                Intermediate,
                t("What did an agent change?", "Was hat ein Agent geändert?"),
                "changedBy = agent AND status != Done ORDER BY priority",
                t(
                    "`changedBy` is a field like any other. With `agent` it finds tickets an agent worked on last – handy to review what happened while you were away.",
                    "`changedBy` ist ein Feld wie jedes andere. Mit `agent` findet es Tickets, an denen zuletzt ein Agent gearbeitet hat – praktisch, um zu prüfen, was in deiner Abwesenheit passiert ist.",
                ),
                &["WEB-8", "WEB-14", "WEB-11"],
            )),
        ],
    );

    let logic = section(
        "logic",
        t("Combining conditions", "Bedingungen verbinden"),
        vec![
            p(
                "`AND` needs both conditions, `OR` at least one, `NOT` turns a condition around. **`NOT` binds strongest, then `AND`, then `OR`.** Use parentheses to say what belongs together – when in doubt, use them.",
                "`AND` verlangt beide Bedingungen, `OR` mindestens eine, `NOT` dreht eine Bedingung um. **`NOT` bindet am stärksten, dann `AND`, dann `OR`.** Mit Klammern legst du fest, was zusammengehört – im Zweifel setze sie.",
            ),
            e(ex(
                "logic-precedence",
                Intermediate,
                t("AND binds stronger than OR", "AND bindet stärker als OR"),
                "type = Defect AND priority = High OR priority = Highest",
                t(
                    "This is read as `(type = Defect AND priority = High) OR priority = Highest` – so it also finds the epics and stories with the highest priority. That is rarely what you want; the next example fixes it with parentheses.",
                    "Das wird als `(type = Defect AND priority = High) OR priority = Highest` gelesen – es findet also auch die Epics und Stories mit höchster Priorität. Das willst du selten; das nächste Beispiel behebt es mit Klammern.",
                ),
                &["WEB-2", "WEB-3", "WEB-8", "WEB-14", "WEB-15", "WEB-18"],
            )),
            e(ex(
                "logic-parentheses",
                Intermediate,
                t("Parentheses group conditions", "Klammern gruppieren Bedingungen"),
                "type = Defect AND (priority = High OR priority = Highest)",
                t(
                    "The parentheses make `OR` apply to the two priorities only. Now every hit is a defect.",
                    "Durch die Klammern gilt `OR` nur für die beiden Prioritäten. Jetzt ist jeder Treffer ein Defect.",
                ),
                &["WEB-14", "WEB-15", "WEB-18"],
            )),
            e(ex(
                "logic-not",
                Intermediate,
                t("Stories that are neither done nor finished", "Stories, die weder erledigt noch fertig entwickelt sind"),
                r#"type = "User story" AND NOT (status = Done OR status = "Development finished")"#,
                t(
                    "`NOT` in front of a parenthesis turns the whole group around. It gives the same result as `status NOT IN (Done, \"Development finished\")`.",
                    "`NOT` vor einer Klammer dreht die ganze Gruppe um. Das Ergebnis ist dasselbe wie bei `status NOT IN (Done, \"Development finished\")`.",
                ),
                &["WEB-8", "WEB-10", "WEB-22"],
            )),
        ],
    );

    let sorting = section(
        "sorting",
        t("Sorting", "Sortieren"),
        vec![
            p(
                "`ORDER BY` (or `SORT BY`) comes at the very end. Name one or more fields, each with `ASC` (ascending, the default) or `DESC`. Dropdown fields are sorted in the order of their values – for the priority that means the most important first. Empty values always come last. Tickets that are equal keep the order of their keys. `SORT BY favorites` puts starred tickets first.",
                "`ORDER BY` (oder `SORT BY`) steht ganz am Ende. Nenne ein oder mehrere Felder, jeweils mit `ASC` (aufsteigend, Standard) oder `DESC`. Auswahlfelder werden in der Reihenfolge ihrer Werte sortiert – bei der Priorität steht also das Wichtigste zuerst. Leere Werte kommen immer zuletzt. Tickets, die gleich sind, behalten die Reihenfolge ihrer Schlüssel. `SORT BY favorites` stellt Tickets mit Stern nach vorn.",
            ),
            e(ex(
                "sort-priority",
                Beginner,
                t("Defects, most important first", "Defects, das Wichtigste zuerst"),
                "type = Defect ORDER BY priority",
                t(
                    "Without a direction the order is ascending, which for the priority is Highest, High, Medium, Low, Lowest. WEB-15 and WEB-18 are equal (High) and keep their key order.",
                    "Ohne Richtung wird aufsteigend sortiert, bei der Priorität also Highest, High, Medium, Low, Lowest. WEB-15 und WEB-18 sind gleich (High) und behalten ihre Schlüssel-Reihenfolge.",
                ),
                &["WEB-14", "WEB-15", "WEB-18", "WEB-17", "WEB-16"],
            )),
            e(ex(
                "sort-multi",
                Intermediate,
                t("Sort by two fields", "Nach zwei Feldern sortieren"),
                "sprint = activeSprint() AND status != Done ORDER BY priority ASC, due DESC",
                t(
                    "Several sort fields are separated by commas; the second one decides between tickets that are equal in the first. Here: by priority, and within one priority the latest due date first.",
                    "Mehrere Sortierfelder werden durch Kommas getrennt; das zweite entscheidet zwischen Tickets, die im ersten gleich sind. Hier: nach Priorität, und innerhalb einer Priorität zuerst das späteste Fälligkeitsdatum.",
                ),
                &["WEB-8", "WEB-14", "WEB-9", "WEB-15", "WEB-11", "WEB-10", "WEB-12", "WEB-19", "WEB-20"],
            )),
        ],
    );

    let advanced = section(
        "advanced",
        t("Putting it together", "Alles zusammen"),
        vec![
            p(
                "Real questions need several of these building blocks at once. The first example is a typical release check; the status “Development finished” is a custom status of this project.",
                "Echte Fragen brauchen mehrere dieser Bausteine zugleich. Das erste Beispiel ist eine typische Release-Prüfung; der Status „Development finished“ ist ein eigener Status dieses Projekts.",
            ),
            e(ex(
                "epics-release",
                Advanced,
                t("Finished epics of release 2.0.0", "Fertige Epics des Releases 2.0.0"),
                r#"type = Epic AND "Release version" = 2.0.0 AND status IN (Done, "Development finished") ORDER BY priority"#,
                t(
                    "Epics of release 2.0.0 that are either `Done` or in the custom status “Development finished” (quoted because of the space), the most important first. WEB-3 belongs to the release but is still in progress, so it is not listed.",
                    "Epics des Releases 2.0.0, die entweder `Done` sind oder im eigenen Status „Development finished“ (wegen des Leerzeichens in Anführungszeichen), das Wichtigste zuerst. WEB-3 gehört zum Release, ist aber noch in Arbeit und steht deshalb nicht in der Liste.",
                ),
                &["WEB-2", "WEB-1"],
            )),
            e(ex(
                "combo-sprint",
                Advanced,
                t("Important, unblocked work of the sprint", "Wichtige, nicht blockierte Arbeit des Sprints"),
                r#"(type = Defect OR type = "User story") AND sprint = activeSprint() AND IS NOT blocked AND priority IN (High, Highest) ORDER BY priority, due"#,
                t(
                    "Parentheses for the two types, then the active sprint, a predicate, a list and a two-field sort. The two blocked tickets of the sprint (WEB-10 and WEB-15) are left out.",
                    "Klammern für die beiden Typen, dann der aktive Sprint, ein Prädikat, eine Liste und eine Sortierung nach zwei Feldern. Die zwei blockierten Tickets des Sprints (WEB-10 und WEB-15) fallen heraus.",
                ),
                &["WEB-14", "WEB-8", "WEB-9"],
            )),
            e(ex(
                "combo-attention",
                Advanced,
                t("What needs attention first?", "Was braucht zuerst Aufmerksamkeit?"),
                "status != Done AND (due < currentDate() OR priority = Highest) AND IS NOT blocked SORT BY favorites, due",
                t(
                    "Open tickets that are overdue or have the highest priority, minus the blocked ones. Starred tickets come first (`SORT BY favorites`), then everything by due date; the epic WEB-3 has no due date and therefore comes last.",
                    "Offene Tickets, die überfällig sind oder die höchste Priorität haben, ohne die blockierten. Tickets mit Stern stehen vorn (`SORT BY favorites`), dann alles nach Fälligkeit; das Epic WEB-3 hat keine Fälligkeit und steht deshalb ganz hinten.",
                ),
                &["WEB-11", "WEB-14", "WEB-8", "WEB-20", "WEB-3"],
            )),
        ],
    );

    HelpDoc {
        sections: vec![
            start,
            data,
            filters,
            text,
            empty,
            dates,
            sprints,
            terminals,
            flags,
            logic,
            sorting,
            advanced,
            super::reference::troubleshooting(),
            super::reference::reference(),
        ],
    }
}

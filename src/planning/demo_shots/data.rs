//! Inhalt der Demo-Datenbank für die README-Screenshots: ein erfundener Online-Shop („Acme Shop“, Schlüssel `SHOP`) mit
//! Epics, Stories, Aufgaben, Fehlern und Unteraufgaben, drei Sprints (zwei abgeschlossen, einer aktiv, einer geplant), Backlog.
//!
//! **Zeit:** Alle Zeitangaben sind **Arbeitstage** `W` relativ zum Start von „Sprint 10“ (`W0`); jeder Sprint dauert zehn
//! Arbeitstage: Sprint 10 = W0–W9, Sprint 11 = W10–W19, Sprint 12 (aktiv) = W20–W29, Sprint 13 (geplant) = W30–W39. „Heute“ liegt
//! im aktiven Sprint (um W25–W28); Ereignisse nach „heute“ werden auf heute gekürzt (siehe `Builder::at`).
//! Texte in Deutsch und Englisch; `{#id}` wird beim Schreiben durch den echten Ticket-Schlüssel ersetzt.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ty {
    Epic,
    Story,
    Task,
    Sub,
    Defect,
}

impl Ty {
    pub fn key(self) -> &'static str {
        match self {
            Ty::Epic => "epic",
            Ty::Story => "userstory",
            Ty::Task => "devtask",
            Ty::Sub => "subtask",
            Ty::Defect => "defect",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum St {
    Progress,
    Review,
    Done,
}

/// In welchem Sprint ein Ticket geplant wird.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sp {
    None,
    S10,
    /// Sprint 11, am Ende unerledigt und in Sprint 12 übernommen.
    S11Carry,
    S11,
    /// Sprint 12 von Anfang an.
    S12,
    /// Sprint 12, später hinzugekommen: Arbeitstag und Stunde.
    S12Late(i64, u32),
    /// Sprint 12 von Anfang an, aber am Arbeitstag/Stunde wieder herausgenommen (zurück ins Backlog).
    S12Dropped(i64, u32),
    S13,
}

pub struct Tk {
    pub id: &'static str,
    pub ty: Ty,
    pub en: &'static str,
    pub de: &'static str,
    /// Kennung des übergeordneten Tickets (leer = keins)
    pub parent: &'static str,
    /// Priorität: 1 = Highest … 5 = Lowest
    pub prio: i64,
    /// Story Points (0 = keine)
    pub pts: i64,
    /// Arbeitstag des Anlegens
    pub created: i64,
    pub sprint: Sp,
    /// Statuswechsel: (Arbeitstag, Stunde, Status); Anfangsstatus ist „To Do“.
    pub path: &'static [(i64, u32, St)],
    pub labels: &'static [&'static str],
    /// Version (1 = „1.4“ erschienen, 2 = „1.5“, 3 = „2.0“), 0 = keine
    pub ver: usize,
    /// Meilenstein (1 = Beta, 2 = öffentlicher Start), 0 = keiner
    pub ms: usize,
}

const NONE: &[&str] = &[];

macro_rules! tk {
    ($id:expr, $ty:expr, $en:expr, $de:expr, $parent:expr, $prio:expr, $pts:expr, $created:expr, $sprint:expr, $path:expr, $labels:expr) => {
        Tk { id: $id, ty: $ty, en: $en, de: $de, parent: $parent, prio: $prio, pts: $pts, created: $created, sprint: $sprint, path: $path, labels: $labels, ver: 0, ms: 0 }
    };
}

use Sp::*;
use St::*;
use Ty::*;

/// Die Tickets in der Reihenfolge, in der sie (nach Anlegezeit) ihre Nummern bekommen.
pub fn tickets() -> Vec<Tk> {
    let mut v = vec![
        // ---- Epics
        tk!("e1", Epic, "Orders v2 refactor", "Bestellungen v2: Umbau", "", 2, 0, -8, Sp::None, &[(-7, 9, Progress)], &["backend"]),
        tk!("e2", Epic, "Checkout redesign", "Checkout neu gestalten", "", 2, 0, -8, Sp::None, &[(-6, 9, Progress)], &["frontend"]),
        tk!("e3", Epic, "Docs and onboarding", "Doku und Einstieg", "", 3, 0, -7, Sp::None, &[(-2, 9, Progress)], &["docs"]),
        tk!("e4", Epic, "Infrastructure and reliability", "Infrastruktur und Zuverlässigkeit", "", 2, 0, -7, Sp::None, &[(-1, 9, Progress)], &["infra"]),
        // ---- Stories
        tk!("u1", Story, "Customers can cancel an order", "Kunden können eine Bestellung stornieren", "e1", 2, 8, -6, Sp::None, &[(2, 9, Progress), (19, 16, Done)], &["backend", "api"]),
        tk!("u2", Story, "One-page checkout", "Checkout auf einer Seite", "e2", 2, 13, -6, Sp::None, &[(12, 9, Progress)], &["frontend"]),
        tk!("u3", Story, "Saved payment methods", "Gespeicherte Zahlungsmethoden", "e2", 3, 8, -5, Sp::None, &[], &["frontend", "payments"]),
        tk!("u4", Story, "API getting-started guide", "Einstiegsleitfaden für die API", "e3", 3, 5, -4, Sp::None, &[(22, 9, Progress)], &["docs", "api"]),
        tk!("u5", Story, "Reliable payment processing", "Zuverlässige Zahlungsabwicklung", "e4", 2, 8, -3, Sp::None, &[(21, 9, Progress)], &["backend", "payments"]),
        // ---- Sprint 10
        tk!("x1", Task, "Set up CI pipeline for the orders service", "CI-Pipeline für den Bestell-Service einrichten", "e1", 3, 5, -4, S10, &[(0, 10, Progress), (3, 15, Done)], &["backend", "infra"]),
        tk!("x2", Task, "Orders table migration script", "Migrationsskript für die Bestelltabelle", "u1", 3, 3, -4, S10, &[(1, 9, Progress), (2, 16, Done)], &["backend"]),
        tk!("x3", Task, "Order confirmation e-mail template", "Vorlage für die Bestellbestätigung per E-Mail", "u1", 3, 5, -4, S10, &[(2, 9, Progress), (6, 14, Done)], &["frontend"]),
        tk!("x4", Defect, "Wrong timezone in order timestamps", "Falsche Zeitzone in Bestell-Zeitstempeln", "e1", 2, 2, 3, S10, &[(4, 11, Progress), (5, 10, Done)], &["backend"]),
        // ---- Sprint 11
        tk!("y1", Task, "Order status webhooks", "Webhooks für den Bestellstatus", "e1", 3, 5, 6, S11, &[(10, 9, Progress), (14, 16, Done)], &["api", "backend"]),
        tk!("y2", Task, "Refund endpoint", "Endpunkt für Rückerstattungen", "u1", 2, 5, 6, S11, &[(11, 9, Progress), (16, 15, Done)], &["backend", "payments"]),
        tk!("y3", Task, "Cart API: add and remove line items", "Warenkorb-API: Positionen hinzufügen und entfernen", "u2", 3, 8, 7, S11, &[(10, 10, Progress), (17, 11, Done)], &["api"]),
        tk!("y4", Defect, "Checkout total off by one cent", "Checkout-Summe um einen Cent falsch", "u2", 2, 2, 12, S11, &[(15, 9, Progress), (16, 12, Done)], &["frontend"]),
        tk!("d", Task, "Address form component", "Adressformular-Komponente", "u2", 3, 3, 8, S11Carry, &[(15, 10, Progress), (22, 15, Done)], &["frontend"]),
        // ---- Sprint 12 (aktiv)
        tk!("a", Task, "Add idempotency keys to POST /orders", "Idempotenz-Schlüssel für POST /orders", "e1", 2, 3, 12, S12, &[(20, 9, Progress), (21, 16, Done)], &["backend", "api"]),
        tk!("b", Task, "Paginate the order history endpoint", "Bestellverlauf seitenweise ausliefern", "u1", 3, 5, 12, S12, &[(20, 13, Progress), (26, 11, Done)], &["backend", "api"]),
        tk!("c", Task, "Extract the order service from the monolith", "Bestell-Service aus dem Monolithen herauslösen", "e1", 2, 5, 12, S12, &[(20, 9, Progress), (25, 10, Done)], &["backend", "infra"]),
        tk!("e", Task, "Payment step with saved cards", "Zahlungsschritt mit gespeicherten Karten", "u2", 2, 5, 14, S12, &[(22, 10, Progress)], &["frontend", "payments"]),
        tk!("f", Defect, "Coupon field ignores trailing spaces", "Gutscheinfeld ignoriert Leerzeichen am Ende", "u2", 3, 2, 21, S12Late(22, 11), &[(22, 13, Progress), (23, 10, Done)], &["frontend"]),
        tk!("g", Defect, "Duplicate order after payment retry", "Doppelte Bestellung nach erneutem Zahlungsversuch", "u5", 1, 5, 22, S12Late(23, 10), &[(23, 11, Progress), (26, 10, Review)], &["backend", "payments"]),
        tk!("h", Task, "Staging environment with Terraform", "Staging-Umgebung mit Terraform", "e4", 3, 5, 14, S12, &[(20, 10, Progress), (25, 15, Review), (26, 9, Done)], &["infra"]),
        tk!("i", Task, "Rotate database credentials", "Datenbank-Zugangsdaten rotieren", "u5", 2, 2, 14, S12, &[(21, 9, Progress), (22, 12, Review)], &["security", "infra"]),
        tk!("j", Task, "Getting-started guide for the REST API", "Einstiegsleitfaden für die REST-API", "u4", 3, 3, 15, S12, &[(22, 14, Progress), (26, 10, Done)], &["docs", "api"]),
        tk!("l", Task, "Migrate the docs to the new site", "Doku auf die neue Website umziehen", "u4", 3, 5, 15, S12Dropped(24, 14), &[], &["docs"]),
        // ---- Unteraufgaben
        tk!("s1", Sub, "Define the service API contract", "API-Vertrag des Dienstes festlegen", "c", 3, 0, 20, S12Late(20, 10), &[(20, 10, Progress), (21, 15, Done)], NONE),
        tk!("s2", Sub, "Move order tables into their own schema", "Bestelltabellen in ein eigenes Schema verschieben", "c", 3, 0, 20, S12Late(20, 10), &[(21, 10, Progress), (24, 12, Done)], NONE),
        tk!("s3", Sub, "Tokenize cards with the PSP SDK", "Karten über das PSP-SDK tokenisieren", "e", 3, 0, 23, S12Late(23, 9), &[(23, 10, Progress), (25, 16, Done)], NONE),
        tk!("s5", Sub, "Reproduce with a failing integration test", "Mit fehlschlagendem Integrationstest reproduzieren", "g", 2, 0, 24, S12Late(24, 9), &[(24, 10, Progress), (24, 16, Done)], NONE),
        tk!("s4", Sub, "Write a retry regression test", "Regressionstest für den erneuten Versuch schreiben", "g", 2, 0, 26, S12Late(26, 12), &[(26, 13, Progress)], NONE),
        // ---- Sprint 13 (geplant)
        tk!("o", Task, "Address autocomplete", "Adress-Autovervollständigung", "u2", 3, 5, 24, S13, &[], &["frontend"]),
        tk!("m", Task, "Delete saved cards", "Gespeicherte Karten löschen", "u3", 3, 3, 25, S13, &[], &["frontend", "payments"]),
        tk!("n", Task, "Set a default payment method", "Standard-Zahlungsmethode festlegen", "u3", 3, 3, 25, S13, &[], &["frontend", "payments"]),
        tk!("p", Defect, "Order list shows the wrong currency", "Bestellliste zeigt die falsche Währung", "u1", 2, 3, 26, S13, &[], &["frontend"]),
        // ---- Backlog
        tk!("k", Task, "Alert on failed payments", "Alarm bei fehlgeschlagenen Zahlungen", "u5", 2, 3, 18, S12, &[], &["infra", "payments"]),
        tk!("q", Task, "Changelog automation", "Changelog automatisch erzeugen", "e3", 4, 3, 17, Sp::None, &[], &["docs"]),
        tk!("r", Task, "Dashboards for order latency", "Dashboards für die Bestell-Latenz", "e4", 3, 5, 18, Sp::None, &[], &["infra"]),
        tk!("s", Defect, "CSV export breaks on non-ASCII names", "CSV-Export bricht bei Namen mit Umlauten", "e1", 4, 2, 19, Sp::None, &[], &["backend"]),
        tk!("t", Story, "Guest checkout", "Gast-Checkout", "e2", 3, 8, 19, Sp::None, &[], &["frontend"]),
        tk!("u", Task, "Load test the payment flow", "Lasttest für den Zahlungsablauf", "u5", 3, 5, 20, Sp::None, &[], &["testing", "payments"]),
        tk!("v", Task, "Remove legacy order endpoints", "Alte Bestell-Endpunkte entfernen", "e1", 5, 3, 20, Sp::None, &[], &["backend"]),
        tk!("w", Task, "Accessibility pass on the checkout", "Barrierefreiheit im Checkout prüfen", "u2", 3, 3, 21, Sp::None, &[], &["frontend"]),
    ];
    // Version und Meilenstein der Stories und einiger Aufgaben
    for t in &mut v {
        let (ver, ms) = match t.id {
            "u1" => (1, 1),
            "u2" => (2, 2),
            "u3" => (3, 2),
            "u4" => (2, 0),
            "u5" => (2, 1),
            "t" => (3, 2),
            "x3" | "y2" => (1, 1),
            "a" | "b" | "c" => (2, 1),
            "e" | "d" => (2, 2),
            "g" => (2, 1),
            "m" | "n" | "o" => (3, 2),
            _ => (0, 0),
        };
        t.ver = ver;
        t.ms = ms;
    }
    v
}

/// Beschreibung (Markdown) eines Tickets: (Kennung, Englisch, Deutsch).
pub const DESCRIPTIONS: &[(&str, &str, &str)] = &[
    (
        "e1",
        "Split the order logic out of the monolith into its own service with a clean API, idempotent writes and its own schema.",
        "Die Bestelllogik aus dem Monolithen in einen eigenen Dienst mit sauberer API, idempotenten Schreibzugriffen und eigenem Schema herauslösen.",
    ),
    (
        "g",
        "After the payment provider **retries the callback**, the customer ends up with *two* orders for a single cart.\n\nSeen in production on three orders last week. Suspected cause: the retry creates a new order before the idempotency key from {#a} is checked.\n\nBlocks the rollout of {#e}.",
        "Wenn der Zahlungsanbieter den **Callback wiederholt**, hat der Kunde *zwei* Bestellungen für einen einzigen Warenkorb.\n\nLetzte Woche bei drei Bestellungen in Produktion aufgetreten. Vermutete Ursache: Der erneute Versuch legt eine neue Bestellung an, bevor der Idempotenz-Schlüssel aus {#a} geprüft wird.\n\nBlockiert die Auslieferung von {#e}.",
    ),
    (
        "c",
        "Move order creation, order history and refunds behind a small HTTP API.\n\n- the monolith calls the new service only through the client library\n- orders get their own database schema\n- no behaviour change for customers",
        "Bestellanlage, Bestellverlauf und Rückerstattungen hinter eine kleine HTTP-API legen.\n\n- der Monolith ruft den neuen Dienst nur über die Client-Bibliothek auf\n- Bestellungen bekommen ein eigenes Datenbankschema\n- keine Änderung des Verhaltens für Kunden",
    ),
    (
        "e",
        "Show the customer's saved cards in the payment step. The card is tokenized by the payment provider, we never see the number.",
        "Im Zahlungsschritt die gespeicherten Karten des Kunden anzeigen. Die Karte wird vom Zahlungsanbieter tokenisiert, wir sehen die Nummer nie.",
    ),
    (
        "a",
        "`POST /orders` accepts an `Idempotency-Key` header. A repeated request with the same key returns the original order instead of creating a new one.",
        "`POST /orders` akzeptiert den Header `Idempotency-Key`. Eine wiederholte Anfrage mit demselben Schlüssel liefert die ursprüngliche Bestellung zurück, statt eine neue anzulegen.",
    ),
    (
        "u2",
        "Address, shipping and payment on a single page, with inline validation.",
        "Adresse, Versand und Zahlung auf einer einzigen Seite, mit Prüfung direkt im Formular.",
    ),
];

/// Schritte zum Nachstellen des Fehlers (Feld „Steps to reproduce“ der Fehler).
pub const STEPS: &[(&str, &str, &str)] = &[(
    "g",
    "1. Put an item in the cart and go to the checkout\n2. Pay with a card that triggers a 3-D Secure retry\n3. Let the callback fire twice (see the `worker` log)\n4. The order list shows two identical orders",
    "1. Einen Artikel in den Warenkorb legen und zur Kasse gehen\n2. Mit einer Karte zahlen, die einen 3-D-Secure-Versuch wiederholt\n3. Den Callback zweimal auslösen (siehe Log von `worker`)\n4. Die Bestellliste zeigt zwei identische Bestellungen",
)];

/// Kommentar: (Ticket, Arbeitstag, Stunde, Agent-Sitzung oder leer, Englisch, Deutsch).
pub const COMMENTS: &[(&str, i64, u32, &str, &str, &str)] = &[
    (
        "g",
        23,
        11,
        "worker",
        "Reproduced locally with `pytest -k test_payment_retry`. The second callback skips the key check because the key is only stored *after* the order exists.",
        "Lokal mit `pytest -k test_payment_retry` nachgestellt. Der zweite Callback überspringt die Schlüsselprüfung, weil der Schlüssel erst gespeichert wird, *nachdem* die Bestellung existiert.",
    ),
    (
        "g",
        24,
        9,
        "",
        "Thanks! Let's store the key first, in the same transaction as the order. I'll pair on this after the stand-up.",
        "Danke! Wir speichern den Schlüssel zuerst, in derselben Transaktion wie die Bestellung. Ich mache das nach dem Stand-up zusammen mit dir.",
    ),
    (
        "g",
        26,
        10,
        "",
        "Fix is in review. It needs the migration from {#x2} on staging first.",
        "Die Korrektur ist im Review. Sie braucht vorher die Migration aus {#x2} auf Staging.",
    ),
    (
        "c",
        22,
        15,
        "",
        "The contract is agreed with the checkout team ({#s1}). Next step is the schema move.",
        "Der Vertrag ist mit dem Checkout-Team abgestimmt ({#s1}). Als Nächstes folgt der Umzug des Schemas.",
    ),
    (
        "e",
        26,
        16,
        "",
        "Works against the PSP sandbox. Still polishing the empty state for customers without a saved card.",
        "Funktioniert gegen die Sandbox des Zahlungsanbieters. Der Leerzustand für Kunden ohne gespeicherte Karte wird noch verfeinert.",
    ),
    (
        "a",
        21,
        16,
        "",
        "Keys are kept for 24 hours; a repeat request inside that window returns the original order.",
        "Schlüssel werden 24 Stunden aufbewahrt; eine Wiederholung in diesem Zeitraum liefert die ursprüngliche Bestellung.",
    ),
];

/// Verknüpfung zweier Tickets: (von, Typ-Schlüssel, nach). `g blocks e` erscheint bei `e` als „is blocked by“.
pub const LINKS: &[(&str, &str, &str)] = &[("g", "blocks", "e"), ("g", "relates", "a"), ("c", "relates", "h"), ("y4", "relates", "f")];

/// Sterne und Markierungen.
pub const STARRED: &[&str] = &["g", "c", "e", "u2"];
pub const FLAGGED: &[&str] = &["g"];

/// Zeiteinträge: (Ticket, Arbeitstag, Dauer, Art der Arbeit: 0 Development, 1 Test, 2 Planning, 3 Refinement).
pub const TIME: &[(&str, i64, &str, usize)] = &[
    ("a", 20, "2h", 0),
    ("a", 21, "3h", 0),
    ("b", 20, "2h", 0),
    ("b", 24, "3h 30m", 0),
    ("b", 26, "1h 30m", 1),
    ("c", 20, "1h", 2),
    ("c", 21, "4h", 0),
    ("c", 22, "2h 30m", 0),
    ("c", 24, "3h", 0),
    ("c", 25, "2h", 1),
    ("d", 22, "2h 30m", 0),
    ("e", 22, "2h", 0),
    ("e", 24, "4h", 0),
    ("e", 25, "3h", 0),
    ("e", 26, "2h 30m", 0),
    ("f", 22, "1h", 0),
    ("f", 23, "30m", 1),
    ("g", 23, "1h 30m", 0),
    ("g", 24, "3h", 0),
    ("g", 25, "2h", 0),
    ("g", 26, "2h 30m", 1),
    ("h", 20, "2h", 0),
    ("h", 22, "3h", 0),
    ("h", 23, "2h", 0),
    ("h", 25, "2h 30m", 0),
    ("i", 21, "1h", 0),
    ("i", 22, "1h", 1),
    ("j", 22, "1h 30m", 0),
    ("j", 24, "2h", 0),
    ("j", 26, "1h", 3),
    ("l", 21, "1h", 3),
    ("u1", 21, "1h", 2),
    ("u2", 24, "1h", 3),
    ("u5", 25, "1h", 2),
    ("x1", 0, "3h", 0),
    ("x3", 4, "3h", 0),
    ("y2", 13, "4h", 0),
    ("y3", 12, "6h", 0),
];

/// Texte, die nur in einer Sprache anders sind.
pub struct Texts {
    pub project_name: &'static str,
    pub project_desc: &'static str,
    pub review: &'static str,
    pub sprint_goals: [&'static str; 4],
    pub version_note: &'static str,
    pub milestones: [&'static str; 2],
    pub board_main: &'static str,
    pub board_sprint: &'static str,
    pub board_main_desc: &'static str,
    pub dash_name: &'static str,
    pub filters: [(&'static str, &'static str); 3],
    pub tile_search: &'static str,
}

pub fn texts(de: bool) -> Texts {
    if de {
        Texts {
            project_name: "Acme Shop",
            project_desc: "Online-Shop: Bestellungen, Checkout und Infrastruktur",
            review: "In Prüfung",
            sprint_goals: ["Bestell-Service vorbereiten", "Erstattungen und Warenkorb-API", "Idempotente Bestellungen und das neue Adressformular ausliefern", "Gespeicherte Zahlungsmethoden"],
            version_note: "Meilenstein-Version",
            milestones: ["Beta-Start", "Öffentlicher Start"],
            board_main: "Team-Board",
            board_sprint: "Sprint-Board",
            board_main_desc: "Alle Tickets des Projekts",
            dash_name: "Team-Übersicht",
            filters: [("Offene Fehler", "type = Defect AND status != Done"), ("In Prüfung", "status = \"In Prüfung\" ORDER BY priority"), ("Hohe Priorität", "priority IN (Highest, High) AND status != Done")],
            tile_search: "Offene Fehler",
        }
    } else {
        Texts {
            project_name: "Acme Shop",
            project_desc: "Online shop: orders, checkout and infrastructure",
            review: "In Review",
            sprint_goals: ["Prepare the order service", "Refunds and the cart API", "Ship idempotent orders and the new address form", "Saved payment methods"],
            version_note: "Milestone version",
            milestones: ["Beta launch", "Public launch"],
            board_main: "Team board",
            board_sprint: "Sprint board",
            board_main_desc: "All tickets of the project",
            dash_name: "Team overview",
            filters: [("Open defects", "type = Defect AND status != Done"), ("In review", "status = \"In Review\" ORDER BY priority"), ("High priority", "priority IN (Highest, High) AND status != Done")],
            tile_search: "Open defects",
        }
    }
}

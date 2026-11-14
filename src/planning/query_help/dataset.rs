//! Gemeinsame Beispieldaten „Web shop“ (Projekt `WEB`) für die meisten Hilfe-Beispiele.
//!
//! Die Daten sind so gewählt, dass jede Abfrage der Hilfe sowohl Treffer als auch Nicht-Treffer hat. Alle Datumsangaben sind relativ
//! zu „heute“ (`@-2` = vor zwei Tagen), damit `currentDate()` an jedem Tag dasselbe Ergebnis liefert. Englische Namen wie in den
//! Screenshots (die Standardnamen der Typen/Status/Prioritäten stammen aus `planning::names`).

use super::demo::*;
use crate::planning::fields::FieldKind;
use crate::planning::statuses::StatusCategory;

fn s(v: &str) -> String {
    v.to_string()
}

fn field(name: &str, kind: FieldKind, types: &[&str]) -> DemoField {
    DemoField { name: s(name), kind, options: Vec::new(), types: types.iter().map(|t| s(t)).collect() }
}

/// Das Projekt „Web shop“: fünf Tickettypen, ein eigener Status „Development finished“, drei Sprints (einer aktiv), vier Versionen,
/// drei Terminals und 22 Tickets.
pub fn shop() -> DemoSpec {
    let mut d = DemoSpec {
        project_key: s("WEB"),
        project_name: s("Web shop"),
        types: ["Epic", "User story", "Dev task", "Subtask", "Defect"].iter().map(|t| s(t)).collect(),
        statuses: vec![
            DemoStatus { name: s("To Do"), category: StatusCategory::Todo },
            DemoStatus { name: s("In Progress"), category: StatusCategory::Progress },
            // benutzerdefinierter Status des Projekts (nicht Teil der Standard-Status)
            DemoStatus { name: s("Development finished"), category: StatusCategory::Progress },
            DemoStatus { name: s("Done"), category: StatusCategory::Done },
        ],
        priorities: ["Highest", "High", "Medium", "Low", "Lowest"].iter().map(|t| s(t)).collect(),
        fields: vec![
            field("Description", FieldKind::Rich, &[]),
            field("Labels", FieldKind::Labels, &[]),
            field("Release version", FieldKind::Version, &["Epic"]),
            field("Story points", FieldKind::Number, &["User story", "Dev task", "Subtask", "Defect"]),
            field("Due", FieldKind::Date, &["User story", "Dev task", "Subtask", "Defect"]),
            field("Terminal", FieldKind::Terminal, &["User story", "Dev task", "Defect"]),
        ],
        sprints: vec![
            DemoSprint { name: s("Sprint 14"), start: s("@-26"), end: s("@-13"), state: SprintState::Closed },
            DemoSprint { name: s("Sprint 15"), start: s("@-6"), end: s("@+7"), state: SprintState::Active },
            DemoSprint { name: s("Sprint 16"), start: s("@+8"), end: s("@+21"), state: SprintState::Planned },
        ],
        versions: vec![
            DemoVersion { name: s("1.0.0"), due: Some(s("@-120")), released: true },
            DemoVersion { name: s("1.1.0"), due: Some(s("@-60")), released: true },
            DemoVersion { name: s("2.0.0"), due: Some(s("@+30")), released: false },
            DemoVersion { name: s("2.1.0"), due: Some(s("@+90")), released: false },
        ],
        terminals: ["api-work", "login-fix", "docs"].iter().map(|t| s(t)).collect(),
        current_terminal: Some(s("login-fix")),
        tickets: Vec::new(),
    };
    let t = DemoTicket::new;

    // Epics (WEB-1 … WEB-5)
    d.add(t("Epic", "Checkout redesign").status("Development finished").priority("High").text("Release version", "2.0.0").list("Labels", &["frontend"]).favorite().changed_by("ui"));
    d.add(t("Epic", "Customer accounts").status("Done").priority("Highest").text("Release version", "2.0.0").list("Labels", &["backend"]).changed_by("ui"));
    d.add(t("Epic", "Payment providers").status("In Progress").priority("Highest").text("Release version", "2.0.0").list("Labels", &["backend", "api"]).changed_by("ui"));
    d.add(t("Epic", "Mobile app").status("To Do").priority("Medium").changed_by("ui"));
    d.add(t("Epic", "Legacy cleanup").status("Done").priority("Low").list("Labels", &["legacy"]).changed_by("ui"));

    // User stories (WEB-6 … WEB-10)
    d.add(t("User story", "Log in with e-mail and password").status("Done").priority("High").sprint("Sprint 14").parent("WEB-2").num("Story points", 5.0).text("Due", "@-15").list("Labels", &["backend", "auth"]).changed_by("ui"));
    d.add(t("User story", "Reset password by e-mail").status("Done").priority("Medium").sprint("Sprint 14").num("Story points", 3.0).text("Due", "@-14").list("Labels", &["backend", "auth"]).changed_by("ui"));
    d.add(t("User story", "Save payment method").status("In Progress").priority("Highest").sprint("Sprint 15").parent("WEB-3").num("Story points", 8.0).text("Due", "@+3").list("Labels", &["backend", "payments"]).favorite().changed_by("agent"));
    d.add(t("User story", "One-page checkout form").status("Development finished").priority("High").sprint("Sprint 15").parent("WEB-1").num("Story points", 8.0).text("Due", "@+5").list("Labels", &["frontend"]).changed_by("ui"));
    d.add(t("User story", "Order summary after checkout").status("To Do").priority("Medium").sprint("Sprint 15").parent("WEB-1").num("Story points", 3.0).text("Due", "@+6").list("Labels", &["frontend"]).blocked().changed_by("ui"));

    // Dev tasks (WEB-11 … WEB-13)
    d.add(t("Dev task", "Integrate payment provider API").status("In Progress").priority("High").sprint("Sprint 15").parent("WEB-3").num("Story points", 5.0).text("Due", "@-2").list("Labels", &["backend", "api"]).text("Terminal", "api-work").favorite().changed_by("agent"));
    d.add(t("Dev task", "Add login rate limiting").status("To Do").priority("Medium").sprint("Sprint 15").parent("WEB-2").num("Story points", 2.0).text("Due", "@+1").list("Labels", &["backend", "auth"]).text("Terminal", "login-fix").changed_by("ui"));
    d.add(t("Dev task", "Migrate cart table").status("To Do").priority("Low").sprint("Sprint 16").num("Story points", 3.0).text("Due", "@+12").list("Labels", &["backend"]).changed_by("ui"));

    // Defects (WEB-14 … WEB-18)
    d.add(
        t("Defect", "Login fails with umlaut in password")
            .status("In Progress")
            .priority("Highest")
            .sprint("Sprint 15")
            .num("Story points", 2.0)
            .text("Due", "@-1")
            .list("Labels", &["backend", "auth"])
            .text("Terminal", "login-fix")
            .text("Description", "Users with ä, ö or ü in their password cannot log in.")
            .favorite()
            .changed_by("agent"),
    );
    d.add(
        t("Defect", "Cart total wrong with discount code")
            .status("To Do")
            .priority("High")
            .sprint("Sprint 15")
            .num("Story points", 3.0)
            .text("Due", "@+2")
            .list("Labels", &["backend"])
            .text("Description", "Discount and coupon codes are applied twice on the cart page.")
            .blocked()
            .changed_by("ui"),
    );
    d.add(t("Defect", "Typo on checkout button").status("To Do").priority("Lowest").num("Story points", 1.0).list("Labels", &["frontend"]).changed_by("ui"));
    d.add(t("Defect", "Login page flickers on Safari").status("To Do").priority("Medium").text("Due", "@+9").list("Labels", &["frontend", "auth"]).changed_by("ui"));
    d.add(t("Defect", "Old coupon codes still accepted").status("Done").priority("High").sprint("Sprint 14").num("Story points", 2.0).text("Due", "@-14").list("Labels", &["backend"]).changed_by("ui"));

    // Subtasks (WEB-19, WEB-20) und weitere Tickets im Backlog (WEB-21, WEB-22)
    d.add(t("Subtask", "Write tests for rate limiting").status("To Do").priority("Medium").sprint("Sprint 15").parent("WEB-12").num("Story points", 1.0).text("Due", "@+1").list("Labels", &["backend", "auth"]).changed_by("ui"));
    d.add(t("Subtask", "Write error message texts").status("Development finished").priority("Low").sprint("Sprint 15").parent("WEB-9").num("Story points", 1.0).text("Due", "@-3").list("Labels", &["frontend"]).changed_by("ui"));
    d.add(t("Dev task", "Upgrade web framework").status("To Do").priority("Low").num("Story points", 5.0).list("Labels", &["backend", "legacy"]).changed_by("ui"));
    d.add(t("User story", "Wishlist for customers").status("To Do").priority("Lowest").parent("WEB-4").num("Story points", 5.0).list("Labels", &["frontend"]).changed_by("ui"));
    d
}

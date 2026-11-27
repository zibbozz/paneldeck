//! Vorschau im Theme-Editor (#309): eine Musterseite mit echten egui-Bedienelementen und Beispieltexten. Alles lässt
//! sich bedienen, bewirkt aber nichts. Jedes Element merkt sich, welche Farben es zeigt (`Marks`); ein Klick darauf
//! wählt die Farbe in der Liste, ein Hinweis der Design-Prüfung hebt die Elemente hervor.

use crate::state::Lang;
use crate::style::{self, Icon};
use crate::theme::{Theme, UiColors};
use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Bereiche der Vorschau mit ihrer Farbe (die erste passende kleinste Fläche gewinnt beim Klick).
pub(super) type Marks = Vec<(Rect, &'static str)>;

/// Zustand der Beispiel-Bedienelemente (nur für die Vorschau).
pub(super) struct Sample {
    text: String,
    focused_text: String,
    error_text: String,
    check: bool,
    check2: bool,
    slider: f32,
    toggle: bool,
    tab: usize,
    radio: usize,
}

impl Default for Sample {
    fn default() -> Self {
        Sample { text: String::new(), focused_text: "paneldeck".into(), error_text: "12:7O".into(), check: true, check2: false, slider: 0.6, toggle: true, tab: 0, radio: 1 }
    }
}

fn mark(m: &mut Marks, r: Rect, tok: &'static str) {
    m.push((r, tok));
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).small().strong().color(ui.visuals().weak_text_color()));
    ui.add_space(2.0);
}

/// Breite des Inhalts eines gefärbten Kastens.
const BOX_W: f32 = 260.0;

/// Gefärbter Kasten (Hinweis, Warnung, Fehler, Toast) in einer umbrechenden Zeile. Passt er nicht mehr in die Zeile,
/// beginnt er eine neue – früher bekam er nur den Rest der Zeile, der Text brach Wort für Wort um, und lange Wörter
/// ragten rechts hinaus (das Editor-Fenster wuchs mit).
fn tinted_box(ui: &mut Ui, m: &mut Marks, col: Color32, tok: &'static str, title: &str, body: &str) {
    // In einer umbrechenden Zeile meint `available_width` die ganze Zeile; der Rest der aktuellen Zeile ist das hier.
    if ui.available_size_before_wrap().x < BOX_W + 22.0 {
        ui.end_row();
    }
    let r = egui::Frame::new()
        .fill(col.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0_f32, col.gamma_multiply(0.55)))
        .corner_radius(CornerRadius::same(style::RADIUS))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(BOX_W.min(ui.available_width()));
            ui.label(RichText::new(title).strong().color(col));
            ui.label(body);
        })
        .response;
    mark(m, r.rect, tok);
}

/// Die ganze Musterseite. `t`/`u`: bearbeitetes Theme und seine Oberflächenfarben (Stil ist schon gesetzt).
pub(super) fn page(ui: &mut Ui, t: &Theme, u: &UiColors, lang: Lang, s: &mut Sample, m: &mut Marks) {
    let full = ui.max_rect();
    mark(m, full, "window_bg");
    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);

    heading(ui, lang.t("Knöpfe", "Buttons"));
    ui.horizontal_wrapped(|ui| {
        let r = ui.button(lang.t("Normal", "Normal"));
        mark(m, r.rect, "widget_bg");
        let r = ui.add(egui::Button::new(RichText::new(lang.t("Überfahren", "Hover")).color(u.text_strong)).fill(u.hover_bg).stroke(Stroke::new(1.0_f32, u.hover_border)));
        mark(m, r.rect, "hover_bg");
        let r = ui.add(egui::Button::new(RichText::new(lang.t("Gedrückt", "Pressed")).color(u.text_strong)).fill(u.active_bg).stroke(Stroke::new(1.0_f32, u.accent)));
        mark(m, r.rect, "active_bg");
        let r = ui.add_enabled(false, egui::Button::new(lang.t("Deaktiviert", "Disabled")));
        mark(m, r.rect, "text_dim");
        let r = ui.add(egui::Button::new(RichText::new(lang.t("Neues Terminal", "New terminal")).strong()).fill(u.accent.gamma_multiply(0.85)));
        mark(m, r.rect, "accent");
        let r = ui.add(egui::Button::new(RichText::new(lang.t("Löschen", "Delete")).color(Color32::WHITE)).fill(u.danger_button));
        mark(m, r.rect, "danger_button");
        let r = style::icon_btn(ui, Icon::Gear, lang.t("Einstellungen", "Settings"), false);
        mark(m, r.rect, "text");
    });

    heading(ui, lang.t("Eingaben", "Inputs"));
    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut s.text).hint_text(lang.t("Leer – Platzhalter", "Empty – placeholder")).desired_width(170.0));
        mark(m, r.rect, "input_bg");
        let r = ui.add(egui::TextEdit::singleline(&mut s.focused_text).desired_width(150.0));
        ui.painter().rect_stroke(r.rect.expand(1.0), CornerRadius::same(style::RADIUS), Stroke::new(1.5_f32, u.focus), StrokeKind::Outside);
        mark(m, r.rect.expand(2.0), "focus");
        ui.vertical(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut s.error_text).desired_width(150.0));
            ui.painter().rect_stroke(r.rect.expand(1.0), CornerRadius::same(style::RADIUS), Stroke::new(1.5_f32, u.error), StrokeKind::Outside);
            mark(m, r.rect.expand(2.0), "error");
            let l = ui.label(RichText::new(lang.t("Ungültige Uhrzeit", "Invalid time")).small().color(u.error));
            mark(m, l.rect, "error");
        });
    });

    heading(ui, lang.t("Auswahl und Regler", "Checkboxes and sliders"));
    ui.horizontal_wrapped(|ui| {
        let r = ui.checkbox(&mut s.check, lang.t("Aktiv", "Checked"));
        mark(m, r.rect, "widget_bg");
        let r = ui.checkbox(&mut s.check2, lang.t("Aus", "Unchecked"));
        mark(m, r.rect, "widget_border");
        let r = ui.radio_value(&mut s.radio, 0, "A");
        mark(m, r.rect, "widget_bg");
        let r = ui.radio_value(&mut s.radio, 1, "B");
        mark(m, r.rect, "widget_bg");
        let r = style::toggle(ui, &mut s.toggle);
        mark(m, r.rect, "accent");
        let r = ui.add(egui::Slider::new(&mut s.slider, 0.0..=1.0).show_value(false));
        mark(m, r.rect, "widget_bg");
        let r = ui.add(egui::ProgressBar::new(s.slider).desired_width(120.0).fill(u.progress));
        mark(m, r.rect, "progress");
    });

    heading(ui, lang.t("Tabs", "Tabs"));
    ui.horizontal(|ui| {
        for (i, name) in [lang.t("Allgemein", "General"), lang.t("Darstellung", "Appearance"), lang.t("Tastenkürzel", "Shortcuts")].into_iter().enumerate() {
            let r = style::tab(ui, name, s.tab == i);
            if r.clicked() {
                s.tab = i;
            }
            mark(m, r.rect, if s.tab == i { "tab_active" } else { "text" });
        }
    });

    ui.add_space(4.0);
    two_cols(
        ui,
        m,
        "sidebar",
        12.0,
        |ui, m| {
            heading(ui, lang.t("Seitenleiste", "Sidebar"));
            sidebar(ui, u, lang, m);
        },
        |ui, m| {
            heading(ui, lang.t("Chips und Labels", "Chips and labels"));
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(360.0);
                let r = style::pill(ui, "Paneldeck", Some(12), u.accent, true);
                mark(m, r.rect, "chip_active");
                let r = style::pill(ui, lang.t("Privat", "Private"), Some(3), u.success, false);
                mark(m, r.rect, "text");
                for (txt, col, tok) in [("bug", u.error, "error"), ("ui", u.info, "info"), ("docs", u.success, "success")] {
                    let r = egui::Frame::new().fill(col.gamma_multiply(0.18)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
                        ui.label(RichText::new(txt).small().color(col));
                    });
                    mark(m, r.response.rect, tok);
                }
            });
            let (rect, _) = ui.allocate_exact_size(Vec2::new(300.0, 24.0), Sense::hover());
            let w = style::status_pill(ui.painter(), Pos2::new(rect.right(), rect.center().y), lang.t("bereit", "ready"), u.success);
            mark(m, Rect::from_min_max(Pos2::new(rect.right() - w, rect.top()), rect.max), "success");
            let w2 = style::status_pill(ui.painter(), Pos2::new(rect.right() - w - 8.0, rect.center().y), lang.t("wartet", "waiting"), u.warning);
            mark(m, Rect::from_min_max(Pos2::new(rect.right() - w - 8.0 - w2, rect.top()), Pos2::new(rect.right() - w - 8.0, rect.bottom())), "warning");

            heading(ui, lang.t("Dialog", "Dialog"));
            let r = egui::Frame::window(ui.style()).show(ui, |ui| {
                ui.set_width(330.0);
                ui.label(RichText::new(lang.t("Sitzung schließen?", "Close session?")).heading());
                ui.label(lang.t("Im Terminal läuft noch ein Programm.", "A program is still running in the terminal."));
                let d = ui.label(RichText::new(lang.t("Ungespeicherte Ausgaben gehen verloren.", "Unsaved output will be lost.")).small().color(u.text_dim));
                mark(m, d.rect, "text_dim");
                let l = ui.hyperlink_to(lang.t("Mehr erfahren", "Learn more"), "https://example.invalid");
                mark(m, l.rect, "link");
                ui.horizontal(|ui| {
                    let r = ui.add(egui::Button::new(RichText::new(lang.t("Schließen", "Close")).color(Color32::WHITE)).fill(u.danger_button));
                    mark(m, r.rect, "danger_button");
                    let r = ui.button(lang.t("Abbrechen", "Cancel"));
                    mark(m, r.rect, "widget_bg");
                });
            });
            mark(m, r.response.rect, "border");
        },
    );

    heading(ui, lang.t("Toast und Hinweise", "Toast and notices"));
    ui.horizontal_wrapped(|ui| {
        let r = egui::Frame::window(ui.style()).inner_margin(Margin::same(0)).show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(260.0, 44.0), Sense::hover());
            ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())), CornerRadius::same(2), u.info);
            ui.painter().text(rect.min + Vec2::new(14.0, 8.0), egui::Align2::LEFT_TOP, lang.t("Theme gespeichert", "Theme saved"), FontId::proportional(13.5), u.text);
            ui.painter().text(rect.min + Vec2::new(14.0, 26.0), egui::Align2::LEFT_TOP, lang.t("vor 2 Sekunden", "2 seconds ago"), FontId::proportional(11.5), u.text_dim);
        });
        mark(m, r.response.rect, "info");
        tinted_box(ui, m, u.info, "info", lang.t("Hinweis", "Notice"), lang.t("Neue Version verfügbar.", "A new version is available."));
        tinted_box(ui, m, u.warning, "warning", lang.t("Warnung", "Warning"), lang.t("Die Warteschlange ist pausiert.", "The queue is paused."));
        tinted_box(ui, m, u.error, "error", lang.t("Fehler", "Error"), lang.t("Datei konnte nicht gespeichert werden.", "Could not save the file."));
        tinted_box(ui, m, u.danger, "danger", lang.t("Freigabe nötig", "Approval needed"), lang.t("Claude wartet auf deine Erlaubnis.", "Claude is waiting for your permission."));
    });

    two_cols(
        ui,
        m,
        "board",
        12.0,
        |ui, m| {
            heading(ui, lang.t("Ticket und Board-Spalte", "Ticket card and board column"));
            board(ui, u, lang, m);
        },
        |ui, m| {
            heading(ui, lang.t("Chat und Freigabe", "Chat and tool approval"));
            chat(ui, u, lang, m);
        },
    );

    heading(ui, lang.t("Terminal", "Terminal"));
    terminal(ui, t, m);

    two_cols(
        ui,
        m,
        "diff",
        16.0,
        |ui, m| {
            heading(ui, "Diff");
            diff(ui, u, m);
        },
        |ui, m| {
            heading(ui, lang.t("Code mit Syntaxfarben", "Code with syntax colours"));
            code(ui, u, m);
        },
    );
    heading(ui, lang.t("Editor: Syntaxfarben", "Editor: syntax colours"));
    editor_code(ui, t, u, m);
    ui.add_space(12.0);
}

/// Zwei Spalten nebeneinander – oder untereinander, wenn sie nebeneinander nicht in die Breite passen (schmales
/// Editor-Fenster). Die Spalten haben feste Breiten; ihre Summe stammt aus dem letzten Bild.
fn two_cols(ui: &mut Ui, m: &mut Marks, salt: &str, gap: f32, left: impl FnOnce(&mut Ui, &mut Marks), right: impl FnOnce(&mut Ui, &mut Marks)) {
    let id = ui.id().with(("pd_preview_cols", salt));
    let need = ui.ctx().data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    let (wl, wr) = if need <= ui.available_width() {
        ui.horizontal_top(|ui| {
            let wl = ui.vertical(|ui| left(ui, m)).response.rect.width();
            ui.add_space(gap);
            let wr = ui.vertical(|ui| right(ui, m)).response.rect.width();
            (wl, wr)
        })
        .inner
    } else {
        let wl = ui.vertical(|ui| left(ui, m)).response.rect.width();
        let wr = ui.vertical(|ui| right(ui, m)).response.rect.width();
        (wl, wr)
    };
    let need = wl + gap + wr + 2.0 * ui.spacing().item_spacing.x;
    ui.ctx().data_mut(|d| d.insert_temp(id, need));
}

fn sidebar(ui: &mut Ui, u: &UiColors, lang: Lang, m: &mut Marks) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(250.0, 150.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(style::RADIUS), u.sidebar_bg);
    mark(m, rect, "sidebar_bg");
    let row_h = 26.0;
    let rows: [(&str, Option<(Color32, &'static str)>, bool, bool); 5] = [
        (lang.t("Projekte", "Projects"), None, false, true),
        ("api-server", Some((u.success, "success")), true, false),
        ("frontend", Some((u.accent, "accent")), false, false),
        ("docs", Some((u.warning, "warning")), false, false),
        ("deploy", Some((u.danger, "danger")), false, false),
    ];
    for (i, (name, dot, active, folder)) in rows.into_iter().enumerate() {
        let r = Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0 + i as f32 * (row_h + 2.0)), Vec2::new(rect.width() - 12.0, row_h));
        if active {
            p.rect_filled(r, CornerRadius::same(8), u.sidebar_active);
            mark(m, r, "sidebar_active");
        }
        let x = if folder { r.min.x + 8.0 } else { r.min.x + 22.0 };
        if folder {
            style::paint_icon(p, Icon::Folder, Pos2::new(x + 6.0, r.center().y), u.text_dim);
            p.text(Pos2::new(x + 18.0, r.center().y), egui::Align2::LEFT_CENTER, name, FontId::proportional(12.5), u.text_dim);
            mark(m, r, "text_dim");
        } else {
            p.text(Pos2::new(x + 12.0, r.center().y), egui::Align2::LEFT_CENTER, name, FontId::proportional(13.0), if active { u.text_strong } else { u.text });
            if let Some((c, tok)) = dot {
                let dc = Pos2::new(x + 2.0, r.center().y);
                p.circle_filled(dc, 4.0, c);
                mark(m, Rect::from_center_size(dc, Vec2::splat(12.0)), tok);
            }
            if !active {
                mark(m, Rect::from_min_size(Pos2::new(x + 12.0, r.min.y), Vec2::new(90.0, r.height())), "text");
            }
        }
    }
}

fn board(ui: &mut Ui, u: &UiColors, lang: Lang, m: &mut Marks) {
    let col = egui::Frame::new().fill(u.surface).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(260.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("In Arbeit", "In progress")).strong());
            ui.label(RichText::new("2").small().color(u.text_dim));
        });
        for (i, (title, flag, star)) in [(lang.t("Theme-Editor bauen", "Build the theme editor"), true, true), (lang.t("Doku ergänzen", "Update the docs"), false, false)].into_iter().enumerate() {
            let card = egui::Frame::new()
                .fill(u.window_bg)
                .stroke(Stroke::new(1.0_f32, u.border))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("PAN-{}", 309 + i)).small().color(u.text_dim));
                        if star {
                            let r = ui.label(RichText::new("★").color(u.ticket_star));
                            mark(m, r.rect, "ticket_star");
                        }
                        if flag {
                            let r = ui.label(RichText::new("⚑").color(u.ticket_flag));
                            mark(m, r.rect, "ticket_flag");
                        }
                    });
                    ui.label(title);
                    ui.horizontal(|ui| {
                        let r = egui::Frame::new().fill(u.chip_active).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 1)).show(ui, |ui| {
                            ui.label(RichText::new("2.6.0").small());
                        });
                        mark(m, r.response.rect, "chip_active");
                    });
                });
            mark(m, card.response.rect, "window_bg");
        }
    });
    mark(m, col.response.rect, "surface");
}

fn chat(ui: &mut Ui, u: &UiColors, lang: Lang, m: &mut Marks) {
    ui.set_width(340.0);
    let text = lang.t("Kannst du die Tests laufen lassen?", "Can you run the tests?");
    let g = ui.painter().layout(text.to_string(), FontId::proportional(13.5), u.text_strong, 240.0);
    let size = g.size() + Vec2::new(20.0, 14.0);
    let (row, _) = ui.allocate_exact_size(Vec2::new(340.0, size.y), Sense::hover());
    let bubble = Rect::from_min_size(Pos2::new(row.max.x - size.x, row.min.y), size);
    ui.painter().rect_filled(bubble, 10.0, u.chat_user_bg);
    ui.painter().galley(bubble.min + Vec2::new(10.0, 7.0), g, u.text_strong);
    mark(m, bubble, "chat_user_bg");
    ui.label(RichText::new("qwen3:8b").small().color(u.text_dim));
    ui.label(lang.t("Gern – ich starte cargo test.", "Sure – I'll start cargo test."));
    let r = egui::Frame::new().fill(u.chat_approval_bg).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(lang.t("run_command wartet auf Freigabe", "run_command needs approval")).color(u.warning));
        ui.label(RichText::new("cargo test --release").monospace());
        // Freigabe-Knöpfe ohne eigene Farbe wie im Chat (#390).
        ui.horizontal(|ui| {
            for label in [lang.t("Erlauben", "Allow"), lang.t("Ablehnen", "Deny")] {
                let r = ui.button(label);
                mark(m, r.rect, "widget_bg");
            }
        });
    });
    mark(m, r.response.rect, "chat_approval_bg");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(lang.t("Nachricht …", "Message …")).color(u.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(RichText::new(lang.t("Senden", "Send")).strong()).fill(u.chat_send));
            mark(m, r.rect, "chat_send");
        });
    });
}

fn terminal(ui: &mut Ui, t: &Theme, m: &mut Marks) {
    let font = FontId::monospace(13.0);
    let cw = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
    let lh = 18.0;
    let w = (ui.available_width() - 4.0).clamp(300.0, 760.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, lh * 6.0 + 16.0), Sense::hover());
    let p = ui.painter_at(rect);
    let bg = t.bg.to_opaque();
    p.rect_filled(rect, CornerRadius::same(6), bg);
    mark(m, rect, "bg");
    let origin = rect.min + Vec2::new(10.0, 8.0);
    let put = |col: usize, row: usize, text: &str, c: Color32, tok: &'static str, m: &mut Marks| {
        let pos = origin + Vec2::new(col as f32 * cw, row as f32 * lh);
        p.text(pos, egui::Align2::LEFT_TOP, text, font.clone(), c);
        mark(m, Rect::from_min_size(pos, Vec2::new(text.chars().count() as f32 * cw, lh)), tok);
    };
    put(0, 0, "PS C:\\paneldeck>", t.ansi[2], "ansi2", m);
    put(17, 0, "git status", t.fg, "fg", m);
    put(0, 1, "modified:", t.ansi[1], "ansi1", m);
    put(10, 1, "src/theme.rs", t.ansi[9], "ansi9", m);
    put(0, 2, "warning:", t.ansi[3], "ansi3", m);
    put(9, 2, "1 file changed", t.fg, "fg", m);
    put(25, 2, "(dim)", t.ansi[8], "ansi8", m);
    // Auswahl über einem Wort, Text bleibt in Vordergrundfarbe.
    let sel = Rect::from_min_size(origin + Vec2::new(17.0 * cw, 0.0), Vec2::new(10.0 * cw, lh));
    p.rect_filled(sel, 0.0, t.selection);
    p.text(sel.min, egui::Align2::LEFT_TOP, "git status", font.clone(), t.fg);
    mark(m, sel, "selection");
    // 16 Farben als Blöcke mit Nummer.
    for i in 0..16 {
        let x = origin.x + (i % 8) as f32 * 4.0 * cw;
        let y = origin.y + (3 + i / 8) as f32 * lh;
        let r = Rect::from_min_size(Pos2::new(x, y + 2.0), Vec2::new(3.4 * cw, lh - 4.0));
        p.rect_filled(r, 2.0, t.ansi[i]);
        p.text(Pos2::new(x + 0.3 * cw, y + 1.0), egui::Align2::LEFT_TOP, format!("{i:>2}"), font.clone(), if crate::theme::color::luminance(t.ansi[i]) > 0.35 { Color32::BLACK } else { Color32::WHITE });
        mark(m, r, ANSI_NAMES[i]);
    }
    // Cursor als Block am Ende der letzten Zeile.
    let row = 5;
    put(0, row, "PS C:\\paneldeck>", t.ansi[2], "ansi2", m);
    let cur = Rect::from_min_size(origin + Vec2::new(17.0 * cw, row as f32 * lh), Vec2::new(cw, lh));
    p.rect_filled(cur, 0.0, t.cursor);
    mark(m, cur, "cursor");
}

const ANSI_NAMES: [&str; 16] = ["ansi0", "ansi1", "ansi2", "ansi3", "ansi4", "ansi5", "ansi6", "ansi7", "ansi8", "ansi9", "ansi10", "ansi11", "ansi12", "ansi13", "ansi14", "ansi15"];

fn diff(ui: &mut Ui, u: &UiColors, m: &mut Marks) {
    let r = egui::Frame::new().fill(u.code_bg).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_width(330.0);
        for (letter, file, c, tok) in [("A", "src/theme/tokens.rs", u.diff_add, "diff_add"), ("M", "src/style.rs", u.diff_change, "diff_change"), ("D", "src/old.rs", u.diff_remove, "diff_remove"), ("R", "docs/themes.md", u.diff_rename, "diff_rename")] {
            ui.horizontal(|ui| {
                let r = ui.label(RichText::new(letter).monospace().color(c));
                mark(m, r.rect, tok);
                ui.label(RichText::new(file).monospace());
            });
        }
        ui.add_space(6.0);
        for (line, c, tok) in [("@@ -12,3 +12,4 @@", u.diff_rename, "diff_rename"), ("-    let accent = ACCENT;", u.diff_remove, "diff_remove"), ("+    let accent = style::brand();", u.diff_add, "diff_add"), ("     apply(ctx, &theme);", u.text_dim, "text_dim")] {
            let r = ui.label(RichText::new(line).monospace().color(c));
            mark(m, r.rect, tok);
        }
    });
    mark(m, r.response.rect, "code_bg");
}

fn code(ui: &mut Ui, u: &UiColors, m: &mut Marks) {
    let r = egui::Frame::new().fill(u.input_bg).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let parts: &[&[(&str, Color32, &'static str)]] = &[
            &[("project", u.syntax_field, "syntax_field"), (" = ", u.syntax_operator, "syntax_operator"), ("PAN", u.syntax_value, "syntax_value"), (" AND ", u.syntax_keyword, "syntax_keyword"), ("status", u.syntax_field, "syntax_field"), (" != ", u.syntax_operator, "syntax_operator"), ("\"Done\"", u.syntax_value, "syntax_value")],
            &[("AND ", u.syntax_keyword, "syntax_keyword"), ("due", u.syntax_field, "syntax_field"), (" < ", u.syntax_operator, "syntax_operator"), ("endOfWeek()", u.syntax_function, "syntax_function"), (" AND ", u.syntax_keyword, "syntax_keyword"), ("labels ", u.syntax_field, "syntax_field"), ("is EMPTY", u.syntax_predicate, "syntax_predicate")],
            &[("ORDER BY ", u.syntax_keyword, "syntax_keyword"), ("priority DESC", u.syntax_sort, "syntax_sort")],
        ];
        for line in parts {
            ui.horizontal(|ui| {
                for (txt, c, tok) in line.iter() {
                    let r = ui.label(RichText::new(*txt).monospace().color(*c));
                    mark(m, r.rect, tok);
                }
            });
        }
    });
    mark(m, r.response.rect, "input_bg");
}

/// Probe des Editors im Syntax-Modus (#312): jede Farbe `code_*` einmal, auf dem Hintergrund des Editors.
fn editor_code(ui: &mut Ui, t: &Theme, u: &UiColors, m: &mut Marks) {
    let fg = t.fg;
    let r = egui::Frame::new().fill(t.bg).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let lines: &[&[(&str, Color32, &'static str)]] = &[
            &[("#include", u.code_preprocessor, "code_preprocessor"), (" ", fg, "fg"), ("<config.h>", u.code_string, "code_string")],
            &[("// ", u.code_comment, "code_comment"), ("Konfiguration lesen / read the config", u.code_comment, "code_comment")],
            &[("fn ", u.code_keyword, "code_keyword"), ("parse", u.code_function, "code_function"), ("(", fg, "fg"), ("path", u.code_variable, "code_variable"), (": &", u.code_operator, "code_operator"), ("Path", u.code_type, "code_type"), (") {", fg, "fg")],
            &[("    let ", u.code_keyword, "code_keyword"), ("n", fg, "fg"), (" = ", u.code_operator, "code_operator"), ("42", u.code_number, "code_number"), (" + ", u.code_operator, "code_operator"), ("MAX", u.code_constant, "code_constant"), (";", fg, "fg")],
            &[("    print(", fg, "fg"), ("\"ok", u.code_string, "code_string"), ("\\n", u.code_escape, "code_escape"), ("\"", u.code_string, "code_string"), (");  ", fg, "fg"), ("@@@", u.code_invalid, "code_invalid")],
            &[("}", fg, "fg")],
            &[("<div ", u.code_tag, "code_tag"), ("class", u.code_attribute, "code_attribute"), ("=", fg, "fg"), ("\"note\"", u.code_string, "code_string"), (">", u.code_tag, "code_tag")],
            &[("[Setup]", u.code_heading, "code_heading"), ("  ", fg, "fg"), ("## Kapitel / chapter", u.code_heading, "code_heading")],
        ];
        for line in lines {
            ui.horizontal(|ui| {
                for (txt, c, tok) in line.iter() {
                    let r = ui.label(RichText::new(*txt).monospace().color(*c));
                    mark(m, r.rect, tok);
                }
            });
        }
    });
    mark(m, r.response.rect, "bg");
}

/// Die kleinste markierte Fläche unter `pos` (Klick in der Vorschau).
pub(super) fn hit(m: &Marks, pos: Pos2) -> Option<&'static str> {
    m.iter().filter(|(r, _)| r.contains(pos)).min_by(|a, b| a.0.area().total_cmp(&b.0.area())).map(|(_, t)| *t)
}

/// Elemente der Farben `toks` hervorheben (Rahmen in Akzent- und Kontrastfarbe).
pub(super) fn highlight(ui: &Ui, m: &Marks, toks: &[&'static str], clip: Rect) {
    let p = ui.painter().with_clip_rect(clip);
    let t = ui.ctx().input(|i| i.time);
    let pulse = (0.65 + 0.35 * (t * 4.0).sin()) as f32;
    for (r, tok) in m {
        if toks.contains(tok) && r.area() < clip.area() * 0.5 {
            p.rect_stroke(r.expand(3.0), CornerRadius::same(4), Stroke::new(3.0_f32, Color32::BLACK.gamma_multiply(0.6)), StrokeKind::Outside);
            p.rect_stroke(r.expand(3.0), CornerRadius::same(4), Stroke::new(2.0_f32, Color32::from_rgb(0xff, 0x3d, 0xd6).gamma_multiply(pulse)), StrokeKind::Outside);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_prefers_the_smallest_area() {
        let big = Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0));
        let small = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(10.0));
        let m: Marks = vec![(big, "window_bg"), (small, "accent")];
        assert_eq!(hit(&m, Pos2::new(15.0, 15.0)), Some("accent"));
        assert_eq!(hit(&m, Pos2::new(50.0, 50.0)), Some("window_bg"));
        assert_eq!(hit(&m, Pos2::new(500.0, 50.0)), None);
    }

    #[test]
    fn every_marked_token_exists_and_the_page_draws_in_both_languages() {
        for lang in [Lang::En, Lang::De] {
            for t in crate::theme::all() {
                let ctx = egui::Context::default();
                let u = t.ui_colors();
                let mut marks = Marks::new();
                let mut s = Sample::default();
                let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 2000.0))), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        style::with_preview(u, || page(ui, &t, &u, lang, &mut s, &mut marks));
                    });
                });
                assert!(marks.len() > 60, "{}", marks.len());
                for (_, tok) in &marks {
                    assert!(crate::theme::tokens::token(tok).is_some(), "{tok}");
                }
                // Fast jede Oberflächenfarbe kommt in der Vorschau vor.
                let missing: Vec<&str> = crate::theme::tokens::UI_TOKENS.iter().map(|t| t.name).filter(|n| !marks.iter().any(|(_, m)| m == n)).collect();
                assert!(missing.len() <= 6, "nicht in der Vorschau: {missing:?}");
            }
        }
    }
}

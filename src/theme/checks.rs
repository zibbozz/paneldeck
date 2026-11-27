//! Design-Prüfung im Theme-Editor (#309): Hinweise, die nie blockieren. Jede Regel ist abschaltbar (Standard an),
//! jeder Hinweis nennt die beteiligten Farben und – wenn möglich – einen Vorschlag mit der kleinsten Änderung
//! (Helligkeit, Farbton bleibt), den der Editor mit „Übernehmen“ setzt.

use super::color::{chroma, contrast, delta_e, lab, luminance, nudge_lightness, over, simulate, to_hex, Cvd};
use super::file::Msg;
use super::tokens::{resolve, token};
use super::Theme;
use crate::state::Lang;
use eframe::egui::Color32;

/// Regelgruppe (in den Einstellungen des Editors einzeln abschaltbar).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Rule {
    /// WCAG-Kontrast Text/Hintergrund (4,5:1, große Elemente/Symbole/Rahmen 3:1).
    Contrast,
    /// Zustände unterscheidbar (Hover, aktiv, Auswahl, Fokus).
    Distinct,
    /// Statusfarben unterscheidbar, auch bei Farbsehschwächen.
    Status,
    /// Terminal-Palette lesbar.
    Terminal,
    /// Harmonie und Stimmigkeit.
    Harmony,
    /// Lesbarkeit über dem eigenen Hintergrund (#387).
    Background,
}

impl Rule {
    pub const ALL: [Rule; 6] = [Rule::Contrast, Rule::Distinct, Rule::Status, Rule::Terminal, Rule::Harmony, Rule::Background];

    pub fn key(self) -> &'static str {
        match self {
            Rule::Contrast => "contrast",
            Rule::Distinct => "distinct",
            Rule::Status => "status",
            Rule::Terminal => "terminal",
            Rule::Harmony => "harmony",
            Rule::Background => "background",
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Rule::Contrast => lang.t("Kontrast (WCAG)", "Contrast (WCAG)"),
            Rule::Distinct => lang.t("Unterscheidbare Zustände", "Distinguishable states"),
            Rule::Status => lang.t("Statusfarben und Farbsehschwächen", "Status colours and colour blindness"),
            Rule::Terminal => lang.t("Terminal-Palette", "Terminal palette"),
            Rule::Harmony => lang.t("Harmonie und Stimmigkeit", "Harmony and consistency"),
            Rule::Background => lang.t("Hintergrund", "Background"),
        }
    }

    pub fn desc(self, lang: Lang) -> &'static str {
        match self {
            Rule::Contrast => lang.t("Text auf seinem Hintergrund mindestens 4,5:1, Symbole, Fokus und Balken 3:1.", "Text on its background at least 4.5:1, icons, focus and bars 3:1."),
            Rule::Distinct => lang.t("Hover, gedrückt, aktiv und Auswahl heben sich vom Normalzustand ab (ΔE).", "Hover, pressed, active and selection stand out from the normal state (ΔE)."),
            Rule::Status => lang.t("Erfolg/Warnung/Fehler/Info und Diff-Farben bleiben unterscheidbar – auch bei Rot-Grün- und Blau-Gelb-Schwäche.", "Success/warning/error/info and diff colours stay distinguishable – also with red-green and blue-yellow deficiencies."),
            Rule::Terminal => lang.t("ANSI-Farben lesbar, helle Varianten heller, Text in der Auswahl lesbar, Cursor sichtbar.", "ANSI colours readable, bright variants brighter, text readable in the selection, cursor visible."),
            Rule::Harmony => lang.t("Akzent hebt sich vom Text ab, nicht zu viele grelle Flächen, gleichmäßige Stufen Hintergrund → Fläche → Rahmen.", "Accent stands out from text, not too many strongly saturated areas, even steps background → surface → border."),
            Rule::Background => lang.t(
                "Text und Terminalschrift je Bereich gegen die Fläche über dem Bild: mindestens 4,5:1 gegen das Mittel, 3:1 gegen die hellsten bzw. dunkelsten 10 %.",
                "Text and terminal font per area against the surface over the image: at least 4.5:1 against the average, 3:1 against the brightest or darkest 10 %.",
            ),
        }
    }
}

/// Vorschlag: Farbe `token` auf `color` setzen.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub token: &'static str,
    pub color: Color32,
}

/// Vorschlag der Regel „Hintergrund“: Helligkeit bzw. Deckkraft der Überlagerung setzen (nichts ändert sich von selbst).
#[derive(Clone, Debug, PartialEq)]
pub struct BgFix {
    pub brightness: Option<i8>,
    pub overlay_opacity: Option<u8>,
}

/// Ein Hinweis der Design-Prüfung.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub rule: Rule,
    /// Beteiligte Farben (erste = die, die der Vorschlag ändert).
    pub tokens: Vec<&'static str>,
    pub msg: Msg,
    pub fix: Option<Fix>,
    /// Vorschlag für den Hintergrund (nur Regel „Hintergrund“).
    pub bg_fix: Option<BgFix>,
}

/// Was die Regel „Hintergrund“ braucht: Kennzahlen des Bildes (vor Helligkeit/Überlagerung) und die globale Deckkraft.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BgInput {
    pub stats: super::bake::Stats,
    pub global: f32,
}

/// Mindestkontrast für Text (WCAG AA).
pub const TEXT_MIN: f64 = 4.5;
/// Mindestkontrast für große Elemente, Symbole, Rahmen (WCAG AA).
pub const LARGE_MIN: f64 = 3.0;
/// Mindestabstand zweier Zustände (ΔE 2000), darunter wirken sie fast gleich.
pub const STATE_MIN_DE: f64 = 2.5;
/// Mindestabstand zweier Statusfarben (normal).
pub const STATUS_MIN_DE: f64 = 12.0;
/// Mindestabstand zweier Statusfarben in der Simulation einer Farbsehschwäche.
pub const CVD_MIN_DE: f64 = 6.0;
/// Akzent gegen Text: darunter kaum zu unterscheiden.
pub const ACCENT_TEXT_MIN_DE: f64 = 15.0;
/// Buntheit (CIELAB-Chroma), ab der eine Fläche als grell zählt.
pub const LOUD_CHROMA: f64 = 40.0;

/// Deckende Farbe eines Namens: durchscheinende Farben über ihrem Untergrund.
fn solid(t: &Theme, name: &str) -> Color32 {
    let c = resolve(t, name);
    if c.a() == 255 {
        return c;
    }
    let base = match name {
        "window_bg" | "sidebar_bg" | "surface" | "input_bg" | "code_bg" => resolve(t, "bg").to_opaque(),
        "sidebar_active" => solid(t, "sidebar_bg"),
        "selection_bg" | "text_dim" => solid(t, "input_bg"),
        _ => solid(t, "window_bg"),
    };
    over(c, base)
}

fn label(name: &str, lang: Lang) -> String {
    match token(name) {
        Some(tk) => format!("{} ({})", lang.t(tk.de, tk.en), name),
        None => name.to_string(),
    }
}

fn names(list: &[&str]) -> (String, String) {
    let de: Vec<String> = list.iter().map(|n| label(n, Lang::De)).collect();
    let en: Vec<String> = list.iter().map(|n| label(n, Lang::En)).collect();
    (de.join(" / "), en.join(" / "))
}

/// Kandidat einsetzen: Theme-Kopie mit `token` = `c`.
fn with(t: &Theme, tok: &str, c: Color32) -> Theme {
    let mut x = t.clone();
    super::set_color(&mut x, tok, c);
    x
}

struct Ctx<'a> {
    t: &'a Theme,
    out: Vec<Hint>,
}

impl Ctx<'_> {
    fn contrast_pair(&mut self, rule: Rule, fg: &'static str, bg: &'static str, min: f64) {
        let k = contrast(solid(self.t, fg), solid(self.t, bg));
        if k >= min {
            return;
        }
        let t = self.t;
        let fix = nudge_lightness(resolve(t, fg), |c| contrast(solid(&with(t, fg, c), fg), solid(t, bg)) >= min).map(|color| Fix { token: fg, color });
        let (d, e) = names(&[fg, bg]);
        self.out.push(Hint {
            rule,
            tokens: vec![fg, bg],
            msg: Msg::new(format!("Kontrast {:.2}:1 unter {min}:1 – {d}", (k * 100.0).floor() / 100.0), format!("Contrast {:.2}:1 is below {min}:1 – {e}", (k * 100.0).floor() / 100.0)),
            fix,
            bg_fix: None,
        });
    }

    fn distinct_pair(&mut self, a: &'static str, b: &'static str, what_de: &str, what_en: &str) {
        let d = delta_e(solid(self.t, a), solid(self.t, b));
        if d >= STATE_MIN_DE {
            return;
        }
        let t = self.t;
        let fix = nudge_lightness(resolve(t, a), |c| delta_e(solid(&with(t, a, c), a), solid(t, b)) >= STATE_MIN_DE).map(|color| Fix { token: a, color });
        let (nd, ne) = names(&[a, b]);
        self.out.push(Hint {
            rule: Rule::Distinct,
            tokens: vec![a, b],
            msg: Msg::new(format!("{what_de} sehen fast gleich aus (ΔE {d:.1}) – {nd}"), format!("{what_en} look almost the same (ΔE {d:.1}) – {ne}")),
            fix,
            bg_fix: None,
        });
    }

    /// Kleinster Abstand zweier Farben normal und in allen Simulationen (mit der schlechtesten Sicht).
    fn worst(a: Color32, b: Color32) -> (f64, Option<Cvd>) {
        let mut w = (delta_e(a, b) * CVD_MIN_DE / STATUS_MIN_DE, None);
        for k in Cvd::ALL {
            let d = delta_e(simulate(a, k), simulate(b, k));
            if d < w.0 {
                w = (d, Some(k));
            }
        }
        w
    }

    fn status_set(&mut self, set: &[&'static str]) {
        let ok = |a: Color32, b: Color32| delta_e(a, b) >= STATUS_MIN_DE && Cvd::ALL.iter().all(|&k| delta_e(simulate(a, k), simulate(b, k)) >= CVD_MIN_DE);
        for i in 0..set.len() {
            for j in i + 1..set.len() {
                let (a, b) = (set[i], set[j]);
                let (ca, cb) = (solid(self.t, a), solid(self.t, b));
                if ok(ca, cb) {
                    continue;
                }
                let normal = delta_e(ca, cb);
                let t = self.t;
                // Die zweite Farbe verschieben (Helligkeit bleibt bei allen Farbsehschwächen erhalten); die anderen
                // Farben der Gruppe dürfen dabei nicht schlechter werden.
                let others: Vec<Color32> = set.iter().filter(|n| **n != b).map(|n| solid(t, n)).collect();
                let cand = |all: bool| {
                    nudge_lightness(resolve(t, b), |c| {
                        let cs = solid(&with(t, b, c), b);
                        ok(ca, cs) && (!all || others.iter().all(|&o| ok(o, cs)))
                    })
                };
                let fix = cand(true).or_else(|| cand(false)).map(|color| Fix { token: b, color });
                let (nd, ne) = names(&[a, b]);
                let msg = if normal < STATUS_MIN_DE {
                    Msg::new(format!("Statusfarben zu ähnlich (ΔE {normal:.1}) – {nd}"), format!("Status colours too similar (ΔE {normal:.1}) – {ne}"))
                } else {
                    let (d, k) = Self::worst(ca, cb);
                    let k = k.unwrap_or(Cvd::Deutan);
                    Msg::new(
                        format!("Bei {} kaum unterscheidbar (ΔE {d:.1}) – {nd}", k.label(Lang::De)),
                        format!("Hard to tell apart with {} (ΔE {d:.1}) – {ne}", k.label(Lang::En)),
                    )
                };
                self.out.push(Hint { rule: Rule::Status, tokens: vec![b, a], msg, fix, bg_fix: None });
            }
        }
    }
}

const ANSI: [&str; 16] = ["ansi0", "ansi1", "ansi2", "ansi3", "ansi4", "ansi5", "ansi6", "ansi7", "ansi8", "ansi9", "ansi10", "ansi11", "ansi12", "ansi13", "ansi14", "ansi15"];

/// Alle Hinweise der eingeschalteten Regeln (ohne Regel „Hintergrund“, siehe [`run_with`]). Nur in Tests; die
/// Oberfläche nimmt [`run_with`].
#[cfg(test)]
pub fn run(t: &Theme, on: impl Fn(Rule) -> bool) -> Vec<Hint> {
    run_with(t, None, on)
}

/// Alle Hinweise der eingeschalteten Regeln; mit `bg` (Kennzahlen des Hintergrunds) auch die Regel „Hintergrund“.
pub fn run_with(t: &Theme, bg: Option<&BgInput>, on: impl Fn(Rule) -> bool) -> Vec<Hint> {
    let mut c = Ctx { t, out: Vec::new() };
    if on(Rule::Background) {
        if let Some(i) = bg {
            background_rule(&mut c, i);
        }
    }
    if on(Rule::Contrast) {
        for (fg, bg, min) in [
            ("text", "window_bg", TEXT_MIN),
            ("text", "sidebar_bg", TEXT_MIN),
            ("text", "surface", TEXT_MIN),
            ("text", "input_bg", TEXT_MIN),
            ("text", "widget_bg", TEXT_MIN),
            ("text_dim", "window_bg", TEXT_MIN),
            ("text_dim", "sidebar_bg", TEXT_MIN),
            ("text_strong", "active_bg", TEXT_MIN),
            ("text_strong", "hover_bg", TEXT_MIN),
            ("text", "selection_bg", TEXT_MIN),
            ("link", "window_bg", TEXT_MIN),
            ("success", "window_bg", TEXT_MIN),
            ("warning", "window_bg", TEXT_MIN),
            ("error", "window_bg", TEXT_MIN),
            ("diff_add", "sidebar_bg", TEXT_MIN),
            ("diff_remove", "sidebar_bg", TEXT_MIN),
            ("diff_change", "sidebar_bg", TEXT_MIN),
            ("diff_rename", "sidebar_bg", TEXT_MIN),
            ("syntax_field", "input_bg", TEXT_MIN),
            ("syntax_operator", "input_bg", TEXT_MIN),
            ("syntax_value", "input_bg", TEXT_MIN),
            ("syntax_keyword", "input_bg", TEXT_MIN),
            ("syntax_function", "input_bg", TEXT_MIN),
            ("syntax_predicate", "input_bg", TEXT_MIN),
            ("syntax_sort", "input_bg", TEXT_MIN),
            // Syntaxfarben des Editors (#312) auf dem Hintergrund des Editors; Kommentare dürfen gedämpft sein.
            ("code_comment", "bg", LARGE_MIN),
            ("code_string", "bg", TEXT_MIN),
            ("code_number", "bg", TEXT_MIN),
            ("code_constant", "bg", TEXT_MIN),
            ("code_keyword", "bg", TEXT_MIN),
            ("code_type", "bg", TEXT_MIN),
            ("code_function", "bg", TEXT_MIN),
            ("code_operator", "bg", TEXT_MIN),
            ("code_tag", "bg", TEXT_MIN),
            ("code_attribute", "bg", TEXT_MIN),
            ("code_preprocessor", "bg", TEXT_MIN),
            ("code_escape", "bg", TEXT_MIN),
            ("code_heading", "bg", TEXT_MIN),
            ("code_variable", "bg", TEXT_MIN),
            // Vergleich und Änderungsmarken (#385): Text auf den Zeilen-Hintergründen, Marken auf dem Editor-Hintergrund.
            ("text", "diff_add_bg", TEXT_MIN),
            ("text", "diff_remove_bg", TEXT_MIN),
            ("marker_added", "bg", LARGE_MIN),
            ("marker_modified", "bg", LARGE_MIN),
            ("marker_deleted", "bg", LARGE_MIN),
            ("marker_staged_added", "bg", LARGE_MIN),
            ("marker_staged_modified", "bg", LARGE_MIN),
            ("marker_staged_deleted", "bg", LARGE_MIN),
            ("accent", "window_bg", LARGE_MIN),
            ("danger", "window_bg", LARGE_MIN),
            ("info", "window_bg", LARGE_MIN),
            ("progress", "sidebar_bg", LARGE_MIN),
        ] {
            c.contrast_pair(Rule::Contrast, fg, bg, min);
        }
    }
    if on(Rule::Distinct) {
        c.distinct_pair("hover_bg", "widget_bg", "Hover und Normalzustand", "Hover and normal state");
        c.distinct_pair("active_bg", "widget_bg", "Gedrückt und Normalzustand", "Pressed and normal state");
        c.distinct_pair("tab_active", "window_bg", "Aktiver und inaktiver Tab", "Active and inactive tab");
        c.distinct_pair("sidebar_active", "sidebar_bg", "Aktive und inaktive Sitzung", "Active and inactive session");
        c.distinct_pair("selection_bg", "input_bg", "Auswahl und Hintergrund", "Selection and background");
        c.distinct_pair("chip_active", "window_bg", "Aktiver Chip und Hintergrund", "Active chip and background");
        c.contrast_pair(Rule::Distinct, "focus", "input_bg", LARGE_MIN);
    }
    if on(Rule::Status) {
        c.status_set(&["success", "warning", "error", "info"]);
        c.status_set(&["diff_add", "diff_remove", "diff_change"]);
    }
    if on(Rule::Terminal) {
        c.contrast_pair(Rule::Terminal, "fg", "bg", TEXT_MIN);
        c.contrast_pair(Rule::Terminal, "fg", "selection", TEXT_MIN);
        c.contrast_pair(Rule::Terminal, "cursor", "bg", LARGE_MIN);
        // Die „Hintergrundfarbe“ der Palette (Schwarz im dunklen, Weiß im hellen Theme) darf verschwinden.
        let skip: &[usize] = if t.dark { &[0] } else { &[7, 15] };
        for (i, n) in ANSI.iter().enumerate() {
            if !skip.contains(&i) {
                c.contrast_pair(Rule::Terminal, n, "bg", LARGE_MIN);
            }
        }
        let not_brighter: Vec<usize> = (0..8).filter(|&i| luminance(t.ansi[i + 8]) <= luminance(t.ansi[i])).collect();
        if let Some(&first) = not_brighter.first() {
            let (b, n) = (ANSI[first + 8], ANSI[first]);
            let base = luminance(t.ansi[first]);
            let fix = nudge_lightness(t.ansi[first + 8], |x| luminance(x) > base * 1.15 + 0.01).map(|color| Fix { token: b, color });
            let list: Vec<String> = not_brighter.iter().map(|i| format!("{} ≤ {}", i + 8, i)).collect();
            c.out.push(Hint {
                rule: Rule::Terminal,
                tokens: vec![b, n],
                msg: Msg::new(format!("Helle Varianten nicht heller als normal: ANSI {}", list.join(", ")), format!("Bright variants not brighter than normal: ANSI {}", list.join(", "))),
                fix,
                bg_fix: None,
            });
        }
        let sel = delta_e(t.selection, resolve(t, "bg").to_opaque());
        if sel < STATE_MIN_DE {
            let bg = resolve(t, "bg").to_opaque();
            let fix = nudge_lightness(t.selection, |x| delta_e(x, bg) >= STATE_MIN_DE && contrast(t.fg, x) >= contrast(t.fg, t.selection).min(TEXT_MIN)).map(|color| Fix { token: "selection", color });
            let (d, e) = names(&["selection", "bg"]);
            c.out.push(Hint { rule: Rule::Terminal, tokens: vec!["selection", "bg"], msg: Msg::new(format!("Auswahl kaum sichtbar (ΔE {sel:.1}) – {d}"), format!("Selection barely visible (ΔE {sel:.1}) – {e}")), fix, bg_fix: None });
        }
    }
    if on(Rule::Harmony) {
        let (a, x) = (solid(t, "accent"), solid(t, "text"));
        let d = delta_e(a, x);
        if d < ACCENT_TEXT_MIN_DE {
            let fix = nudge_lightness(resolve(t, "accent"), |c| delta_e(c.to_opaque(), x) >= ACCENT_TEXT_MIN_DE).map(|color| Fix { token: "accent", color });
            let (nd, ne) = names(&["accent", "text"]);
            c.out.push(Hint { rule: Rule::Harmony, tokens: vec!["accent", "text"], msg: Msg::new(format!("Akzent kaum vom Text zu unterscheiden (ΔE {d:.1}) – {nd}"), format!("Accent barely distinct from text (ΔE {d:.1}) – {ne}")), fix, bg_fix: None });
        }
        let areas = ["window_bg", "surface", "sidebar_bg", "input_bg", "widget_bg", "hover_bg", "active_bg", "selection_bg", "sidebar_active", "tab_active", "chip_active", "chat_user_bg"];
        let loud: Vec<&'static str> = areas.iter().copied().filter(|n| chroma(solid(t, n)) > LOUD_CHROMA).collect();
        if loud.len() >= 3 {
            let (nd, ne) = names(&loud);
            c.out.push(Hint {
                rule: Rule::Harmony,
                tokens: loud.clone(),
                msg: Msg::new(format!("{} stark gesättigte Flächen – das wirkt unruhig: {nd}", loud.len()), format!("{} strongly saturated areas – this looks busy: {ne}", loud.len())),
                fix: None,
                bg_fix: None,
            });
        }
        // Stufen: Hintergrund → abgesetzte Fläche → Rahmen sollen in derselben Richtung zunehmen.
        let l = |n: &str| lab(solid(t, n))[0];
        let (base, surf, border) = (l("window_bg"), l("surface"), l("border"));
        let (s1, s2) = (surf - base, border - base);
        if s2.abs() < s1.abs() || (s1.abs() > 0.5 && s1.signum() != s2.signum()) {
            let target = s1.abs().max(2.0) * 1.5 * if s1.abs() > 0.5 { s1.signum() } else if t.dark { 1.0 } else { -1.0 };
            let b = resolve(t, "border");
            let fix = Some(Fix { token: "border", color: super::color::with_lightness(b, base + target) }).filter(|f| f.color != b);
            let (nd, ne) = names(&["border", "surface", "window_bg"]);
            c.out.push(Hint {
                rule: Rule::Harmony,
                tokens: vec!["border", "surface", "window_bg"],
                msg: Msg::new(
                    format!("Ungleichmäßige Stufen: Rahmen hebt sich weniger ab als die Fläche (ΔL {:.1} gegen {:.1}) – {nd}", s2.abs(), s1.abs()),
                    format!("Uneven steps: the border stands out less than the surface (ΔL {:.1} vs. {:.1}) – {ne}", s2.abs(), s1.abs()),
                ),
                fix,
                bg_fix: None,
            });
        }
    }
    c.out
}

/// Bereiche der Regel „Hintergrund“: Bereich, Schrift, Fläche.
const BG_AREAS: [(super::background::Area, &str, &str); 5] = {
    use super::background::Area::*;
    [(Terminals, "fg", "bg"), (Editor, "fg", "bg"), (Todo, "text", "bg"), (Chat, "text", "bg"), (Sidebar, "text", "sidebar_bg")]
};

/// Kontrast der Schrift `text` auf der Fläche `surface` (Deckkraft `alpha`) über dem Hintergrund mit den Kennzahlen `s`
/// (nach Helligkeit/Überlagerung): (gegen das Mittel, gegen die gefährlichsten 10 %).
pub fn bg_contrast(text: Color32, surface: Color32, alpha: f32, s: &super::bake::Stats) -> (f64, f64) {
    let [r, g, b, _] = surface.to_srgba_unmultiplied();
    let top = Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    // Heller Text ist über hellen Stellen gefährdet, dunkler über dunklen.
    let extreme = if luminance(text) >= luminance(surface) { s.bright } else { s.dark };
    (contrast(text, over(top, s.mean)), contrast(text, over(top, extreme)))
}

fn bg_ok(k: (f64, f64)) -> bool {
    k.0 >= TEXT_MIN && k.1 >= LARGE_MIN
}

fn background_rule(c: &mut Ctx, i: &BgInput) {
    let t = c.t;
    let Some(b) = t.background.as_ref() else { return };
    let overlay = super::bake::overlay_of(t);
    let now = i.stats.after(b.brightness, overlay);
    for (area, text, surf) in BG_AREAS {
        if !b.area(area).on {
            continue;
        }
        let alpha = b.surface_alpha(area, i.global);
        let (tc, sc) = (solid(t, text), solid(t, surf));
        let k = bg_contrast(tc, sc, alpha, &now);
        if bg_ok(k) {
            continue;
        }
        // Vorschlag: mit Überlagerung deren Deckkraft erhöhen, sonst die Helligkeit (abdunkeln für helle Schrift).
        let fix = match (&b.overlay, overlay) {
            (Some(o), Some((oc, _))) => ((o.opacity / 5 * 5 + 5)..=100)
                .step_by(5)
                .find(|&op| bg_ok(bg_contrast(tc, sc, alpha, &i.stats.after(b.brightness, Some((oc, op))))))
                .map(|op| BgFix { brightness: None, overlay_opacity: Some(op) }),
            _ => {
                let dir: i32 = if luminance(tc) >= luminance(sc) { -5 } else { 5 };
                (1..=40)
                    .map(|n| (b.brightness as i32 + dir * n).clamp(-100, 100) as i8)
                    .find(|&br| bg_ok(bg_contrast(tc, sc, alpha, &i.stats.after(br, overlay))))
                    .map(|br| BgFix { brightness: Some(br), overlay_opacity: None })
            }
        };
        let (de_a, en_a) = (area.label(Lang::De), area.label(Lang::En));
        let low_mean = k.0 < TEXT_MIN;
        let (kk, min) = if low_mean { (k.0, TEXT_MIN) } else { (k.1, LARGE_MIN) };
        let kk = (kk * 100.0).floor() / 100.0;
        let msg = if low_mean {
            Msg::new(format!("Bereich „{de_a}“: Kontrast {kk:.2}:1 gegen das Bild (Mittel) unter {min}:1"), format!("Area “{en_a}”: contrast {kk:.2}:1 against the image (average) is below {min}:1"))
        } else {
            let (wd, we) = if luminance(tc) >= luminance(sc) { ("hellsten", "brightest") } else { ("dunkelsten", "darkest") };
            Msg::new(format!("Bereich „{de_a}“: Kontrast {kk:.2}:1 gegen die {wd} 10 % des Bildes unter {min}:1"), format!("Area “{en_a}”: contrast {kk:.2}:1 against the {we} 10 % of the image is below {min}:1"))
        };
        c.out.push(Hint { rule: Rule::Background, tokens: vec![text, surf], msg, fix: None, bg_fix: fix });
    }
}

/// Kurztext eines Vorschlags der Regel „Hintergrund“.
pub fn bg_fix_text(f: &BgFix, lang: Lang) -> String {
    match (f.brightness, f.overlay_opacity) {
        (Some(b), _) => format!("{} → {b:+} %", lang.t("Helligkeit", "Brightness")),
        (_, Some(o)) => format!("{} → {o} %", lang.t("Überlagerung", "Overlay")),
        _ => String::new(),
    }
}

/// Kurztext eines Vorschlags („border → #3a3f4b“).
pub fn fix_text(f: &Fix) -> String {
    format!("{} → {}", f.token, to_hex(f.color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::tokens::Spec;

    fn applied(t: &Theme, f: &Fix) -> Theme {
        with(t, f.token, f.color)
    }

    #[test]
    fn builtin_themes_have_few_hints() {
        // Eingebaute Themes: Hinweise sind erlaubt (sie blockieren nie), aber keine Flut. Das helle Theme nutzt die
        // Statusfarben des dunklen Standards und bekommt deshalb viele Kontrast-Hinweise.
        for t in crate::theme::all() {
            let hints = run(&t, |_| true);
            for h in &hints {
                eprintln!("{}: [{:?}] {}", t.name, h.rule, h.msg.en);
            }
            assert!(hints.len() <= 32, "{}: {} Hinweise", t.name, hints.len());
            assert!(hints.iter().all(|h| !h.tokens.is_empty() && !h.msg.de.is_empty() && !h.msg.en.is_empty()));
        }
    }

    #[test]
    fn builtin_themes_have_readable_syntax_colours() {
        for t in crate::theme::all() {
            let bad: Vec<String> = run(&t, |r| r == Rule::Contrast).into_iter().filter(|h| h.tokens.iter().any(|n| n.starts_with("code_"))).map(|h| h.msg.en).collect();
            assert!(bad.is_empty(), "{}: {bad:?}", t.name);
        }
    }

    #[test]
    fn builtin_themes_have_visible_change_markers() {
        for t in crate::theme::all() {
            let bad: Vec<String> = run(&t, |r| r == Rule::Contrast).into_iter().filter(|h| h.tokens.iter().any(|n| n.starts_with("marker_"))).map(|h| h.msg.en).collect();
            assert!(bad.is_empty(), "{}: {bad:?}", t.name);
        }
        // Gestagte und nicht gestagte Marken sind verschiedene Farben.
        let u = crate::theme::tokens::ui_colors(&crate::theme::all()[0]);
        assert_ne!(u.marker_added, u.marker_staged_added);
        assert_ne!(u.marker_modified, u.marker_staged_modified);
        assert_ne!(u.marker_deleted, u.marker_staged_deleted);
    }

    #[test]
    fn rules_can_be_switched_off() {
        let mut t = crate::theme::all()[0].clone();
        t.ui.insert("text".into(), Spec::Link { to: "window_bg".into(), alpha: None });
        assert!(run(&t, |r| r == Rule::Contrast).iter().any(|h| h.tokens == ["text", "window_bg"]));
        assert!(run(&t, |_| false).is_empty());
        assert!(run(&t, |r| r != Rule::Contrast).iter().all(|h| h.rule != Rule::Contrast));
    }

    #[test]
    fn contrast_fix_satisfies_the_rule_with_a_small_change() {
        let mut t = crate::theme::all()[0].clone();
        let bg = resolve(&t, "window_bg");
        t.ui.insert("text".into(), Spec::Hex(crate::style::mix(bg, Color32::WHITE, 0.15)));
        let hints = run(&t, |r| r == Rule::Contrast);
        let h = hints.iter().find(|h| h.tokens == ["text", "window_bg"]).expect("Hinweis");
        assert!(h.msg.en.contains("below 4.5:1") && h.msg.de.contains("unter 4.5:1"), "{:?}", h.msg);
        let f = h.fix.clone().expect("Vorschlag");
        assert_eq!(f.token, "text");
        let fixed = applied(&t, &f);
        let k = contrast(solid(&fixed, "text"), solid(&fixed, "window_bg"));
        assert!((4.5..5.5).contains(&k), "knapp erfüllt: {k}");
        assert!(!run(&fixed, |r| r == Rule::Contrast).iter().any(|h| h.tokens == ["text", "window_bg"]));
    }

    #[test]
    fn states_must_differ() {
        let mut t = crate::theme::all()[0].clone();
        t.ui.insert("hover_bg".into(), Spec::Link { to: "widget_bg".into(), alpha: None });
        let hints = run(&t, |r| r == Rule::Distinct);
        let h = hints.iter().find(|h| h.tokens == ["hover_bg", "widget_bg"]).expect("Hover = normal");
        let f = h.fix.clone().unwrap();
        let fixed = applied(&t, &f);
        assert!(delta_e(solid(&fixed, "hover_bg"), solid(&fixed, "widget_bg")) >= STATE_MIN_DE);
    }

    #[test]
    fn status_colours_are_checked_under_colour_blindness() {
        let mut t = crate::theme::all()[0].clone();
        // Rot und Grün gleicher Helligkeit: normal verschieden, bei Rot-Grün-Schwäche fast gleich.
        t.ui.insert("error".into(), Spec::Hex(Color32::from_rgb(0xc0, 0x50, 0x40)));
        t.ui.insert("success".into(), Spec::Hex(Color32::from_rgb(0x6a, 0x80, 0x30)));
        t.ui.insert("warning".into(), Spec::Hex(Color32::from_rgb(0xf0, 0xe0, 0x40)));
        t.ui.insert("info".into(), Spec::Hex(Color32::from_rgb(0x30, 0x70, 0xf0)));
        let hints = run(&t, |r| r == Rule::Status);
        let h = hints.iter().find(|h| h.tokens.contains(&"error") && h.tokens.contains(&"success")).expect("Rot/Grün");
        assert!(h.msg.en.contains("deficiency") || h.msg.en.contains("too similar"), "{}", h.msg.en);
        let f = h.fix.clone().expect("Vorschlag");
        let fixed = applied(&t, &f);
        let (a, b) = (solid(&fixed, "error"), solid(&fixed, "success"));
        for k in Cvd::ALL {
            assert!(delta_e(simulate(a, k), simulate(b, k)) >= CVD_MIN_DE, "{k:?}");
        }
    }

    #[test]
    fn terminal_palette_rules() {
        let mut t = crate::theme::all()[0].clone();
        t.ansi[9] = t.ansi[1];
        t.ansi[4] = Color32::from_rgb(0x22, 0x25, 0x2b);
        t.selection = t.bg.to_opaque();
        let hints = run(&t, |r| r == Rule::Terminal);
        assert!(hints.iter().any(|h| h.msg.en.contains("not brighter") && h.msg.en.contains("9 ≤ 1")), "{hints:?}");
        let h = hints.iter().find(|h| h.tokens == ["ansi4", "bg"]).expect("Blau unlesbar");
        assert!(applied(&t, h.fix.as_ref().unwrap()).ansi[4] != t.ansi[4]);
        assert!(hints.iter().any(|h| h.tokens == ["selection", "bg"]));
        // Schwarz im dunklen Theme ist erlaubt.
        assert!(!hints.iter().any(|h| h.tokens == ["ansi0", "bg"]));
    }

    #[test]
    fn background_rule_checks_each_area_and_suggests_a_fix() {
        use crate::theme::background::{Area, Background, Kind, Overlay};
        use crate::theme::bake::Stats;
        let mut t = crate::theme::all()[0].clone();
        let mut b = Background::new(Kind::Image);
        for a in Area::ALL {
            b.area_mut(a).opacity = 30;
        }
        t.background = Some(b);
        // Helles Bild unter hellem Text (dunkles Theme): Hinweise je Bereich, Vorschlag = abdunkeln.
        let bright = Stats { mean: Color32::from_gray(225), bright: Color32::from_gray(250), dark: Color32::from_gray(180) };
        let inp = BgInput { stats: bright, global: 1.0 };
        let hints = run_with(&t, Some(&inp), |r| r == Rule::Background);
        assert_eq!(hints.len(), 5, "{hints:?}");
        assert!(run(&t, |r| r == Rule::Background).is_empty(), "ohne Kennzahlen keine Prüfung");
        assert!(run_with(&t, Some(&inp), |r| r != Rule::Background).iter().all(|h| h.rule != Rule::Background), "abschaltbar");
        let h = hints.iter().find(|h| h.tokens == ["fg", "bg"]).unwrap();
        assert!(h.msg.en.contains("Terminals") && h.msg.de.contains("Terminals"));
        let f = h.bg_fix.clone().expect("Vorschlag");
        let br = f.brightness.expect("Helligkeit");
        assert!(br < 0, "abdunkeln: {br}");
        assert!(!bg_fix_text(&f, Lang::En).is_empty() && !bg_fix_text(&f, Lang::De).is_empty());
        // Vorschlag übernommen: Bereich „Terminals“ besteht.
        t.background.as_mut().unwrap().brightness = br;
        assert!(!run_with(&t, Some(&inp), |r| r == Rule::Background).iter().any(|h| h.tokens == ["fg", "bg"]));
        // Mit Überlagerung wird deren Deckkraft vorgeschlagen.
        t.background.as_mut().unwrap().brightness = 0;
        t.background.as_mut().unwrap().overlay = Some(Overlay { color: crate::theme::Spec::Hex(Color32::BLACK), opacity: 10 });
        let h = run_with(&t, Some(&inp), |r| r == Rule::Background).into_iter().find(|h| h.tokens == ["fg", "bg"]).unwrap();
        assert!(h.bg_fix.as_ref().and_then(|f| f.overlay_opacity).is_some_and(|o| o > 10));
        // Dunkles Bild: keine Hinweise; ausgeschalteter Bereich wird nicht geprüft.
        let dark = BgInput { stats: Stats { mean: Color32::from_gray(10), bright: Color32::from_gray(40), dark: Color32::from_gray(0) }, global: 1.0 };
        assert!(run_with(&t, Some(&dark), |r| r == Rule::Background).is_empty());
        t.background.as_mut().unwrap().overlay = None;
        for a in Area::ALL {
            t.background.as_mut().unwrap().area_mut(a).on = false;
        }
        assert!(run_with(&t, Some(&inp), |r| r == Rule::Background).is_empty());
        // Ein Bild mit wenigen hellen Stellen: das Mittel reicht, die hellsten 10 % nicht.
        let spots = BgInput { stats: Stats { mean: Color32::from_gray(20), bright: Color32::from_gray(255), dark: Color32::from_gray(0) }, global: 1.0 };
        let mut t2 = crate::theme::all()[0].clone();
        let mut b2 = Background::new(Kind::Image);
        b2.area_mut(Area::Terminals).opacity = 5;
        t2.background = Some(b2);
        let h = run_with(&t2, Some(&spots), |r| r == Rule::Background);
        assert!(h.iter().any(|h| h.tokens == ["fg", "bg"] && h.msg.en.contains("brightest 10 %")), "{h:?}");
    }

    #[test]
    fn harmony_rules() {
        let mut t = crate::theme::all()[0].clone();
        t.ui.insert("accent".into(), Spec::Link { to: "text".into(), alpha: None });
        for n in ["window_bg", "surface", "sidebar_bg", "input_bg"] {
            t.ui.insert(n.into(), Spec::Hex(Color32::from_rgb(0xd0, 0x10, 0x60)));
        }
        let hints = run(&t, |r| r == Rule::Harmony);
        assert!(hints.iter().any(|h| h.tokens == ["accent", "text"] && h.fix.is_some()), "{hints:?}");
        assert!(hints.iter().any(|h| h.msg.en.contains("strongly saturated")));
    }
}

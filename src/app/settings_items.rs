//! Einstellungsfenster (#395): jede Einstellung genau einmal beschrieben – Seite, Beschriftung und Erklärung (DE/EN),
//! gepflegte Stichwörter, Standard (geändert? zurücksetzen?) und ob das Zurücksetzen erst nachfragt. Suche, Marken für
//! geänderte Werte, „Nur geänderte“ und das Zurücksetzen lesen alle diese eine Tabelle; die Bedienelemente selbst zeichnen
//! die Seiten (`settings.rs`) mit [`srow`] bzw. [`block`] unter dem Schlüssel des Eintrags.

use super::settings_nav::{Page, Query};
use super::App;
use crate::state::{Lang, Settings};
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Align, Color32, FontId, Layout, Pos2, Rect, Sense, Ui, UiBuilder, Vec2};
use eframe::egui::text::{LayoutJob, TextFormat};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, OnceLock};

/// Weicht der Wert vom Standard ab? (`s` = aktuelle, `d` = Standard-Einstellungen)
pub(super) type ModFn = Arc<dyn Fn(&Settings, &Settings) -> bool + Send + Sync>;
/// Auf den Standard zurücksetzen.
pub(super) type ResetFn = Arc<dyn Fn(&mut Settings, &Settings) + Send + Sync>;

/// Standard einer Einstellung.
pub(super) enum Def {
    /// Kein Standard (Sprache, reine Knöpfe, Listen ohne festen Ausgangswert).
    None,
    /// Felder in `settings.json`; `reset` fehlt, wenn Zurücksetzen Daten löschen würde (Profile, Server).
    S { modified: ModFn, reset: Option<ResetFn> },
    /// Zustand außerhalb von `settings.json` (Autostart, Kontextmenü, globale Hooks, Regeln beim Speichern).
    A { modified: fn(&App) -> bool, reset: Option<fn(&mut App)> },
}

/// Eine Einstellung (Zeile) oder ein Abschnitt mit eigener Oberfläche (Block).
pub(super) struct Item {
    pub page: Page,
    pub key: &'static str,
    pub de: &'static str,
    pub en: &'static str,
    pub hint_de: &'static str,
    pub hint_en: &'static str,
    /// Gepflegte Stichwörter (beide Sprachen, alte Reiternamen, Fachbegriffe).
    pub kw: &'static [&'static str],
    pub def: Def,
    /// Zurücksetzen fragt erst nach (Integration, Sicherheit, „Sitzungen merken“, MCP-Stufe).
    pub risky: bool,
}

impl Item {
    pub(super) fn label(&self, lang: Lang) -> &'static str {
        lang.t(self.de, self.en)
    }

    pub(super) fn hint(&self, lang: Lang) -> &'static str {
        lang.t(self.hint_de, self.hint_en)
    }

    pub(super) fn resettable(&self) -> bool {
        matches!(self.def, Def::S { reset: Some(_), .. } | Def::A { reset: Some(_), .. })
    }

    /// Geänderter Wert gegenüber dem Standard (Einstellungen `s`, Standard `d`; Zustand außerhalb über `app`).
    pub(super) fn modified(&self, s: &Settings, d: &Settings, app: Option<&App>) -> bool {
        match &self.def {
            Def::None => false,
            Def::S { modified, .. } => modified(s, d),
            Def::A { modified, .. } => app.is_some_and(modified),
        }
    }

    fn risky(mut self) -> Item {
        self.risky = true;
        self
    }

    /// Trifft die Suche diesen Eintrag? Wo, entscheidet die Anzeige: Beschriftung bzw. Erklärung in der Sprache der
    /// Oberfläche, sonst die andere Sprache oder ein Stichwort.
    pub(super) fn hit(&self, q: &Query, lang: Lang) -> Option<Via> {
        let mut all: Vec<&str> = vec![self.de, self.en, self.hint_de, self.hint_en];
        all.extend_from_slice(self.kw);
        if !q.matches_any(&all) {
            return None;
        }
        let (label, hint, other_label, other_hint) = match lang {
            Lang::De => (self.de, self.hint_de, self.en, self.hint_en),
            Lang::En => (self.en, self.hint_en, self.de, self.hint_de),
        };
        let found = |t: &str| q.terms().any(|term| super::settings_nav::find(t, term).is_some());
        Some(if found(label) {
            Via::Label
        } else if found(hint) {
            Via::Hint
        } else if found(other_label) {
            Via::Other(other_label)
        } else if let Some(k) = self.kw.iter().find(|k| found(k)) {
            Via::Keyword(k)
        } else if found(other_hint) {
            Via::Other(other_label)
        } else {
            Via::Label
        })
    }
}

/// Wo ein Suchtreffer liegt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Via {
    Label,
    Hint,
    /// In der anderen Sprache (gezeigt wird deren Beschriftung).
    Other(&'static str),
    Keyword(&'static str),
}

/// Gleich nach serde (für Felder ohne `PartialEq`).
pub(super) fn same<T: serde::Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// Standard aus einem Feld von `Settings`: `f!(editor.autosave)`.
macro_rules! f {
    ($($p:tt)+) => {
        Def::S {
            modified: Arc::new(|s: &Settings, d: &Settings| !same(&s.$($p)+, &d.$($p)+)),
            reset: Some(Arc::new(|s: &mut Settings, d: &Settings| s.$($p)+ = d.$($p)+.clone())),
        }
    };
}

/// Nur „geändert“ (Zurücksetzen würde Daten löschen): `fm!(profiles)`.
macro_rules! fm {
    ($($p:tt)+) => {
        Def::S { modified: Arc::new(|s: &Settings, d: &Settings| !same(&s.$($p)+, &d.$($p)+)), reset: None }
    };
}

type Txt = (&'static str, &'static str);
const NO: Txt = ("", "");

fn it(page: Page, key: &'static str, label: Txt, hint: Txt, kw: &'static [&'static str], def: Def) -> Item {
    Item { page, key, de: label.0, en: label.1, hint_de: hint.0, hint_en: hint.1, kw, def, risky: false }
}

/// Reiter der Claude-Standards (#292) als Seite: geändert, solange der Reiter Abweichungen zählt.
fn claude_tab(tab: super::opts_tabs::Tab) -> Def {
    use super::opts_tabs::Tab;
    let i = match tab {
        Tab::Claude => 0,
        Tab::Access => 1,
        _ => 2,
    };
    Def::S {
        modified: Arc::new(move |s: &Settings, _d: &Settings| super::dialogs::tab_counts(&s.claude_defaults, &crate::claude_opts::ClaudeOpts::default())[i] > 0),
        reset: Some(Arc::new(move |s: &mut Settings, _d: &Settings| {
            super::dialogs::reset_tab(&mut s.claude_defaults, &crate::claude_opts::ClaudeOpts::default(), tab);
            // Neue Standards gelten auch im Dialog wieder (wie beim Ändern auf der Seite).
            s.last_claude_opts = None;
        })),
    }
}

/// Alle Einträge (in Seiten- und Anzeigereihenfolge).
pub(super) fn items() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(build)
}

/// Eintrag zu einem Schlüssel.
pub(super) fn item(key: &str) -> Option<&'static Item> {
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    let idx = INDEX.get_or_init(|| items().iter().enumerate().map(|(i, it)| (it.key, i)).collect());
    idx.get(key).map(|i| &items()[*i])
}

/// Einträge einer Seite.
pub(super) fn on_page(p: Page) -> impl Iterator<Item = &'static Item> {
    items().iter().filter(move |i| i.page == p)
}

fn build() -> Vec<Item> {
    use Page::*;
    let win = cfg!(windows);
    let mac = cfg!(target_os = "macos");
    let mut v = vec![
        // ---- Allgemein › Sprache & Start
        it(GeneralStart, "language", ("Sprache", "Language"), NO, &["deutsch", "english", "englisch", "german", "language"], Def::None),
        it(
            GeneralStart,
            "remember_sessions",
            ("Sitzungen merken", "Remember sessions"),
            (
                "Terminals, Chats, Projekte und Ordner beim nächsten Start wiederherstellen. Aus: jeder Start ist leer, Chatverläufe werden beim Beenden gelöscht.",
                "Restore terminals, chats, projects and folders on the next start. Off: every start is empty, chat histories are deleted on exit.",
            ),
            &["wiederherstellen", "restore", "start", "sessions", "sitzungen"],
            f!(remember_sessions),
        )
        .risky(),
        // ---- Allgemein › Benachrichtigungen
        it(GeneralNotifications, "notify_toast", ("Windows-Toast", "Windows toast"), ("Bei Freigabe oder Fertig, wenn das Fenster im Hintergrund ist", "On approval or done while the window is in the background"), &["toast", "benachrichtigung", "notification", "freigabe", "approval"], f!(notify_toast)),
        it(GeneralNotifications, "notify_sound", ("Ton", "Sound"), ("Mit dem Toast", "With the toast"), &["sound", "audio", "klang", "ton"], f!(notify_sound)),
        it(GeneralNotifications, "notify_flash", ("Taskleiste blinken", "Flash taskbar"), NO, &["taskbar", "taskleiste", "blinken", "flash"], f!(notify_flash)),
        it(GeneralNotifications, "cmd_notify_secs", ("Befehl fertig melden", "Notify when command finishes"), ("ab dieser Dauer in Sekunden (0 = aus)", "after this many seconds (0 = off)"), &["befehl", "command", "dauer", "duration", "fertig", "done"], f!(cmd_notify_secs)),
        it(
            GeneralNotifications,
            "program_notify",
            ("Benachrichtigungen von Programmen", "Notifications from programs"),
            ("OSC 9 und OSC 777 – nur für Sitzungen, die du gerade nicht ansiehst, höchstens einige pro Minute", "OSC 9 and OSC 777 – only for sessions you are not looking at, at most a few per minute"),
            &["osc 9", "osc 777", "programm", "program"],
            f!(program_notify),
        ),
        it(
            GeneralNotifications,
            "bell_toast",
            ("Toast bei Glocke", "Toast on bell"),
            ("Die Glocke (BEL) einer Sitzung, die du gerade nicht ansiehst, markiert sie immer in der Seitenleiste; an: zusätzlich Toast bzw. Hinweis", "A bell (BEL) in a session you are not looking at always marks it in the sidebar; on: also a toast or notice"),
            &["bel", "bell", "glocke", "piep", "beep"],
            f!(bell_toast),
        ),
        it(GeneralNotifications, "progress_indicator", ("Fortschritt anzeigen", "Show progress"), ("Fortschritt von Programmen (OSC 9;4) in der Seitenleiste und in der Taskleiste", "Progress reported by programs (OSC 9;4) in the sidebar and on the taskbar"), &["progress", "osc 9;4", "taskbar", "taskleiste"], f!(progress_indicator)),
        // ---- Allgemein › Fenster & Tray
        it(GeneralWindow, "confirm_close", ("Schließen bestätigen", "Confirm close"), ("Bei laufenden Sitzungen nachfragen", "Ask for running sessions"), &["close", "beenden", "quit", "nachfragen", "ask"], f!(confirm_close)),
        it(GeneralWindow, "close_to_tray", ("Schließen → Tray", "Close → tray"), ("Das Fenster verschwindet im Tray (Beenden über das Tray-Menü)", "The window goes to the tray (quit via the tray menu)"), &["tray", "infobereich", "schließen", "close"], f!(close_to_tray)),
        it(GeneralWindow, "minimize_to_tray", ("Minimieren → Tray", "Minimize → tray"), ("Kein Taskleistenbutton mehr; Klick aufs Tray-Symbol holt das Fenster zurück", "No taskbar button; clicking the tray icon brings the window back"), &["tray", "infobereich", "minimieren", "minimize", "taskleiste", "taskbar"], f!(minimize_to_tray)),
        // ---- Allgemein › Windows-Integration
        it(
            GeneralIntegration,
            "autostart",
            if win { ("Mit Windows starten", "Start with Windows") } else { ("Bei der Anmeldung starten", "Start at login") },
            if win { ("Startet minimiert im Tray", "Starts minimized in the tray") } else { ("Startet minimiert (Autostart im Benutzerordner)", "Starts minimized (autostart in your home folder)") },
            &["autostart", "login", "anmeldung", "boot", "systemstart"],
            Def::A { modified: |a| a.win_autostart.is_mine(), reset: Some(App::reset_autostart) },
        )
        .risky(),
    ];
    if !mac {
        v.push(
            it(
                GeneralIntegration,
                "context_menu",
                if win { ("Explorer-Kontextmenü", "Explorer context menu") } else { ("Dateimanager: „In Paneldeck öffnen“", "File manager: “Open in Paneldeck”") },
                if win {
                    ("Rechtsklick auf Ordner: „In Paneldeck öffnen“ / „Mit Claude starten“", "Right-click a folder: “Open in Paneldeck” / “Start with Claude”")
                } else {
                    ("Rechtsklick auf Ordner in Dolphin und Nemo; in Nautilus unter „Skripte“. Ausschalten entfernt die Dateien wieder.", "Right-click a folder in Dolphin and Nemo; in Nautilus under “Scripts”. Turning it off removes the files again.")
                },
                &["rechtsklick", "right-click", "ordner", "folder", "kontextmenü", "context menu", "explorer"],
                Def::A { modified: |a| a.win_context.is_mine(), reset: Some(App::reset_context_menu) },
            )
            .risky(),
        );
    }
    if win {
        v.push(
            it(
                GeneralIntegration,
                "edit_context",
                ("Explorer: „In Paneldeck-Editor öffnen“", "Explorer: “Open in Paneldeck editor”"),
                ("Rechtsklick auf eine Datei öffnet sie in einem neuen Editor-Tab (alle Dateiarten)", "Right-click a file to open it in a new editor tab (all file types)"),
                &["editor", "datei", "file", "kontextmenü", "context menu", "rechtsklick", "right-click"],
                Def::A { modified: |a| a.editor_svc.edit_context.is_mine(), reset: Some(App::reset_edit_context) },
            )
            .risky(),
        );
    }
    v.extend([
        // ---- Darstellung › Theme
        it(AppearanceTheme, "theme", ("Theme", "Theme"), NO, &["farbschema", "color scheme", "dark", "dunkel", "hell", "light", "farben", "colors"], f!(theme)),
        it(
            AppearanceTheme,
            "theme_manage",
            ("Theme bearbeiten", "Edit theme"),
            ("Kopie bearbeiten, neues Theme erzeugen, umbenennen, importieren, exportieren, löschen", "Edit a copy, generate a new theme, rename, import, export, delete"),
            &["theme-editor", "theme editor", "neues theme", "new theme", "hintergrund", "background", "bild", "image"],
            Def::None,
        ),
        it(AppearanceTheme, "custom_themes", ("Eigene Themes", "Custom themes"), ("JSON-Dateien im Ordner themes (Beispiel liegt dabei)", "JSON files in the themes folder (example included)"), &["json", "ordner", "folder", "neu laden", "reload"], Def::None),
        // ---- Darstellung › Schrift & Skalierung
        it(AppearanceFonts, "font_size", ("Schriftgröße", "Font size"), ("Terminalschrift (Cascadia Mono)", "Terminal font (Cascadia Mono)"), &["pt", "größe", "size", "zoom", "text"], f!(font_size)),
        it(
            AppearanceFonts,
            "terminal_font",
            ("Terminalschrift", "Terminal font"),
            (
                "Jede installierte Festbreitenschrift, z. B. Nerd Fonts. Powerline-, Rahmen- und Blockzeichen zeichnet Paneldeck selbst, damit sie lückenlos anschließen.",
                "Any installed monospace font, e.g. Nerd Fonts. Paneldeck draws Powerline, box-drawing and block characters itself so they line up without gaps.",
            ),
            &["schriftart", "font", "monospace", "cascadia", "hack", "powerline"],
            f!(terminal_font),
        ),
        it(AppearanceFonts, "ui_scale", ("UI-Skalierung", "UI scale"), ("Gesamte Oberfläche vergrößern/verkleinern", "Scale the whole interface"), &["zoom", "skalierung", "scale", "dpi", "größe", "size"], f!(ui_scale)),
        // ---- Darstellung › Transparenz & Animation
        it(AppearanceTransparency, "opacity", ("Deckkraft", "Opacity"), ("Terminal und Seitenleiste", "Terminal and sidebar"), &["transparency", "transparenz", "durchsichtig", "alpha"], f!(opacity)),
        it(
            AppearanceTransparency,
            "reduce_transparency",
            ("Transparenz reduzieren", "Reduce transparency"),
            (
                "Alle Flächen deckend, auch ohne eigenen Hintergrund des Themes (Bild, Verlauf, Muster – im Theme-Editor unter „Hintergrund“)",
                "All surfaces opaque, also without the theme's own background (image, gradient, pattern – in the theme editor under “Background”)",
            ),
            &["opak", "opaque", "deckend", "hintergrund", "background"],
            f!(reduce_transparency),
        ),
        it(AppearanceTransparency, "animations", ("Animationen", "Animations"), ("Weiche Übergänge bei Hover, Fokus und Panels", "Smooth transitions for hover, focus and panels"), &["animation", "übergang", "transition", "bewegung", "motion"], f!(animations)),
        // ---- Darstellung › Farbpaletten
        it(
            AppearancePalettes,
            "palette",
            ("Farbpaletten", "Color palettes"),
            ("Farben für Sitzungen, Projekte, Tags und Diagramme – gemeinsam oder je Bereich", "Colours for sessions, projects, tags and charts – shared or per area"),
            &["palette", "farben", "colors", "colours", "projektfarbe", "project colour", "tags", "diagramme", "charts"],
            f!(palette),
        )
        .risky(),
        // ---- Terminal › Shell & Umgebung
        it(TerminalShell, "default_shell", ("Standard-Shell", "Default shell"), ("Für neue Sitzungen und Teilungen", "For new sessions and splits"), &["powershell", "cmd", "bash", "wsl", "git bash", "shell"], f!(default_shell)),
        it(
            TerminalShell,
            "shell_integration",
            ("Shell-Integration", "Shell integration"),
            ("Befehlsmarken, Ordner-Tracking für PowerShell, cmd, bash, zsh und fish (wirkt für neue Sitzungen; ändert keine Profile oder Startdateien)", "Command marks, folder tracking for PowerShell, cmd, bash, zsh and fish (applies to new sessions; does not change profiles or startup files)"),
            &["osc 133", "befehlsmarken", "command marks", "prompt", "zsh", "fish"],
            f!(shell_integration),
        ),
        it(
            TerminalShell,
            "inherit_env",
            ("Umgebung von Paneldeck erben", "Inherit Paneldeck's environment"),
            ("Aus: frisch aus Windows (setx wirkt sofort). An: die Umgebung beim Start von Paneldeck.", "Off: fresh from Windows (setx applies at once). On: the environment Paneldeck was started with."),
            &["umgebungsvariablen", "environment variables", "path", "setx", "env"],
            f!(inherit_env),
        ),
        // ---- Terminal › Verhalten
        it(TerminalBehavior, "scrollback_lines", ("Scrollback", "Scrollback"), ("Zeilen pro Terminal (wirkt für neue Sitzungen)", "Lines per terminal (applies to new sessions)"), &["verlauf", "history", "zeilen", "lines", "puffer", "buffer"], f!(scrollback_lines)),
        it(TerminalBehavior, "cursor_blink", ("Blinkender Cursor", "Blinking cursor"), ("Wenn das Programm es anfordert; nur im fokussierten Terminal", "When the program asks for it; only in the focused terminal"), &["cursor", "blink", "blinken", "schreibmarke"], f!(cursor_blink)),
        it(
            TerminalBehavior,
            "alt_screen_wheel",
            ("Mausrad in Vollbildprogrammen", "Mouse wheel in full-screen programs"),
            (
                "Für Programme wie less oder man ohne eigene Mausunterstützung. Umschalt+Mausrad scrollt den Verlauf von Paneldeck (Vollbildprogramme haben keinen).",
                "For programs like less or man without their own mouse support. Shift+wheel scrolls Paneldeck's scrollback (full-screen programs have none).",
            ),
            &["mouse wheel", "mausrad", "scroll", "less", "man", "vim", "alternate screen"],
            f!(alt_screen_wheel),
        ),
    ]);
    if win {
        v.extend([
            it(
                TerminalBehavior,
                "shift_enter",
                ("Umschalt+Enter = neue Zeile in PowerShell", "Shift+Enter = new line in PowerShell"),
                (
                    "PSReadLine fügt dann eine Zeile ein, statt den Befehl auszuführen. Programme, die in PowerShell laufen und das kitty-Tastaturprotokoll nicht nutzen, bekommen dabei ein normales Enter.",
                    "PSReadLine then inserts a line instead of running the command. Programs running inside PowerShell that do not use the kitty keyboard protocol receive a plain Enter.",
                ),
                &["psreadline", "neue zeile", "newline", "kitty", "shift+enter", "umschalt+enter"],
                f!(shift_enter_newline_powershell),
            ),
            it(
                TerminalBehavior,
                "bundled_conpty",
                ("Mitgelieferten Konsolen-Host verwenden (empfohlen)", "Use the bundled console host (recommended)"),
                (
                    "Neuere ConPTY (OpenConsole) neben paneldeck.exe: reicht Farbabfragen und flimmerfreie Ausgabe durch. Aus: Konsolen-Host von Windows. Wirkt für neue Sitzungen.",
                    "Newer ConPTY (OpenConsole) next to paneldeck.exe: passes colour queries and flicker-free output through. Off: the console host built into Windows. Applies to new sessions.",
                ),
                &["conpty", "openconsole", "konsole", "console host", "virenscanner", "antivirus"],
                f!(bundled_conpty),
            ),
        ]);
    }
    v.extend([
        // ---- Terminal › Zwischenablage & Einfügen
        it(
            TerminalClipboard,
            "osc52_copy",
            ("Programme dürfen in die Zwischenablage kopieren (OSC 52)", "Allow programs to copy to the clipboard (OSC 52)"),
            (
                "Z. B. Claude Code, vim oder tmux in WSL- und SSH-Sitzungen. Nur die fokussierte Sitzung, nur Schreiben (nie Lesen), höchstens 1 MB. Achtung: Auch ein Agent mit Steuer-Zugriff (MCP) kann so Text in deine Zwischenablage legen.",
                "E.g. Claude Code, vim or tmux in WSL and SSH sessions. Only the focused session, write only (never read), at most 1 MB. Note: an agent with control access (MCP) can also put text on your clipboard this way.",
            ),
            &["osc 52", "clipboard", "zwischenablage", "kopieren", "copy", "tmux", "ssh"],
            f!(osc52_copy),
        )
        .risky(),
        it(
            TerminalClipboard,
            "warn_multiline_paste",
            ("Vor dem Einfügen mehrerer Zeilen nachfragen", "Warn before pasting multiple lines"),
            ("Nur wenn das Programm kein „Bracketed Paste“ nutzt – dann kann die Shell jede eingefügte Zeile sofort als Befehl ausführen.", "Only when the program does not use bracketed paste – then the shell may run each pasted line as a command right away."),
            &["paste", "einfügen", "bracketed paste", "mehrzeilig", "multi-line", "multiline"],
            f!(warn_multiline_paste),
        ),
        // ---- Terminal › Profile
        it(
            TerminalProfiles,
            "profiles",
            ("Profile", "Profiles"),
            ("Ein Profil bündelt Shell, Startbefehl, Umgebungsvariablen und Darstellung", "A profile bundles shell, startup command, environment variables and appearance"),
            &["profil", "profile", "ssh", "wsl", "administrator", "admin", "startbefehl", "startup command", "umgebungsvariablen", "environment variables", "startordner", "start folder"],
            fm!(profiles),
        ),
        // ---- Terminal › Andere Rechner
        it(
            TerminalRemote,
            "remote_snippets",
            ("Shell-Integration auf anderen Rechnern", "Shell integration on other machines"),
            ("Zeilen für die Startdatei der Shell auf dem anderen Rechner (bash, zsh, fish)", "Lines for the shell's startup file on the other machine (bash, zsh, fish)"),
            &["ssh", "tmux", "bashrc", "zshrc", "fish", "remote", "schnipsel", "snippet", "unter-shell", "sub-shell"],
            Def::None,
        ),
        // ---- Editor › Allgemein
        it(EditorGeneral, "editor_font", ("Schrift", "Font"), ("Standard: die Terminalschrift", "Default: the terminal font"), &["font", "schriftart", "monospace"], f!(editor.font)),
        it(EditorGeneral, "large_file_mb", ("Große Dateien ab", "Large files from"), ("Ohne Minimap, Umbruch und Syntaxfarben; die Suche in Dateien überspringt sie", "Without minimap, wrapping and syntax colours; search in files skips them"), &["mb", "groß", "large", "minimap"], f!(editor.large_file_mb)),
        it(EditorGeneral, "autosave", ("Automatisch speichern", "Autosave"), ("Nur Dateien, die es schon gibt; unbenannte Tabs nie", "Only files that already exist; never untitled tabs"), &["speichern", "save", "auto"], f!(editor.autosave)),
        it(EditorGeneral, "autosave_delay", ("Verzögerung", "Delay"), ("Nach der letzten Eingabe", "After the last change"), &["autosave", "automatisch speichern", "sekunden", "seconds"], f!(editor.autosave_delay_ms)),
        it(EditorGeneral, "menu_bar", ("Menüleiste", "Menu bar"), ("Datei · Bearbeiten · Auswahl · Ansicht · Format über jeder Teilung", "File · Edit · Selection · View · Format above every split"), &["menü", "menu"], f!(editor.menu_bar)),
        it(
            EditorGeneral,
            "multi_cursor",
            ("Mehrere Cursor", "Multiple cursors"),
            ("Nächstes Vorkommen, alle Vorkommen, Cursor/Auswahl zurücknehmen; Menü Auswahl und Palette", "Next occurrence, all occurrences, undo cursor/selection change; Selection menu and palette"),
            &["multi-cursor", "mehrfachauswahl", "multiple selections", "sublime", "occurrence", "vorkommen"],
            f!(editor.multi_cursor),
        ),
        it(EditorGeneral, "multi_cursor_alt_click", ("Alt+Klick", "Alt+click"), ("Setzt einen weiteren Cursor; auf einem Cursor nimmt es ihn weg", "Adds a cursor; on a cursor it removes it"), &["cursor", "maus", "mouse"], f!(editor.multi_cursor_alt_click)),
        it(
            EditorGeneral,
            "multi_cursor_middle",
            ("Mittlere Maustaste", "Middle mouse button"),
            ("Ziehen: Spaltenauswahl, mit Strg dazu; Klick: Cursor setzen, mit Strg einen dazu", "Drag: column selection, with Ctrl added; click: place the cursor, with Ctrl add one"),
            &["spaltenauswahl", "column selection", "block selection", "cursor"],
            f!(editor.multi_cursor_middle),
        ),
        // ---- Editor › Suche & Formatieren
        it(
            EditorSearchFormat,
            "format_info",
            ("Formatieren (JSON, XML)", "Formatting (JSON, XML)"),
            ("Menü Format: formatieren, minifizieren, prüfen – nie von allein", "Format menu: format, minify, validate – never automatically"),
            &["json", "xml", "format", "formatieren", "minify", "minifizieren", "validate", "prüfen", "editorconfig"],
            Def::None,
        ),
        it(EditorSearchFormat, "search_include_ignored", ("Ignorierte einbeziehen", "Include ignored"), ("Vorgabe des Schalters in der Suche: auch Dateien, die .gitignore ausschließt", "Default of the switch in the search: also files excluded by .gitignore"), &[".gitignore", "suche in dateien", "search in files", "gitignore"], f!(editor.search_include_ignored)),
        it(
            EditorSearchFormat,
            "search_exclude",
            ("Ausschlussmuster", "Exclude patterns"),
            ("Eines je Zeile, Syntax wie .gitignore; gilt immer", "One per line, syntax as in .gitignore; always applies"),
            &["exclude", "ausschließen", "node_modules", "gitignore", "muster", "pattern", "suche in dateien", "search in files"],
            f!(editor.search_exclude),
        )
        .risky(),
        // ---- Editor › Sprachen & Syntax
        it(EditorSyntax, "default_mode", ("Standard-Modus", "Default mode"), ("Für Sprachen ohne eigene Einstellung; Standard: Text", "For languages without their own setting; default: Text"), &["text", "syntax", "smart", "modus", "mode", "farben", "colours"], f!(editor.default_mode)),
        it(EditorSyntax, "language_modes", ("Modus je Sprache", "Mode per language"), ("Text, Syntax oder Smart für einzelne Sprachen", "Text, Syntax or Smart for single languages"), &["sprache", "language", "modus", "mode"], f!(editor.language_modes)).risky(),
        it(
            EditorSyntax,
            "language_map",
            ("Eigene Zuordnungen", "Own mappings"),
            ("Endung oder Dateiname → Sprache; gehen vor der eingebauten Erkennung", "Extension or file name → language; take precedence over the built-in detection"),
            &["zuordnung", "mapping", "endung", "extension", "dateiname", "file name", "erkennung", "detection"],
            f!(editor.language_map),
        )
        .risky(),
        // ---- Editor › Smart-Modus
        it(EditorSmart, "smart_auto_complete", ("Vervollständigen von selbst", "Complete automatically"), ("Nach Auslösezeichen und 2 Wortzeichen; Strg+Leertaste geht immer", "After trigger characters and 2 word characters; Ctrl+Space always works"), &["completion", "vervollständigen", "autocomplete", "lsp"], f!(editor.smart.auto_complete)),
        it(EditorSmart, "smart_trace", ("Nachrichten mitschneiden", "Trace messages"), ("Im Protokoll des Servers (nur im Speicher)", "In the server's log (in memory only)"), &["trace", "log", "protokoll", "lsp"], f!(editor.smart.trace)),
        it(EditorSmart, "smart_max_servers", ("Höchstens Server", "At most servers"), ("Der am längsten unbenutzte endet", "The one unused for the longest ends"), &["server", "limit", "grenze"], f!(editor.smart.max_servers)),
        it(EditorSmart, "smart_completion_timeout", ("Zeitgrenze Vervollständigen", "Completion timeout"), NO, &["timeout", "zeitlimit", "ms"], f!(editor.smart.completion_timeout_ms)),
        it(EditorSmart, "smart_max_diagnostics", ("Diagnosen je Datei", "Diagnostics per file"), NO, &["diagnostics", "diagnosen", "fehler", "errors", "limit"], f!(editor.smart.max_diagnostics)),
        it(EditorSmart, "smart_max_message", ("Größte Nachricht", "Largest message"), NO, &["mb", "nachricht", "message", "limit"], f!(editor.smart.max_message_mb)),
        it(EditorSmart, "smart_max_file", ("Smart-Modus bis", "Smart mode up to"), ("Größere Dateien: Syntax-Modus", "Larger files: Syntax mode"), &["mb", "datei", "file", "limit"], f!(editor.smart.max_file_mb)),
        it(EditorSmart, "smart_idle", ("Leerlauf beendet Server nach", "Idle server ends after"), ("Ohne offenes Dokument", "Without an open document"), &["idle", "leerlauf", "minuten", "minutes"], f!(editor.smart.idle_minutes)),
        it(EditorSmart, "smart_typing_pause", ("Tipp-Pause für Diagnosen", "Typing pause for diagnostics"), NO, &["pause", "tippen", "typing", "ms"], f!(editor.smart.typing_pause_ms)),
        it(
            EditorSmart,
            "smart_crash",
            ("Abstürze bis „aus“", "Crashes until “off”"),
            ("So viele Abstürze im Zeitraum: Smart für die Sprache bis zum Neustart aus", "This many crashes in the period: smart mode off for the language until restart"),
            &["crash", "absturz", "abstürze"],
            Def::S {
                modified: Arc::new(|s: &Settings, d: &Settings| s.editor.smart.crash_max != d.editor.smart.crash_max || s.editor.smart.crash_window_minutes != d.editor.smart.crash_window_minutes),
                reset: Some(Arc::new(|s: &mut Settings, d: &Settings| {
                    s.editor.smart.crash_max = d.editor.smart.crash_max;
                    s.editor.smart.crash_window_minutes = d.editor.smart.crash_window_minutes;
                })),
            },
        ),
        it(
            EditorSmart,
            "smart_servers",
            ("Server je Sprache", "Servers per language"),
            ("Eingebaute Server abwählen, eigener Pfad und Argumente", "Untick built-in servers, own path and arguments"),
            &["rust-analyzer", "pyright", "gopls", "clangd", "typescript-language-server", "pfad", "path", "argumente", "arguments", "lsp"],
            Def::S {
                modified: Arc::new(|s: &Settings, d: &Settings| !same(&s.editor.smart.disabled, &d.editor.smart.disabled) || !same(&s.editor.smart.own_paths, &d.editor.smart.own_paths) || !same(&s.editor.smart.own_args, &d.editor.smart.own_args)),
                reset: Some(Arc::new(|s: &mut Settings, d: &Settings| {
                    s.editor.smart.disabled = d.editor.smart.disabled.clone();
                    s.editor.smart.own_paths = d.editor.smart.own_paths.clone();
                    s.editor.smart.own_args = d.editor.smart.own_args.clone();
                })),
            },
        ),
        it(EditorSmart, "smart_trust", ("Vertrauenswürdige Ordner", "Trusted folders"), ("Entziehen beendet die Server dort sofort", "Revoking stops the servers there at once"), &["trust", "vertrauen", "ordner", "folder", "sicherheit", "security"], Def::None),
        // ---- Editor › Vergleich
        it(EditorDiff, "change_markers", ("Änderungsmarken am Rand", "Change markers in the gutter"), ("Hinzugefügte, geänderte und entfernte Zeilen; nicht in großen Dateien", "Added, modified and removed lines; not in large files"), &["gutter", "rand", "marken", "markers", "git", "diff"], f!(editor.change_markers)),
        it(EditorDiff, "marker_base", ("Grundlage der Marken", "Markers compare with"), ("git: nicht gestagt und gestagt in eigenen Farben; außerhalb eines Repositorys die gespeicherte Datei", "git: unstaged and staged in their own colours; outside a repository the saved file"), &["git", "head", "index", "staged"], f!(editor.marker_base)),
        it(EditorDiff, "diff_layout", ("Darstellung im Vergleich", "Comparison layout"), ("Automatisch: nebeneinander, unter 700 px untereinander", "Automatic: side by side, unified below 700 px"), &["diff", "nebeneinander", "side by side", "unified", "untereinander"], f!(editor.diff_layout)),
        it(EditorDiff, "diff_context", ("Kontextzeilen", "Context lines"), ("Unveränderte Zeilen um jede Änderung, der Rest ist eingeklappt", "Unchanged lines around each change, the rest is collapsed"), &["diff", "kontext", "context", "zeilen", "lines"], f!(editor.diff_context)),
        it(EditorDiff, "diff_ignore_whitespace", ("Leerraum ignorieren", "Ignore whitespace"), ("Vorgabe für neue Vergleiche (umschaltbar im Vergleich)", "Default for new comparisons (switchable in the comparison)"), &["whitespace", "leerzeichen", "spaces", "diff"], f!(editor.diff_ignore_whitespace)),
        // ---- Editor › Aktionen beim Speichern
        it(
            EditorOnSave,
            "save_actions",
            ("Aktionen beim Speichern", "Actions on save"),
            ("Regeln je Datei, Ordner oder Muster: vorher formatieren oder Leerraum entfernen, danach Befehle, Kopieren", "Rules per file, folder or pattern: format or trim whitespace before, commands or copying afterwards"),
            &["save actions", "regeln", "rules", "lint", "upload", "scp", "rsync", "format on save", "beim speichern formatieren", "trim", "save-actions.json"],
            Def::A { modified: |a| !a.save_actions.cfg.enabled || !a.save_actions.cfg.rules.is_empty(), reset: None },
        ),
        // ---- Claude › Allgemein
        it(ClaudeGeneral, "claude_mode", ("Claude-Modus", "Claude mode"), ("Zeigt Claude-Funktionen: Start, Status, Kosten, Verlauf", "Shows Claude features: start, status, cost, history"), &["claude code", "status", "kosten", "cost", "verlauf", "history"], f!(claude_mode)),
        it(ClaudeGeneral, "claude_resume", ("Fortsetzen", "Resume"), ("Claude-Sitzungen beim Programmstart fortsetzen (nach „Fortsetzen nach Neustart“ der Sitzung)", "Resume Claude sessions at program start (as set in the session's “Resume after restart”)"), &["resume", "neustart", "restart", "continue", "start"], f!(claude_resume)),
        // ---- Claude › Neue Sitzungen / Zugriff / Erweitert (Standards, früher Reiter im Tab „Claude“)
        it(
            ClaudeSession,
            "claude_defaults_claude",
            ("Standards für neue Claude-Sitzungen", "Defaults for new Claude sessions"),
            ("Modell, Effort, Permission-Mode, Agent", "Model, effort, permission mode, agent"),
            &["modell", "model", "effort", "permission mode", "agent", "standard", "defaults", "opus", "sonnet", "haiku"],
            claude_tab(super::opts_tabs::Tab::Claude),
        ),
        it(
            ClaudeAccess,
            "claude_defaults_access",
            ("Zugriff neuer Claude-Sitzungen", "Access of new Claude sessions"),
            ("Zusätzliche Ordner, Plugin-Ordner, erlaubte und verbotene Werkzeuge, MCP-Konfiguration", "Additional folders, plugin folders, allowed and disallowed tools, MCP configuration"),
            &["plugin-dir", "strict-mcp-config", "add-dir", "zusätzliche ordner", "additional directories", "allowed tools", "erlaubte werkzeuge", "disallowed", "mcp-config", "berechtigung", "permission", "standards", "defaults"],
            claude_tab(super::opts_tabs::Tab::Access),
        )
        .risky(),
        it(
            ClaudeAdvanced,
            "claude_defaults_advanced",
            ("Erweiterte Optionen neuer Claude-Sitzungen", "Advanced options of new Claude sessions"),
            ("Schalter, System-Prompt, Ausweichmodell, Budget, Autocompact", "Switches, system prompt, fallback model, budget, autocompact"),
            &["system prompt", "systemprompt", "fallback", "budget", "autocompact", "chrome", "ide", "verbose", "brief", "flags", "schalter", "switches", "standards", "defaults", "erweitert", "advanced"],
            claude_tab(super::opts_tabs::Tab::Advanced),
        ),
        // ---- Claude › Kosten & Budget
        it(ClaudeCosts, "daily_budget", ("Tagesbudget", "Daily budget"), ("Hinweis, wenn die geschätzten Tageskosten (UTC-Tag, alle Sitzungen) den Betrag in USD überschreiten; 0 = aus", "Notice when estimated daily cost (UTC day, all sessions) exceeds this USD amount; 0 = off"), &["budget", "usd", "dollar", "kosten", "cost", "tag", "day"], f!(daily_budget)),
        it(ClaudeCosts, "session_budget", ("Sitzungsbudget", "Session budget"), ("Hinweis, sobald eine einzelne Sitzung die geschätzten Kosten in USD überschreitet (einmal je Sitzung); 0 = aus", "Notice once a single session exceeds this estimated USD cost; 0 = off"), &["budget", "usd", "dollar", "kosten", "cost", "sitzung", "session"], f!(session_budget)),
        it(
            ClaudeCosts,
            "prices",
            ("Preise für die Kostenschätzung (USD pro Million Token)", "Prices for the cost estimate (USD per million tokens)"),
            ("Nur eine Schätzung auf Basis der lokalen Transkripte. Unbekannte Modelle werden wie Sonnet gerechnet.", "Estimate only, based on local transcripts. Unknown models are priced like Sonnet."),
            &["preise", "prices", "opus", "sonnet", "haiku", "cache", "token", "input", "output"],
            f!(prices),
        )
        .risky(),
        // ---- Claude › Hooks
        it(
            ClaudeHooks,
            "hooks_global",
            ("Hooks", "Hooks"),
            ("Hub-Sitzungen nutzen Hooks automatisch (per --settings). Global eintragen, damit auch manuell gestartetes claude gemeldet wird.", "Hub sessions use hooks automatically (via --settings). Install globally to also track manually started claude."),
            &["hooks", "settings.json", "~/.claude", "global", "installieren", "install", "status"],
            Def::A { modified: |a| a.hooks_global, reset: Some(App::reset_hooks_global) },
        )
        .risky(),
        // ---- Modelle
        it(
            ModelsServers,
            "llm_servers",
            ("Server & Modelle", "Servers & models"),
            ("Lokale Modelle (Ollama oder OpenAI-kompatible Server) für Chats; Stufe, Werkzeuge und Kontext je Modell", "Local models (Ollama or OpenAI-compatible servers) for chats; level, tools and context per model"),
            &["ollama", "lm studio", "openai", "server", "modell", "model", "api key", "schlüssel", "kontext", "context", "stufe", "level", "werkzeuge", "tools", "chat-prompt", "chat prompt"],
            Def::S { modified: Arc::new(|s: &Settings, d: &Settings| !s.llm_servers.is_empty() || !same(&s.chat_prompt_parts, &d.chat_prompt_parts)), reset: None },
        ),
        it(
            ModelsTools,
            "agent_tools",
            ("Agenten-Werkzeuge der Chats", "Agent tools in chats"),
            ("Dateien lesen und bearbeiten, suchen, Git, Befehle und Web im Projektordner des Chats", "Read and edit files, search, git, commands and web inside the chat's project folder"),
            &["werkzeuge", "tools", "zeitlimit", "timeout", "hintergrund", "background", "web", "suche", "search", "sensibel", "sensitive", "befehle", "commands"],
            f!(agent_tools),
        )
        .risky(),
        it(
            ModelsCompact,
            "chat_compact",
            ("Kompaktieren der Chats", "Compacting chats"),
            ("Das Modell fasst ältere Nachrichten zusammen; Schwelle und Sicherheitsfenster", "The model summarises older messages; threshold and safety window"),
            &["compact", "kompaktieren", "zusammenfassung", "summary", "kontext", "context", "/compact"],
            f!(chat_compact),
        ),
        // ---- Agenten › Zugriff
        it(
            AgentsAccess,
            "mcp_level",
            ("Agenten-Schnittstelle (MCP)", "Agent interface (MCP)"),
            ("Lässt KI-Agenten wie Claude Code deine Sitzungen sehen und steuern. Standardmäßig aus.", "Lets AI agents such as Claude Code see and control your sessions. Off by default."),
            &["mcp", "aus", "off", "nur lesen", "read only", "steuern", "control", "stufe", "level", "agent"],
            f!(mcp_level),
        )
        .risky(),
        it(AgentsAccess, "mcp_close_any", ("Fremde Sitzungen schließen", "Close other sessions"), ("Agenten dürfen auch Sitzungen schließen, die sie nicht selbst angelegt haben", "Agents may also close sessions they did not create"), &["schließen", "close", "sitzungen", "sessions"], f!(mcp_close_any)).risky(),
        it(
            AgentsAccess,
            "mcp_session_env",
            ("Umgebungsvariablen setzen", "Set environment variables"),
            ("Agenten dürfen Umgebungsvariablen setzen (nur in Sitzungen, die sie selbst anlegen)", "Agents may set environment variables (only in sessions they create themselves)"),
            &["env", "umgebungsvariablen", "environment variables", "create_session"],
            f!(mcp_session_env),
        )
        .risky(),
        it(AgentsAccess, "mcp_env_allow", ("Erlaubte Namen", "Allowed names"), ("Muster wie APP_*, *_URL; leer = alle außer gesperrten", "Patterns such as APP_*, *_URL; empty = all but blocked"), &["muster", "pattern", "env", "umgebungsvariablen", "environment variables"], f!(mcp_env_allow)).risky(),
        it(AgentsAccess, "agent_create_projects", ("Projekte anlegen", "Create projects"), ("Agenten dürfen Projekte anlegen (nur Ticket-Projekte, verknüpft mit ihrem Projekt)", "Agents may create projects (ticket projects only, linked to their project)"), &["projekt", "project", "tickets"], f!(agent_create_projects)).risky(),
        it(AgentsAccess, "agent_delete", ("Löschen", "Delete"), ("Agenten dürfen Tickets/Kommentare löschen (in den Papierkorb; nur Kommentare von Agenten)", "Agents may delete tickets/comments (to the trash; only comments written by agents)"), &["löschen", "delete", "papierkorb", "trash", "tickets", "kommentare", "comments"], f!(agent_delete)).risky(),
        it(AgentsAccess, "todo_new_private", ("Neue Karten privat", "New cards private"), ("Neue Karten der Todo-Liste nie für Agenten sichtbar", "New to-do cards never visible to agents"), &["privat", "private", "todo", "karten", "cards"], f!(todo_new_private)).risky(),
        it(
            AgentsAccess,
            "agent_access",
            ("Zugriff je Projekt", "Access per project"),
            ("Was Agenten je Projekt dürfen; dazu der Bereich ohne Projekt und die Ticket-Projekte", "What agents may do per project; also the area without project and the ticket projects"),
            &["projekt", "project", "sperren", "lock", "zugriff", "access", "ticket-projekte", "ticket projects"],
            f!(agent_general),
        )
        .risky(),
        it(AgentsAccess, "agent_trash", ("Papierkorb", "Trash"), ("Von Agenten gelöschte Tickets und Kommentare", "Tickets and comments deleted by agents"), &["papierkorb", "trash", "wiederherstellen", "restore"], Def::None),
        // ---- Agenten › Editor (früher Editor → Agenten)
        it(AgentsEditor, "agent_open", ("Agenten dürfen Dateien öffnen", "Agents may open files"), ("Neuer Tab; nur lokale Pfade, nie Netzwerk-/UNC-Pfade oder Geräte", "New tab; local paths only, never network/UNC paths or devices"), &["editor_open", "editor", "öffnen", "open"], f!(editor.agent_open)).risky(),
        it(AgentsEditor, "agent_edit", ("Agenten dürfen offene Dateien bearbeiten", "Agents may edit open files"), ("Ungespeichert, als „vom Agenten“ markiert, rückgängig machbar", "Unsaved, marked “by agent”, can be undone"), &["editor_edit", "editor", "bearbeiten", "edit"], f!(editor.agent_edit)).risky(),
        it(AgentsEditor, "agent_save", ("Agenten dürfen speichern", "Agents may save"), ("Nur offene Dateien, die es schon gibt; unbenannte Tabs speichert nur du", "Only open files that already exist; only you save untitled tabs"), &["editor_save", "editor", "speichern", "save"], f!(editor.agent_save)).risky(),
        it(AgentsEditor, "agent_format", ("Agenten dürfen formatieren", "Agents may format"), ("format_file: offene JSON/XML-Dateien; Prüfen geht ab „Nur lesen“ ohne Schalter", "format_file: open JSON/XML files; validating works from “Read only” without a switch"), &["format_file", "editor", "formatieren", "format"], f!(editor.agent_format)).risky(),
        it(AgentsEditor, "agent_selections", ("Agenten sehen die Auswahlen", "Agents see the selections"), ("editor_read nennt deine Cursor und Auswahlen (ab „Nur lesen“)", "editor_read reports your cursors and selections (from “Read only”)"), &["editor_read", "editor", "cursor", "auswahl", "selection"], f!(editor.agent_selections)).risky(),
        // ---- Agenten › Einrichten & Systemprompt
        it(
            AgentsSetup,
            "mcp_register",
            ("In Claude Code einrichten", "Set up in Claude Code"),
            ("Registrieren, Entfernen, Befehl kopieren, JSON für andere Agenten", "Register, remove, copy command, JSON for other agents"),
            &["registrieren", "register", "claude mcp add", "mcpservers", "json", "cursor", "claude desktop", "servername", "server name"],
            Def::None,
        ),
        it(AgentsSetup, "mcp_sysprompt", ("Systemprompt", "System prompt"), ("Anleitung für Agenten (eigene Vorlage system-prompt.md), Sprache des Prompts", "Instructions for agents (own template system-prompt.md), prompt language"), &["system prompt", "system-prompt.md", "anleitung", "instructions", "sprache", "language"], f!(mcp_prompt_lang)),
        // ---- Agenten › Aufrufprotokoll
        it(AgentsLog, "mcp_log", ("Aufrufprotokoll", "Call log"), ("Letzte Aufrufe der Agenten; Datei mcp-log.jsonl", "Recent agent calls; file mcp-log.jsonl"), &["log", "protokoll", "mcp-log.jsonl", "aufrufe", "calls"], Def::None),
        // ---- Daten & Sicherung
        it(DataBackup, "backup_interval", ("Intervall", "Interval"), ("Stunden zwischen automatischen Sicherungen (0 = keine automatische Sicherung)", "Hours between automatic backups (0 = no automatic backup)"), &["sicherung", "backup", "automatisch", "automatic", "stunden", "hours"], f!(backup_interval_hours)),
        it(DataBackup, "backup_keep", ("Behaltene Kopien", "Copies to keep"), ("Zusätzlich zur Datenbank selbst; manuelle Sicherungen zählen nicht mit", "Besides the database itself; manual backups do not count"), &["sicherung", "backup", "kopien", "copies"], f!(backup_keep)),
        it(DataBackup, "backup_dir", ("Ordner", "Folder"), ("Wo die Sicherungen liegen; „Jetzt sichern“", "Where the backups are; “Back up now”"), &["sicherung", "backup", "ordner", "folder", "jetzt sichern", "back up now", "--backup-dir"], f!(backup_dir)),
        it(DataRestore, "backups_list", ("Vorhandene Sicherungen", "Existing backups"), ("Wiederherstellen beendet Paneldeck und spielt die Sicherung beim nächsten Start ein", "Restoring quits Paneldeck and applies the backup at the next start"), &["wiederherstellen", "restore", "sicherung", "backup"], Def::None),
        it(DataMaintenance, "db_care", ("Datenbankpflege", "Database care"), ("Prüfen, verdichten, reparieren, retten – auf der Wartungsseite", "Check, compact, repair, rescue – on the maintenance page"), &["prüfen", "check", "verdichten", "compact", "reparieren", "repair", "retten", "rescue", "wartung", "maintenance", "datenbank", "database"], Def::None),
        it(
            DataMaintenance,
            "cleanup",
            ("Aufräumen", "Clean up"),
            ("Wählen, was gelöscht werden soll – bis zum kompletten Zurücksetzen", "Choose what to delete – up to a complete reset"),
            &["löschen", "delete", "zurücksetzen", "reset", "cleanup", "verlauf", "history", "protokolle", "logs"],
            Def::None,
        ),
        it(
            DataMaintenance,
            "undo_steps",
            ("Änderungshistorie (Rückgängig)", "Undo history"),
            ("Wie viele Änderungen der Todo-Liste sich mit Strg+Z zurücknehmen lassen", "How many changes to the to-do list Ctrl+Z can take back"),
            &["undo", "rückgängig", "strg+z", "ctrl+z", "todo"],
            f!(undo_steps),
        ),
        it(DataDiagnostics, "diagnostics", ("Diagnose", "Diagnostics"), ("Systeminfos für Fehlerberichte", "System info for bug reports"), &["fehlerbericht", "bug report", "error report", "absturz", "crash", "kopieren", "copy"], Def::None),
        it(DataDiagnostics, "data_folder", ("Datenordner", "Data folder"), ("Einstellungen, Zustand, Datenbank und Protokolle", "Settings, state, database and logs"), &["datenordner", "data folder", "ordner öffnen", "open folder", "paneldeck-data"], Def::None),
        // ---- Info
        it(AboutVersion, "about_info", ("Programminfo", "Program info"), ("Programm, Datenordner, Herkunft, Profil, Startoptionen, Build", "Program, data folder, source, profile, start overrides, build"), &["version", "programm", "program", "profil", "profile", "build", "herkunft", "source", "exe"], Def::None),
        it(AboutUpdates, "update_auto_check", ("Automatisch prüfen", "Check automatically"), ("Beim Start, höchstens einmal am Tag", "At start, at most once a day"), &["update", "aktualisierung", "github", "automatisch", "automatic"], f!(update_auto_check)),
        it(AboutUpdates, "updates", ("Updates", "Updates"), ("Nach Updates suchen, letzte Prüfung, übersprungene Version", "Check for updates, last check, skipped version"), &["update", "aktualisierung", "version", "github", "download"], Def::None),
        it(AboutTutorial, "tutorial", ("Tutorial", "Tutorial"), ("Geführte Schritte in der echten Oberfläche", "Guided steps in the real interface"), &["tour", "hilfe", "help", "kapitel", "chapters", "was ist neu", "what's new"], Def::None),
        it(AboutTutorial, "tutorial_whats_new", ("„Was ist neu“ von selbst starten", "Start “What's new” by itself"), ("Nach einem Update die Tour „Was ist neu“ starten (aus: nur ein Hinweis in der Seitenleiste)", "Start the “What's new” tour after an update (off: just a hint in the sidebar)"), &["tour", "update", "was ist neu", "what's new"], f!(tutorial_whats_new)),
        it(AboutLicenses, "licenses", ("Lizenzen", "Licenses"), ("Lizenzen der verwendeten Bibliotheken (THIRD_PARTY.md)", "Licenses of the libraries used (THIRD_PARTY.md)"), &["lizenz", "license", "third party", "drittanbieter", "open source", "mit"], Def::None),
    ]);
    // Tastenkürzel: eine Zeile je Kürzel (Name in beiden Sprachen, Standardbelegung als Stichwort).
    for (id, default) in crate::keys::DEFAULTS {
        let key: &'static str = Box::leak(format!("key.{id}").into_boxed_str());
        // Kürzel ohne Standardbelegung (z. B. „Alle Sitzungen schließen“): keine Stichwörter.
        let kw: Vec<&'static str> = if default.is_empty() { Vec::new() } else { vec![*default, Box::leak(crate::keys::pretty(default, true).into_boxed_str()) as &'static str] };
        let kw: &'static [&'static str] = Box::leak(kw.into_boxed_slice());
        let id: &'static str = id;
        v.push(it(
            Page::Shortcuts,
            key,
            (crate::keys::label(id, true), crate::keys::label(id, false)),
            NO,
            kw,
            // Geändert nur, wenn die Belegung von der Standardbelegung abweicht (ein Eintrag gleich dem Standard zählt nicht).
            Def::S { modified: Arc::new(move |s: &Settings, _d: &Settings| s.keys.get(id).is_some_and(|v| crate::keys::parse(v) != crate::keys::parse(default))), reset: Some(Arc::new(move |s: &mut Settings, _d: &Settings| {
                s.keys.remove(id);
            })) },
        ));
    }
    v
}

// ------------------------------------------------------------------------------------------------- Suche, Zählen

/// Ergebnis der Suche über alle Einträge.
#[derive(Default)]
pub(super) struct Found {
    /// Getroffene Einträge und wo.
    pub hits: HashMap<&'static str, Via>,
    /// Treffer je Seite (nur Seiten mit Treffern oder passendem Namen).
    pub per_page: BTreeMap<Page, usize>,
}

impl Found {
    pub(super) fn total(&self) -> usize {
        self.hits.len()
    }

    pub(super) fn pages(&self) -> BTreeSet<Page> {
        self.per_page.keys().copied().collect()
    }
}

/// Alle Einträge und Seitennamen durchsuchen (Deutsch und Englisch immer beide).
pub(super) fn search(q: &Query, lang: Lang) -> Found {
    let mut f = Found::default();
    if q.is_empty() {
        return f;
    }
    for i in items() {
        if let Some(via) = i.hit(q, lang) {
            f.hits.insert(i.key, via);
            *f.per_page.entry(i.page).or_default() += 1;
        }
    }
    for p in Page::ALL {
        if super::settings_nav::page_title_hit(q, *p) {
            f.per_page.entry(*p).or_default();
        }
    }
    f
}

/// Geänderte Einträge (Schlüssel) bei den Einstellungen `s`.
pub(super) fn modified_keys(s: &Settings, app: Option<&App>) -> HashSet<&'static str> {
    let d = Settings::default();
    items().iter().filter(|i| i.modified(s, &d, app)).map(|i| i.key).collect()
}

/// Zwischenspeicher für [`modified_keys_cached`]: Fingerabdruck der Einstellungen und die geänderten `Def::S`-Einträge.
pub(super) type ModCache = Option<(u64, HashSet<&'static str>)>;

/// Wie [`modified_keys`], aber die Vergleiche mit dem Standard (`Def::S`, ~110 serde-Vergleiche) nur, wenn sich die
/// Einstellungen geändert haben (in jedem Bild nur noch eine Serialisierung); die Einträge mit App-Zustand (`Def::A`)
/// sind billig und werden immer gefragt.
pub(super) fn modified_keys_cached(s: &Settings, app: Option<&App>, cache: &mut ModCache) -> HashSet<&'static str> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(s).unwrap_or_default().hash(&mut h);
    let fp = h.finish();
    if cache.as_ref().is_none_or(|(f, _)| *f != fp) {
        let d = Settings::default();
        let set = items().iter().filter(|i| matches!(i.def, Def::S { .. }) && i.modified(s, &d, None)).map(|i| i.key).collect();
        *cache = Some((fp, set));
    }
    let mut out = cache.as_ref().map(|(_, set)| set.clone()).unwrap_or_default();
    out.extend(items().iter().filter(|i| matches!(i.def, Def::A { .. }) && i.modified(s, s, app)).map(|i| i.key));
    out
}

/// Geänderte je Seite.
pub(super) fn per_page(keys: &HashSet<&'static str>) -> BTreeMap<Page, usize> {
    let mut m = BTreeMap::new();
    for k in keys {
        if let Some(i) = item(k) {
            *m.entry(i.page).or_default() += 1;
        }
    }
    m
}

/// Einträge zurücksetzen, die nur `settings.json` betreffen (`Def::S`); liefert die übrigen (`Def::A`) zurück.
pub(super) fn reset_settings(s: &mut Settings, keys: &[&'static str]) -> Vec<&'static str> {
    let d = Settings::default();
    let mut rest = Vec::new();
    for k in keys {
        match item(k).map(|i| &i.def) {
            Some(Def::S { reset: Some(r), .. }) => r(s, &d),
            Some(Def::A { reset: Some(_), .. }) => rest.push(*k),
            _ => {}
        }
    }
    rest
}

// ------------------------------------------------------------------------------------------------- Zeilen

/// Was eine Seite in diesem Bild zeigt (ohne Bezug auf die App, damit die Zeilen `&mut self` frei lassen).
pub(super) struct RowCx {
    pub lang: Lang,
    pub page: Page,
    q: Query,
    /// Gezeigte Einträge dieser Seite (`None` = alle).
    show: Option<HashSet<&'static str>>,
    hits: HashMap<&'static str, Via>,
    modified: HashSet<&'static str>,
    /// Zurücksetzen angeklickt (Schlüssel).
    pub reset: Cell<Option<&'static str>>,
    /// Verweis auf eine andere Seite angeklickt.
    pub go: Cell<Option<Page>>,
    /// „Seite zurücksetzen“ angeklickt.
    pub reset_page: Cell<bool>,
}

impl RowCx {
    /// `found` = Suchergebnis (bei Suche), `only_modified` = Filter „Nur geänderte“ (ohne Suche), `show_all` = auf dieser
    /// Seite trotz Suche alles zeigen.
    pub(super) fn new(lang: Lang, page: Page, q: Query, found: Option<&Found>, modified: HashSet<&'static str>, only_modified: bool, show_all: bool) -> RowCx {
        let mut show = None;
        let mut hits = HashMap::new();
        if let Some(f) = found {
            hits = f.hits.iter().filter(|(k, _)| item(k).is_some_and(|i| i.page == page)).map(|(k, v)| (*k, *v)).collect();
            // Seite nur über ihren Namen gefunden: alles zeigen.
            if !show_all && !hits.is_empty() {
                show = Some(hits.keys().copied().collect());
            }
        } else if only_modified {
            show = Some(on_page(page).filter(|i| modified.contains(i.key)).map(|i| i.key).collect());
        }
        RowCx { lang, page, q, show, hits, modified, reset: Cell::new(None), go: Cell::new(None), reset_page: Cell::new(false) }
    }

    /// Wird der Eintrag gezeigt?
    pub(super) fn visible(&self, key: &str) -> bool {
        self.show.as_ref().is_none_or(|s| s.contains(key))
    }

    /// Ohne Filter: auch freie Texte und Verweise zeigen.
    pub(super) fn plain(&self) -> bool {
        self.show.is_none()
    }

    pub(super) fn is_modified(&self, key: &str) -> bool {
        self.modified.contains(key)
    }

    /// (gezeigt, alle) Einträge der Seite.
    pub(super) fn counts(&self) -> (usize, usize) {
        let all = on_page(self.page).count();
        (on_page(self.page).filter(|i| self.visible(i.key)).count(), all)
    }

    /// Beschriftungen der ausgeblendeten Einträge.
    pub(super) fn hidden(&self) -> Vec<&'static str> {
        on_page(self.page).filter(|i| !self.visible(i.key)).map(|i| i.label(self.lang)).collect()
    }

    /// Geänderte, zurücksetzbare Einträge dieser Seite.
    pub(super) fn page_resettable(&self) -> Vec<&'static str> {
        on_page(self.page).filter(|i| self.modified.contains(i.key) && i.resettable()).map(|i| i.key).collect()
    }
}

/// Linker Rand der Zeilen (Platz für die Marke „geändert“).
pub(super) const GUTTER: f32 = 12.0;
/// Platz für den Knopf „Zurücksetzen“ rechts.
const RESET_W: f32 = 30.0;
/// Mindesthöhe einer Zeile.
const ROW_H: f32 = 34.0;

/// Breite der Beschriftungsspalte bei der Breite `w` der Seite.
pub(super) fn label_width(w: f32) -> f32 {
    (w * 0.46).clamp(170.0, 340.0)
}

/// Text mit hervorgehobenen Fundstellen.
pub(super) fn highlighted(text: &str, ranges: &[Range<usize>], font: FontId, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let hl = style::ui().warning.gamma_multiply(0.35);
    let fmt = |bg: Color32| TextFormat { font_id: font.clone(), color, background: bg, ..Default::default() };
    let mut at = 0;
    for r in ranges {
        if r.start > at {
            job.append(&text[at..r.start], 0.0, fmt(Color32::TRANSPARENT));
        }
        job.append(&text[r.clone()], 0.0, fmt(hl));
        at = r.end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, fmt(Color32::TRANSPARENT));
    }
    job
}

/// Beschriftung und Erklärung eines Eintrags (mit Hervorhebung und ggf. „Stichwort: …“).
fn label_block(ui: &mut Ui, cx: &RowCx, it: &Item, hint: Option<&str>, strong: bool) {
    let label = it.label(cx.lang);
    let text_col = if strong { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let body = egui::TextStyle::Body.resolve(ui.style());
    ui.add(egui::Label::new(highlighted(label, &cx.q.ranges(label), body, text_col)).wrap());
    let hint = hint.unwrap_or(it.hint(cx.lang));
    let small = egui::TextStyle::Small.resolve(ui.style());
    let dim = style::dim(ui);
    let note = match cx.hits.get(it.key) {
        Some(Via::Keyword(k)) => Some((cx.lang.t("Stichwort", "Keyword"), *k)),
        Some(Via::Other(t)) => Some((cx.lang.t("English", "Deutsch"), *t)),
        _ => None,
    };
    if !hint.is_empty() || note.is_some() {
        let mut job = highlighted(hint, &cx.q.ranges(hint), small.clone(), dim);
        if let Some((what, t)) = note {
            let fmt = TextFormat { font_id: small.clone(), color: dim, ..Default::default() };
            job.append(&format!("{}{what}: ", if hint.is_empty() { "" } else { "  ·  " }), 0.0, fmt);
            let extra = highlighted(t, &cx.q.ranges(t), small, dim);
            for s in extra.sections {
                job.append(&extra.text[s.byte_range.clone()], 0.0, s.format);
            }
        }
        ui.add(egui::Label::new(job).wrap());
    }
}

/// Knopf „Zurücksetzen“ (↺) bzw. Platzhalter rechts in einer Zeile.
fn reset_button(ui: &mut Ui, cx: &RowCx, it: &Item, at: Pos2) {
    if !(cx.is_modified(it.key) && it.resettable()) {
        return;
    }
    let r = Rect::from_center_size(at, Vec2::splat(24.0));
    let resp = ui.interact(r, ui.id().with(("settings_reset", it.key)), Sense::click());
    let col = if resp.hovered() { ui.visuals().strong_text_color() } else { style::accent() };
    if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, ui.visuals().widgets.hovered.bg_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    style::paint_icon(ui.painter(), style::Icon::Reload, r.center(), col);
    let tip = if it.risky { cx.lang.t("Auf den Standard zurücksetzen (fragt nach)", "Reset to default (asks first)") } else { cx.lang.t("Auf den Standard zurücksetzen", "Reset to default") };
    if resp.on_hover_text(tip).clicked() {
        cx.reset.set(Some(it.key));
    }
}

/// Marke „geändert“ am linken Rand.
fn marker(ui: &Ui, cx: &RowCx, key: &str, rect: Rect) {
    if cx.is_modified(key) {
        let bar = Rect::from_min_max(Pos2::new(rect.left() + 1.0, rect.top() + 4.0), Pos2::new(rect.left() + 4.0, rect.bottom() - 4.0));
        ui.painter().rect_filled(bar, 1.5, style::accent());
    }
}

/// Zeile einer Einstellung: Beschriftung und Erklärung links, Bedienelement rechts, ↺ ganz rechts, Marke links.
pub(super) fn srow(ui: &mut Ui, cx: &RowCx, key: &'static str, add: impl FnOnce(&mut Ui)) {
    srow_hint(ui, cx, key, None, add);
}

/// [`srow`] mit einer Erklärung, die sich zur Laufzeit ändert (Kürzel im Text, Warnungen).
pub(super) fn srow_hint(ui: &mut Ui, cx: &RowCx, key: &'static str, hint: Option<&str>, add: impl FnOnce(&mut Ui)) {
    let Some(it) = item(key) else {
        debug_assert!(false, "unbekannte Einstellung {key}");
        add(ui);
        return;
    };
    if !cx.visible(key) {
        return;
    }
    let origin = ui.cursor().min;
    let w = ui.available_width();
    let lw = label_width(w - GUTTER - RESET_W);
    let mut lab = ui.new_child(UiBuilder::new().id_salt(("srow_l", key)).max_rect(Rect::from_min_size(Pos2::new(origin.x + GUTTER, origin.y + 4.0), Vec2::new(lw, 4000.0))).layout(Layout::top_down(Align::LEFT)));
    label_block(&mut lab, cx, it, hint, false);
    let h = (lab.min_rect().height() + 8.0).max(ROW_H);
    let cx0 = origin.x + GUTTER + lw + S3;
    let ctrl_max = Rect::from_min_max(Pos2::new(cx0, origin.y), Pos2::new((origin.x + w - RESET_W).max(cx0 + 40.0), origin.y + h));
    let mut ctrl = ui.new_child(UiBuilder::new().id_salt(("srow_c", key)).max_rect(ctrl_max).layout(Layout::left_to_right(Align::Center)));
    add(&mut ctrl);
    let h = h.max(ctrl.min_rect().bottom() - origin.y);
    let rect = Rect::from_min_size(origin, Vec2::new(w, h));
    reset_button(ui, cx, it, Pos2::new(origin.x + w - RESET_W / 2.0, origin.y + h.min(ROW_H + 8.0) / 2.0));
    marker(ui, cx, key, rect);
    ui.advance_cursor_after_rect(rect);
    ui.add_space(2.0);
}

/// Abschnitt mit eigener Oberfläche (Listen, Raster, Dialogknöpfe): Überschrift (optional) mit ↺, darunter der Inhalt
/// eingerückt; Marke über die ganze Höhe. Die Überschrift fehlt, wenn der Inhalt sie selbst zeichnet.
pub(super) fn block(ui: &mut Ui, cx: &RowCx, key: &'static str, heading: bool, add: impl FnOnce(&mut Ui)) {
    let Some(it) = item(key) else {
        debug_assert!(false, "unbekannte Einstellung {key}");
        add(ui);
        return;
    };
    if !cx.visible(key) {
        return;
    }
    let origin = ui.cursor().min;
    let w = ui.available_width();
    let mut inner = ui.new_child(UiBuilder::new().id_salt(("sblock", key)).max_rect(Rect::from_min_size(Pos2::new(origin.x + GUTTER, origin.y + 4.0), Vec2::new(w - GUTTER - RESET_W, 100_000.0))).layout(Layout::top_down(Align::LEFT)));
    if heading || cx.hits.contains_key(key) {
        if heading {
            label_block(&mut inner, cx, it, None, true);
        } else {
            // Ohne eigene Überschrift: bei einem Treffer zeigen, was gefunden wurde.
            let small = egui::TextStyle::Small.resolve(inner.style());
            let t = format!("{} · {}", it.label(cx.lang), it.hint(cx.lang));
            let dim = style::dim(&inner);
            inner.add(egui::Label::new(highlighted(&t, &cx.q.ranges(&t), small, dim)).wrap());
        }
        inner.add_space(S1);
    }
    add(&mut inner);
    let h = inner.min_rect().bottom() - origin.y + 4.0;
    let rect = Rect::from_min_size(origin, Vec2::new(w, h.max(ROW_H)));
    reset_button(ui, cx, it, Pos2::new(origin.x + w - RESET_W / 2.0, origin.y + ROW_H / 2.0));
    marker(ui, cx, key, rect);
    ui.advance_cursor_after_rect(rect);
    ui.add_space(S2);
}

/// Freier Hinweis (nur ohne Filter), eingerückt wie die Zeilen.
pub(super) fn note(ui: &mut Ui, cx: &RowCx, text: &str) {
    if !cx.plain() {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.add(egui::Label::new(egui::RichText::new(text).small().color(style::dim(ui))).wrap());
    });
}

/// Verweis auf eine andere Seite (Einstellungen, die an zwei Orte gehören, stehen an einem; am anderen dieser Verweis).
pub(super) fn link(ui: &mut Ui, cx: &RowCx, text: &str, target: Page) {
    if !cx.plain() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.add_space(GUTTER);
        ui.label(egui::RichText::new(text).small().color(style::dim(ui)));
        if ui.link(egui::RichText::new(format!("→ {}", target.path(cx.lang))).small()).clicked() {
            cx.go.set(Some(target));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_every_page_has_entries() {
        let mut seen = HashSet::new();
        for i in items() {
            assert!(seen.insert(i.key), "doppelter Schlüssel {}", i.key);
            assert!(item(i.key).is_some_and(|x| std::ptr::eq(x, i)));
        }
        for p in Page::ALL {
            assert!(on_page(*p).next().is_some(), "Seite {} ohne Einträge (die Suche fände sie nur über den Namen)", p.id());
        }
    }

    #[test]
    fn every_entry_is_bilingual() {
        for i in items() {
            assert!(!i.de.is_empty() && !i.en.is_empty(), "{}", i.key);
            assert_eq!(i.hint_de.is_empty(), i.hint_en.is_empty(), "{}: Erklärung nur in einer Sprache", i.key);
            for k in i.kw {
                assert!(!k.is_empty(), "{}: leeres Stichwort", i.key);
                // Klein geschrieben (die Suche ist es ohnehin); Kürzel-Belegungen dürfen bleiben, wie sie angezeigt werden.
                assert!(i.page == Page::Shortcuts || k.to_lowercase() == **k, "{}: Stichwort {k:?} klein schreiben", i.key);
            }
        }
    }

    #[test]
    fn defaults_are_not_modified_and_changes_are_detected_and_reset() {
        let d = Settings::default();
        let mut s = Settings::default();
        let m = modified_keys(&s, None);
        assert!(m.is_empty(), "Standard gilt als unverändert: {m:?}");
        s.opacity = 0.8;
        s.animations = false;
        s.cursor_blink = false;
        s.editor.smart.crash_max = d.editor.smart.crash_max + 1;
        s.keys.insert("new".into(), "Ctrl+Alt+N".into());
        s.claude_defaults.model = "opus".into();
        s.profiles.push(Default::default());
        let m = modified_keys(&s, None);
        for k in ["opacity", "animations", "cursor_blink", "smart_crash", "key.new", "claude_defaults_claude", "profiles"] {
            assert!(m.contains(k), "{k} geändert: {m:?}");
        }
        let pages = per_page(&m);
        assert_eq!(pages.get(&Page::AppearanceTransparency), Some(&2));
        assert_eq!(pages.get(&Page::TerminalBehavior), Some(&1), "Cursor gehört jetzt zu Terminal › Verhalten");
        // Zurücksetzen: nur Einstellungen mit Standard; Profile bleiben (würde Daten löschen).
        let keys: Vec<&'static str> = m.iter().copied().collect();
        assert!(reset_settings(&mut s, &keys).is_empty(), "keine App-Einträge geändert");
        let m = modified_keys(&s, None);
        assert_eq!(m, HashSet::from(["profiles"]), "nur die Profile bleiben");
        assert_eq!(s.opacity, d.opacity);
        assert!(s.keys.is_empty() && s.last_claude_opts.is_none());
    }

    /// Zwischengespeichert wie ungespeichert; ein Kürzel gleich dem Standard zählt nicht als geändert.
    #[test]
    fn cached_modified_keys_follow_changes_and_default_shortcuts_are_unchanged() {
        let mut s = Settings::default();
        let mut cache: ModCache = None;
        assert_eq!(modified_keys_cached(&s, None, &mut cache), modified_keys(&s, None));
        s.opacity = 0.5;
        let m = modified_keys_cached(&s, None, &mut cache);
        assert!(m.contains("opacity"), "neu verglichen nach der Änderung");
        assert_eq!(m, modified_keys(&s, None));
        let (id, default) = crate::keys::DEFAULTS.iter().find(|(_, d)| !d.is_empty()).copied().unwrap();
        s.keys.insert(id.to_string(), default.to_string());
        let key = format!("key.{id}");
        assert!(!modified_keys_cached(&s, None, &mut cache).iter().any(|k| *k == key), "gleich dem Standard");
        s.keys.insert(id.to_string(), "ctrl+alt+shift+F12".into());
        assert!(modified_keys_cached(&s, None, &mut cache).iter().any(|k| *k == key));
    }

    #[test]
    fn risky_entries_ask_first() {
        // Auch selbst eingegebene Listen (ein Klick würde sie sonst ohne Rückfrage löschen).
        for k in ["remember_sessions", "mcp_level", "autostart", "hooks_global", "osc52_copy", "agent_open", "claude_defaults_access", "language_map", "language_modes", "search_exclude", "prices"] {
            assert!(item(k).is_some_and(|i| i.risky), "{k} fragt nach");
        }
        assert!(item("opacity").is_some_and(|i| !i.risky && i.resettable()));
        assert!(item("profiles").is_some_and(|i| !i.resettable()), "Profile nie per ↺ löschen");
        assert!(item("llm_servers").is_some_and(|i| !i.resettable()), "Server nie per ↺ löschen");
    }

    #[test]
    fn search_finds_both_languages_keywords_and_old_tab_names() {
        let q = Query::new("transp");
        let f = search(&q, Lang::De);
        // Deckkraft über das Stichwort „transparency“, „Transparenz reduzieren“ über den Namen.
        assert_eq!(f.hits.get("opacity"), Some(&Via::Keyword("transparency")));
        assert_eq!(f.hits.get("reduce_transparency"), Some(&Via::Label));
        assert!(!f.hits.contains_key("animations"));
        assert_eq!(f.per_page.get(&Page::AppearanceTransparency), Some(&2));
        // „opacity“ findet „Deckkraft“ in der deutschen Oberfläche (englische Beschriftung)
        let f = search(&Query::new("opacity"), Lang::De);
        assert_eq!(f.hits.get("opacity"), Some(&Via::Other("Opacity")));
        // „Deckkraft“ in der englischen Oberfläche
        let f = search(&Query::new("deckkraft"), Lang::En);
        assert_eq!(f.hits.get("opacity"), Some(&Via::Other("Deckkraft")));
        // Erklärungen und Kürzelnamen
        assert!(search(&Query::new("psreadline"), Lang::En).hits.contains_key("shift_enter") || !cfg!(windows));
        assert!(search(&Query::new("neue sitzung"), Lang::En).hits.contains_key("key.new"), "Name des Kürzels");
        assert!(search(&Query::new("ctrl+shift+t"), Lang::De).hits.contains_key("key.new"), "Standardbelegung");
        // Alter Reitername „Darstellung“ findet die nach Terminal gewanderten Einstellungen über die Seite
        let f = search(&Query::new("darstellung"), Lang::De);
        assert!(f.per_page.contains_key(&Page::TerminalBehavior));
        // Nichts gefunden
        let f = search(&Query::new("xyzzy"), Lang::De);
        assert_eq!((f.total(), f.per_page.len()), (0, 0));
    }

    #[test]
    fn rowcx_filters_rows_for_search_and_only_modified() {
        let q = Query::new("transp");
        let f = search(&q, Lang::De);
        let cx = RowCx::new(Lang::De, Page::AppearanceTransparency, q.clone(), Some(&f), HashSet::new(), false, false);
        assert!(cx.visible("opacity") && cx.visible("reduce_transparency") && !cx.visible("animations"));
        assert_eq!(cx.counts(), (2, 3));
        assert_eq!(cx.hidden(), vec!["Animationen"]);
        assert!(!cx.plain());
        // „Alle zeigen“
        let cx = RowCx::new(Lang::De, Page::AppearanceTransparency, q.clone(), Some(&f), HashSet::new(), false, true);
        assert!(cx.visible("animations") && cx.plain());
        // Seite nur über den Namen gefunden: alle Zeilen
        let q2 = Query::new("verhalten");
        let f2 = search(&q2, Lang::De);
        let cx = RowCx::new(Lang::De, Page::TerminalBehavior, q2, Some(&f2), HashSet::new(), false, false);
        assert!(cx.visible("scrollback_lines") && cx.visible("cursor_blink"));
        // Nur geänderte
        let cx = RowCx::new(Lang::En, Page::AppearanceTransparency, Query::default(), None, HashSet::from(["opacity"]), true, false);
        assert!(cx.visible("opacity") && !cx.visible("animations"));
        assert_eq!(cx.page_resettable(), vec!["opacity"]);
    }

    /// Zeilen ohne App: Beschriftung, Erklärung, Hervorhebung und „Stichwort“, Klick auf ↺ meldet den Schlüssel.
    #[test]
    fn rows_render_labels_markers_and_reset() {
        use eframe::egui::{Event, Modifiers, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let run = |events: Vec<Event>, cx: &RowCx, t: f64| -> Vec<(String, Rect)> {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 600.0))), time: Some(t), events, ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut on = true;
                    srow(ui, cx, "opacity", |ui| {
                        ui.label("SLIDER");
                    });
                    srow(ui, cx, "reduce_transparency", |ui| {
                        crate::style::toggle(ui, &mut on);
                    });
                    srow(ui, cx, "animations", |ui| {
                        ui.label("HIDDEN");
                    });
                });
            });
            out.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some((t.galley.text().to_string(), t.visual_bounding_rect())) } else { None }).collect()
        };
        let q = Query::new("transp");
        let f = search(&q, Lang::De);
        let cx = RowCx::new(Lang::De, Page::AppearanceTransparency, q, Some(&f), HashSet::from(["opacity"]), false, false);
        let texts = run(vec![], &cx, 0.1);
        let has = |s: &str| texts.iter().any(|(t, _)| t.contains(s));
        assert!(has("Deckkraft") && has("Stichwort: transparency") && has("SLIDER") && has("Transparenz reduzieren"));
        assert!(!has("HIDDEN") && !has("Animationen"), "ohne Treffer ausgeblendet");
        // ↺ rechts in der Zeile „Deckkraft“ (geändert)
        let row = texts.iter().find(|(t, _)| t == "Deckkraft").unwrap().1;
        let pos = Pos2::new(700.0 - 8.0 - RESET_W / 2.0, row.center().y + 4.0);
        let mut t = 0.2;
        for e in [Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }, Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }] {
            t += 0.1;
            run(vec![e], &cx, t);
        }
        assert_eq!(cx.reset.get(), Some("opacity"));
    }
}

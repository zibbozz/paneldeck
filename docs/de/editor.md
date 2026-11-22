[← README](../../README.de.md) · [English](../en/editor.md) · **Deutsch**

# Editor

Der Reiter **Editor** ist ein einfacher Texteditor in Paneldeck, unabhängig von den Terminals: eigener Ordnerbaum, eigene Tabs und Teilungen. Geöffnet wird er über den Reiter **Editor** in der Titelleiste oder mit **Alt+Umschalt+3** (änderbar unter *Einstellungen → Tastenkürzel*). *(Seit 3.0.0.)*

![Editor-Reiter mit Ordnerbaum, zwei Teilungen mit Tabs und Statusleiste](../images/de/editor.png)

## Dateien und Ordner
- **Ordner öffnen …** (**Strg+Umschalt+O**) zeigt einen Ordner links als Baum (**Ordnerbaum ein/aus** blendet ihn aus); **Ordner schließen** schließt ihn wieder. Im Baum öffnet ein Klick die Datei in der fokussierten Teilung; das Kontextmenü hat *Rechts daneben öffnen*, *Pfad kopieren*, *Im Explorer anzeigen* und *Als Wurzel öffnen*.
- **Datei öffnen …**, **Neue Datei** und **Zuletzt geöffnet** (*Zuletzt geöffnete Dateien* / *Zuletzt geöffnete Ordner*, *Liste leeren*).
- **Explorer:** Mit dem Schalter *In Paneldeck-Editor öffnen* unter *Einstellungen → Fenster* öffnet ein Rechtsklick auf jede Datei sie hier ([Windows-Integration](windows-integration.md#in-paneldeck-editor-öffnen)); von der Kommandozeile: [`paneldeck.exe --edit <datei>`](command-line.md#--edit-datei).
- Paneldeck merkt sich den offenen Ordner, die Tabs (mit Cursor und Scrollposition), die Teilungen und die Listen „zuletzt geöffnet“ (`editor-state.json` im Datenordner).

## Tabs und Teilungen
- Jede Teilung hat **eine eigene Tableiste**; Tabs lassen sich zwischen Teilungen ziehen. *Schließen*, *Andere schließen*, *Alle schließen* stehen im Kontextmenü des Tabs.
- **Rechts teilen** / **Unten teilen** wie im Reiter Terminals, Größen per Ziehen; **Strg+Alt+Pfeiltasten** wechseln den Fokus zwischen Teilungen.
- **Dieselbe Datei in zwei Teilungen** ist ein Dokument: Änderungen erscheinen sofort in beiden, jede Teilung behält ihre eigene Scrollposition und ihre Cursor.

## Bearbeiten
- **Mehrere Cursor:** Strg+Alt+Hoch/Runter setzt einen Cursor darüber/darunter, **Strg+D** wählt das nächste Vorkommen, **Strg+Umschalt+L** alle Vorkommen, **Alt+Ziehen** wählt eine Spalte; Esc kehrt zu einem Cursor zurück.
- **Strg+L** wählt die Zeile; Zeilen verschieben, verdoppeln und löschen; Tab/Umschalt+Tab rücken ein und aus.
- **Gehe zu Zeile** (*Zeile[:Spalte]*).
- **Suchen und Ersetzen** in der offenen Datei: *Groß-/Kleinschreibung beachten*, *Nur ganze Wörter*, *Regulärer Ausdruck* (Syntax der Rust-Crate `regex`; im Ersatztext `$1`, `${name}`), *Ersetzen*, *Alle ersetzen*; Enter / Umschalt+Enter springen zum nächsten / vorigen Treffer.
- **Minimap** und Zeilenumbruch (beides aus bei [großen Dateien](#große-dateien)).
- Rückgängig/Wiederholen je Dokument; Änderungen eines Agenten sind eigene Rückgängig-Schritte.

![Suchleiste mit regulärem Ausdruck und Ersetzen](../images/de/editor-find.png)

## Kodierung, Zeilenenden, Einrückung
- Beim Öffnen erkennt Paneldeck die **Kodierung** (UTF-8 mit oder ohne BOM, UTF-16 LE/BE, Windows-1252) und behält die **Zeilenenden** wie vorgefunden (LF oder CRLF; gemischte Dateien behalten die Mehrheit, mit Hinweis). Beides steht in der Statusleiste und lässt sich dort umschalten (*Kodierung ändern (beim nächsten Speichern)*, *Zeilenende ändern*). Ein Zeichen, das nicht in die gewählte Kodierung passt, hält das Speichern mit einer Meldung an – die Datei bleibt unverändert.
- Die **Einrückung** (Tabs oder Leerzeichen) wird erkannt; **`.editorconfig`** geht vor (`indent_style`, `indent_size`/`tab_width`, `end_of_line`, `charset`, `trim_trailing_whitespace`, `insert_final_newline`).

## Speichern und Hot Exit
- **Strg+S** speichert. Gespeichert wird **atomar**: Paneldeck schreibt eine temporäre Datei neben die Datei und ersetzt sie dann – ein Absturz hinterlässt nie eine halb geschriebene Datei. Schreibgeschützte Dateien werden nicht überschrieben.
- **Automatisch speichern** (standardmäßig aus) speichert vorhandene Dateien nach kurzer Verzögerung ([Einstellungen](#einstellungen)); unbenannte Tabs nie.
- **Hot Exit:** Ungespeicherte Änderungen überstehen Schließen und Neustart von Paneldeck – sie liegen als reiner Text im Datenordner (`hot-exit\`) und werden gelöscht, sobald der Tab gespeichert oder geschlossen ist. *Einstellungen → Daten & Sicherung → Aufräumen* zeigt sie als *Editor: ungespeicherte Puffer*.

## Auf der Platte geänderte Dateien
Paneldeck bemerkt, wenn ein anderes Programm eine offene Datei ändert (z. B. Claude Code oder ein Agent): Es prüft etwa einmal pro Sekunde Größe und Änderungszeit und reagiert erst, wenn sich die Datei nicht mehr ändert.
- **Ohne eigene Änderungen** wird die Datei still neu geladen, mit kurzem Hinweis (*von der Platte neu geladen*).
- **Mit ungespeicherten Änderungen** bietet eine Infoleiste **Meine behalten**, **Von der Platte laden** und **Unterschied anzeigen**.
- Eine **gelöschte** Datei behält ihren Text im Editor und gilt als ungespeichert; Speichern legt sie neu an.

![Infoleiste „auf der Platte geändert“ mit Meine behalten, Von der Platte laden und Unterschied anzeigen](../images/de/editor-disk-changed.png)

## Suche in Dateien
**Strg+Umschalt+F** öffnet im Editor-Reiter das Fenster *In Dateien suchen*; es durchsucht alle Dateien des offenen Ordners. **F4** / **Umschalt+F4** springen zum nächsten / vorigen Treffer.
- beachtet **`.gitignore`** (auch verschachtelte und die der Elternordner), `.git/info/exclude`, die globale Git-Ausschlussdatei und `.ignore` – auch außerhalb eines Git-Repositorys; **Ignorierte einbeziehen** schaltet das für eine Suche ab (die Vorgabe steht in den Einstellungen);
- deine eigenen **Ausschlussmuster** (*Einstellungen → Editor*, Syntax wie `.gitignore`, z. B. `node_modules`, `*.min.js`, `dist/`) gelten immer; `.git`, `.hg` und `.svn` werden nie durchsucht;
- überspringt **Binärdateien** und Dateien über der Schwelle für große Dateien (die Zusammenfassung zählt sie);
- *Groß-/Kleinschreibung beachten*, *Nur ganze Wörter*, *Regulärer Ausdruck*; gesucht wird zeilenweise;
- die Treffer erscheinen schon während der Suche, nach Dateien gruppiert mit Zeilennummern und hervorgehobenen Stellen; **Abbrechen** hält sie an. Ein Klick öffnet die Datei an der Fundstelle. Höchstens 20 000 Treffer in 5 000 Dateien (1 000 Zeilen je Datei) – dann die Suche eingrenzen.

![Suche in Dateien mit den Treffern nach Dateien gruppiert](../images/de/editor-search.png)

## Große Dateien
Dateien ab **20 MB** (*Einstellungen → Editor → Große Dateien ab*) öffnen im Modus für große Dateien: Der Editor bleibt auch bei mehreren hundert Megabyte flüssig, aber **Minimap und Zeilenumbruch sind aus** (später auch Syntaxfarben), und eine Infoleiste sagt das. Die Suche in Dateien überspringt sie; Agenten können sie lesen, aber nicht bearbeiten.

## Einstellungen
*Einstellungen → Editor*:

| Einstellung | Standard | Bedeutung |
|---|---|---|
| Schrift | die Terminalschrift | Schrift des Editors (eingebaute Cascadia Mono / Hack oder jede installierte Festbreitenschrift) |
| Große Dateien ab | 20 MB | Schwelle des Modus für große Dateien |
| Automatisch speichern / Verzögerung | aus / 1 s | vorhandene Dateien nach der letzten Änderung automatisch speichern |
| Ignorierte einbeziehen | aus | Vorgabe des Such-Schalters |
| Ausschlussmuster | – | eines je Zeile, Syntax wie `.gitignore` |
| **Agenten** | alle aus | siehe [Agenten](#agenten) |

Gespeichert in `settings.json` als `editor` ([Datenformat](data-and-backups.md#neue-felder-in-300)).

![Einstellungen, Reiter Editor](../images/de/editor-settings.png)

## Agenten
Agenten (Claude Code und andere über die [Agenten-Schnittstelle](agent-interface.md#editor) sowie [Chats mit lokalen Modellen](chat.md)) können den Editor benutzen:
- **Lesen** (`editor_list`, `editor_read`) geht ab der Stufe *Nur lesen* – auch ungespeicherte Änderungen.
- Drei Schalter unter *Einstellungen → Editor → Agenten*, **alle standardmäßig aus** und nur bei der Stufe **Steuern** wirksam (für Chats: die Stufe des Modells):
  - **Agenten dürfen Dateien öffnen** – in einem neuen Tab; nur lokale Pfade, nie Netzwerk-/UNC-Pfade (`\\server\freigabe`, `\\wsl.localhost\…`, Netzlaufwerke) oder Geräte. Ein Agent darf auch einen unbenannten Tab mit Text öffnen.
  - **Agenten dürfen offene Dateien bearbeiten** – die Änderung bleibt **ungespeichert**, ist als Änderung des Agenten markiert (*Von einem Agenten geöffnet oder geändert*) und lässt sich rückgängig machen.
  - **Agenten dürfen speichern** – nur Dateien, die offen sind und schon existieren; unbenannte Tabs speicherst nur du.
- Agenten nehmen nie den Tastaturfokus: Eine Datei, die ein Agent öffnet oder zeigt, wird nicht fokussiert – was du gerade tippst, landet nie aus Versehen darin.
- Diese Regeln gelten für den Textmodus und den späteren Hex-Modus gleichermaßen.

## Noch nicht
Syntaxfarben, *Gehe zu Symbol*, Sprachserver und Formatierung kommen mit 3.1.0, ein Hex-Modus mit 3.2.0.

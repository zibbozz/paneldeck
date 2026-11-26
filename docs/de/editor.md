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

## Syntaxfarben
*(Seit 3.1.0.)* Jedes Dokument hat einen **Modus**, rechts in der Statusleiste neben der **Sprache**:
- **Text** – der schlichte Editor. **Das ist der Standard für jede Datei**, bis du Syntax einschaltest.
- **Syntax** – die Sprache wird erkannt und der Text eingefärbt (Kommentare, Zeichenketten, Zahlen, Schlüsselwörter, Typen, Funktionen, Tags, Präprozessor-Zeilen …). Die Farben kommen aus dem aktiven Theme (Farben `code_*`, im [Theme-Editor](themes.md#oberflächenfarben) änderbar), helle und dunkle Themes passen also gleichermaßen.
- *Smart* (Sprachserver) kommt in einer späteren Version; es steht ausgegraut da.

Einschalten:
- **Für eine Datei:** Klick auf den Modus in der Statusleiste → *Diese Datei: Syntax*. Die Wahl wird mit dem Tab gemerkt.
- **Für alle Dateien einer Sprache:** im selben Menü *Alle Dateien: Rust → Syntax* oder unter *Einstellungen → Editor → Sprachen → Modus je Sprache*.
- **Für alle Dateien:** *Standard für alle übrigen Dateien → Syntax* im Menü oder *Einstellungen → Editor → Standard-Modus*.

**Spracherkennung:** zuerst deine eigenen Zuordnungen (*Einstellungen → Editor → Eigene Zuordnungen*, z. B. `*.tpl` → HTML oder `Jenkinsfile` → Groovy), dann der Dateiname (`Makefile`, `CMakeLists.txt`, `Dockerfile`, `Gemfile` …) und die Endung, dann eine Shebang-Zeile (`#!/usr/bin/env python3`) oder eine Modeline (`vim: ft=yaml`, `-*- mode: ruby -*-`). Ein Klick auf die Sprache in der Statusleiste wählt für diese Datei eine andere (Tippen filtert, Enter nimmt den ersten Treffer; *Automatisch erkennen* stellt zurück). Die Wahl wird mit dem Tab gemerkt.

**Sprachen:** AutoIt, Batch, BibTeX, C, C#, C++, CMake, CSS, Diff, Dockerfile, Go, HTML, INI, Inno Setup, Java, JavaScript, JSON, Kotlin, LaTeX, Less, Lua, Makefile, Markdown, NSIS, Pascal / Delphi, Perl, PHP, PowerShell, Python, Ruby, Rust, SCSS, Shell (Bash), SQL, Swift, TeX, TOML, TypeScript, TSX/JSX, Visual Basic (VB.NET, VBA, VBScript), XML, YAML – dazu alle weiteren Grammatiken aus [bat](https://github.com/sharkdp/bat) (Haskell, Kotlin, Terraform, Zig, …; in der Auswahl unter *Weitere Grammatiken*). AutoIt, Inno Setup, Visual Basic und PowerShell nutzen kleine eigene Grammatiken von Paneldeck.

Das Einfärben arbeitet schrittweise und im Hintergrund, Tippen und Rollen warten also nie darauf: Nur die sichtbaren Zeilen (und ein paar darum herum) werden eingefärbt, eine Änderung färbt erst ab der geänderten Zeile neu, und alle Teilungen einer Datei teilen sich die Arbeit. Zeilen über 3 000 Zeichen bleiben ohne Farben. Im [Modus für große Dateien](#große-dateien) bleiben die Farben aus.

![Editor-Reiter im Syntax-Modus: eine Rust- und eine Markdown-Datei mit Syntaxfarben, die Statusleiste zeigt Modus und Sprache](../images/de/editor-syntax.png)

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
- **Mit ungespeicherten Änderungen** bietet eine Infoleiste **Meine behalten**, **Von der Platte laden** und **Unterschied anzeigen** (öffnet einen [Vergleichs-Reiter](#vergleich-und-änderungsmarken): gespeicherte Datei ↔ Editor).
- Eine **gelöschte** Datei behält ihren Text im Editor und gilt als ungespeichert; Speichern legt sie neu an.

![Infoleiste „auf der Platte geändert“ mit Meine behalten, Von der Platte laden und Unterschied anzeigen](../images/de/editor-disk-changed.png)

## Vergleich und Änderungsmarken
*Neu in 3.2.0.* Der Editor vergleicht Dateien wie ein git diff – angelehnt an Sublime Text und Sublime Merge, **alles offline** (nur das lokale `git`).

**Vergleich öffnen** – er öffnet sich als eigener Reiter (*a.json ↔ b.json*):
- Kontextmenü eines Reiters: **Vergleichen mit …** → *Gespeicherter Datei*, *HEAD*, *Index (gestagt)*, *Revision …*, *Zwischenablage*, *Datei …* oder einer anderen offenen Datei;
- Ordnerbaum: **Zum Vergleichen auswählen** an einer Datei, dann **Vergleichen mit „a.json“** an einer anderen – oder zwei Dateien mit **Strg+Klick** markieren und **Markierte vergleichen** wählen;
- Befehlspalette (**Strg+Umschalt+P** im Editor-Reiter): *Vergleichen mit Datei …*, *… gespeicherter Datei*, *… HEAD*, *… dem Index*, *… Revision …*, *… der Zwischenablage*;
- **Unterschied anzeigen** in der Infoleiste einer auf der Platte geänderten Datei.

**Der Vergleichs-Reiter**
- Standardmäßig **nebeneinander**, automatisch **untereinander**, wenn die Teilung schmaler als ~700 px ist; die Werkzeugleiste schaltet zwischen *Automatisch*, *Nebeneinander* und *Untereinander*. Beide Seiten rollen gemeinsam; Zeilen, die es nur auf einer Seite gibt, bekommen auf der anderen eine **schraffierte Füllzeile**.
- Änderungen innerhalb einer Zeile sind **wortweise** hervorgehoben (kurze Wörter bis aufs Zeichen); Zeilen mit sehr vielen Änderungen sind als Ganzes markiert. Syntaxfarben und Zeilennummern auf beiden Seiten (untereinander: alte und neue Spalte); der Zeilenumbruch folgt dem Editor (**Alt+Z**), die Zeilen bleiben auf gleicher Höhe.
- Unveränderte Bereiche sind mit 3 Kontextzeilen **eingeklappt** (Zahl in der Werkzeugleiste, Vorgabe in den Einstellungen); **↓ 20**, **↑ 20** und **Alle zeigen** klappen sie auf.
- **Blockpfeile** zwischen den Seiten übernehmen einen Block auf die andere Seite (**→** / **←**, auch **Alt+→** / **Alt+←** für den aktuellen Block). Eine Seite ist bearbeitbar, wenn sie eine Datei ist; git-Stände, die gespeicherte Datei und die Zwischenablage sind schreibgeschützt. Jeder Pfeil ist **ein Rückgängig-Schritt** im selben Dokument wie im normalen Reiter (gemeinsames Rückgängig, Ungespeichert-Punkt, **Strg+S** speichert die fokussierte Seite). **⇄** tauscht die Seiten.
- **F7** / **Umschalt+F7** springen zur nächsten / vorigen Änderung (einstellbar unter *Einstellungen → Tastenkürzel*). Ein Doppelklick auf eine Zeile öffnet sie im Reiter der Datei.
- Der Vergleich folgt dem Tippen nach etwa 250 ms; git-Stände werden nach dem Speichern, wenn das Fenster den Fokus bekommt und wenn sich `.git/index` oder `HEAD` ändern, neu gelesen.
- **Leerraum ignorieren** (standardmäßig aus). Unterschiede bei **Zeilenenden, Kodierung/BOM und Zeilenende am Schluss** stehen in einer Infoleiste über dem Vergleich, nicht als geänderte Zeilen.
- **Revision …** vergleicht mit HEAD, dem Index, einem Zweig, einem Tag oder einem der letzten 50 Commits der Datei (mit Suche) oder mit einer eingetippten Revision (wird vorher geprüft).
- Menü **⋯**: *Diff kopieren*, *Block kopieren*, *Als Patch speichern …* (git-kompatibel, `git apply` nimmt ihn), *Alle Zeilen zeigen*.
- Große Dateien werden im Hintergrund verglichen, mit Fortschritt und **Abbrechen**; an der Zeitgrenze wird das Ergebnis gröber (ein Hinweis sagt es). Über der Grenze für große Dateien und bei **Binärdateien** zeigt der Reiter stattdessen Größe und **SHA-256** beider Seiten.
- Vergleichs-Reiter kommen nach einem Neustart wieder (außer denen mit der Zwischenablage).

![Vergleichs-Reiter nebeneinander mit Änderungen innerhalb der Zeilen, Füllzeilen und Blockpfeilen](../images/de/editor-diff.png)

**Änderungsmarken** (*Ansicht → Änderungsmarken*, *Einstellungen → Editor*, **standardmäßig aus**) markieren Zeilen am linken Rand des normalen Editors: ein dünner Balken für **hinzugefügte** und **geänderte** Zeilen, ein Dreieck, wo Zeilen **entfernt** wurden, dazu Marken in der Bildlaufleiste und in der Minimap.
- In einem git-Repository vergleichen die Marken **mit dem Index und HEAD zugleich**: **nicht gestagte** Änderungen (Datei ↔ Index) in Grün/Gelb/Rot, **gestagte** Änderungen (Index ↔ HEAD) in Türkis/Blau/Violett. Außerhalb eines Repositorys – oder mit *Grundlage der Marken: Gespeicherte Datei* – zeigen sie die ungespeicherten Änderungen gegenüber der Datei auf der Platte.
- Ein **Klick auf eine Marke** öffnet ein kleines Fenster unter der Zeile: die alten Zeilen, **Zurücksetzen** (ein Rückgängig-Schritt), vorige/nächste Änderung und **Im Vergleich öffnen**.
- **F7** / **Umschalt+F7** springen zwischen den Marken. In großen Dateien gibt es keine Marken.
- Die Farben sind Farben des Themes (*Editor: Vergleich und Änderungsmarken* im Theme-Editor).

![Änderungsmarken am Rand mit dem Fenster einer Marke: alte Zeilen, Zurücksetzen, vorige/nächste, Im Vergleich öffnen](../images/de/editor-gutter.png)

## Suche in Dateien
**Strg+Umschalt+F** öffnet im Editor-Reiter das Fenster *In Dateien suchen*; es durchsucht alle Dateien des offenen Ordners. **F4** / **Umschalt+F4** springen zum nächsten / vorigen Treffer.
- beachtet **`.gitignore`** (auch verschachtelte und die der Elternordner), `.git/info/exclude`, die globale Git-Ausschlussdatei und `.ignore` – auch außerhalb eines Git-Repositorys; **Ignorierte einbeziehen** schaltet das für eine Suche ab (die Vorgabe steht in den Einstellungen);
- deine eigenen **Ausschlussmuster** (*Einstellungen → Editor*, Syntax wie `.gitignore`, z. B. `node_modules`, `*.min.js`, `dist/`) gelten immer; `.git`, `.hg` und `.svn` werden nie durchsucht;
- überspringt **Binärdateien** und Dateien über der Schwelle für große Dateien (die Zusammenfassung zählt sie);
- *Groß-/Kleinschreibung beachten*, *Nur ganze Wörter*, *Regulärer Ausdruck*; gesucht wird zeilenweise;
- die Treffer erscheinen schon während der Suche, nach Dateien gruppiert mit Zeilennummern und hervorgehobenen Stellen; **Abbrechen** hält sie an. Ein Klick öffnet die Datei an der Fundstelle. Höchstens 20 000 Treffer in 5 000 Dateien (1 000 Zeilen je Datei) – dann die Suche eingrenzen.

![Suche in Dateien mit den Treffern nach Dateien gruppiert](../images/de/editor-search.png)

## Große Dateien
Dateien ab **20 MB** (*Einstellungen → Editor → Große Dateien ab*) öffnen im Modus für große Dateien: Der Editor bleibt auch bei mehreren hundert Megabyte flüssig, aber **Minimap, Zeilenumbruch und Syntaxfarben sind aus**, und eine Infoleiste sagt das. Die Suche in Dateien überspringt sie; Agenten können sie lesen, aber nicht bearbeiten.

## Einstellungen
*Einstellungen → Editor*:

| Einstellung | Standard | Bedeutung |
|---|---|---|
| Schrift | die Terminalschrift | Schrift des Editors (eingebaute Cascadia Mono / Hack oder jede installierte Festbreitenschrift) |
| Große Dateien ab | 20 MB | Schwelle des Modus für große Dateien |
| Automatisch speichern / Verzögerung | aus / 1 s | vorhandene Dateien nach der letzten Änderung automatisch speichern |
| Ignorierte einbeziehen | aus | Vorgabe des Such-Schalters |
| Ausschlussmuster | – | eines je Zeile, Syntax wie `.gitignore` |
| Standard-Modus | Text | Modus von Dateien, für deren Sprache nichts eingestellt ist ([Syntaxfarben](#syntaxfarben)) |
| Modus je Sprache | Standard | Text oder Syntax für alle Dateien einer Sprache |
| Eigene Zuordnungen | – | Dateiname oder Endung → Sprache, vor der eingebauten Erkennung |
| Änderungsmarken am Rand | aus | [Änderungsmarken](#vergleich-und-änderungsmarken) |
| Grundlage der Marken | git (Index und HEAD) | oder die gespeicherte Datei |
| Darstellung im Vergleich | Automatisch | nebeneinander, unter ~700 px untereinander |
| Kontextzeilen | 3 | unveränderte Zeilen um jede Änderung im Vergleich |
| Leerraum ignorieren | aus | Vorgabe für neue Vergleiche |
| **Agenten** | alle aus | siehe [Agenten](#agenten) |

Gespeichert in `settings.json` als `editor` ([Datenformat](data-and-backups.md#neue-felder-in-300), [neu in 3.1.0](data-and-backups.md#neue-felder-in-310); neu in 3.2.0: `change_markers`, `marker_base`, `diff_context`, `diff_ignore_whitespace`, `diff_layout`).

![Einstellungen, Reiter Editor](../images/de/editor-settings.png)

## Agenten
Agenten (Claude Code und andere über die [Agenten-Schnittstelle](agent-interface.md#editor) sowie [Chats mit lokalen Modellen](chat.md)) können den Editor benutzen:
- **Lesen** (`editor_list`, `editor_read`) geht ab der Stufe *Nur lesen* – auch ungespeicherte Änderungen. **`editor_diff`** (seit 3.2.0, Stufe *Nur lesen*, nur externe Agenten) vergleicht ein Dokument mit seiner gespeicherten Datei, git HEAD/Index oder einem anderen offenen Dokument.
- Drei Schalter unter *Einstellungen → Editor → Agenten*, **alle standardmäßig aus** und nur bei der Stufe **Steuern** wirksam (für Chats: die Stufe des Modells):
  - **Agenten dürfen Dateien öffnen** – in einem neuen Tab; nur lokale Pfade, nie Netzwerk-/UNC-Pfade (`\\server\freigabe`, `\\wsl.localhost\…`, Netzlaufwerke) oder Geräte. Ein Agent darf auch einen unbenannten Tab mit Text öffnen.
  - **Agenten dürfen offene Dateien bearbeiten** – die Änderung bleibt **ungespeichert**, ist als Änderung des Agenten markiert (*Von einem Agenten geöffnet oder geändert*) und lässt sich rückgängig machen.
  - **Agenten dürfen speichern** – nur Dateien, die offen sind und schon existieren; unbenannte Tabs speicherst nur du.
- Agenten nehmen nie den Tastaturfokus: Eine Datei, die ein Agent öffnet oder zeigt, wird nicht fokussiert – was du gerade tippst, landet nie aus Versehen darin.
- Diese Regeln gelten für den Textmodus und den späteren Hex-Modus gleichermaßen.

## Noch nicht
Sprachserver (Modus *Smart*: *Gehe zu Symbol*, Formatierung, Diagnosen) und KI-Vervollständigung kommen mit 3.3.0, das Sprachpaket mit 3.4.0, Ausführen von Dateien und Projekten mit 3.5.0, ein Hex-Modus mit 3.6.0.

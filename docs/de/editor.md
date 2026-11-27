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

## Menüleiste
*(Seit 3.2.0.)* Jede Teilung hat über ihrer Tableiste eine schmale **Menüleiste**: **Datei · Bearbeiten · Ansicht · Format**. Die Befehle wirken auf den aktiven Tab dieser Teilung, die Tastenkürzel stehen rechts neben jedem Eintrag:
- **Datei:** Neue Datei, Datei öffnen …, Ordner öffnen …, Speichern, Speichern unter …, Alle speichern (alle ungespeicherten Dateien mit Namen; unbenannte Tabs brauchen *Speichern unter*), die [Aktionen beim Speichern](#aktionen-beim-speichern) (*Aktionen beim Speichern …*, *Ohne Aktionen speichern*, *Pausieren*), [Vergleichen mit …](#vergleich-und-änderungsmarken) (Datei, gespeicherte Datei, HEAD, Index, Revision, Zwischenablage), Reiter schließen.
- **Bearbeiten:** Rückgängig, Wiederholen, Ausschneiden, Kopieren, Einfügen, Alles markieren, Suchen …, Ersetzen …, In Dateien suchen …, Gehe zu Zeile …, Nächstes Vorkommen dazu, Alle Vorkommen markieren, Zeile markieren, Zeile löschen, Zeile nach oben/unten.
- **Ansicht:** Zeilenumbruch, Minimap, Ordnerbaum, Rechts teilen, Unten teilen, Modus *Text* / *Syntax*, Sprache (eingebaute Sprachen; alle Sprachen in der Statusleiste), [Änderungsmarken](#vergleich-und-änderungsmarken) und ihre Grundlage, Nächste/Vorige Änderung (**F7** / **Umschalt+F7**), Befehle (Palette). Im Vergleichs-Reiter gilt *Datei* der fokussierten Seite; *Bearbeiten* und *Format* sind ausgegraut.
- **Format:** die [Befehle zum Formatieren](#json-und-xml-formatieren) und *JSON/XML beim Speichern formatieren*.

Einträge, die gerade nicht passen, sind **ausgegraut**; beim Überfahren steht, warum (z. B. *Weder JSON noch XML* bei einer Textdatei, *Nichts rückgängig zu machen*). In einer schmalen Teilung klappen die vier Menüs zu einem **Menü ▾** zusammen. Die Menüleiste lässt sich unter *Einstellungen → Editor → Menüleiste* abschalten (standardmäßig an). Die Befehls-Palette (**Strg+Umschalt+P**) geht auch im Reiter Editor und listet die Format-Befehle als *Format: …*.

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

## JSON und XML formatieren
*(Seit 3.2.0.)* Das Menü **Format** hat einen eingebauten Formatierer für JSON und XML – er arbeitet **offline**, nichts verlässt deinen Rechner:

| Befehl | Kürzel | Was er tut |
|---|---|---|
| JSON formatieren / XML formatieren | **Alt+Umschalt+F** | hübsch einrücken mit der Einrückung des Dokuments (aus `.editorconfig` bzw. der eigenen Einrückung der Datei: Leerzeichen oder Tabs, Breite) |
| Minifizieren | **Alt+Umschalt+M** | Leerraum entfernen |
| Prüfen | **Alt+Umschalt+V** | nur prüfen (immer die ganze Datei): *Gültiges JSON* oder der Fehler mit Zeile und Spalte |
| JSON-Schlüssel sortieren | – | Objektschlüssel rekursiv alphabetisch sortieren, danach formatieren; nur JSON |

- **Ein Rückgängig-Schritt:** Das Ergebnis ersetzt das Dokument direkt; **Strg+Z** holt den vorigen Text zurück. Der Cursor bleibt auf seiner Zeile.
- **Auswahl:** Mit einer Auswahl wird nur der markierte Text formatiert (die Zeilen nach der ersten bekommen die Einrückung der Zeile, in der die Auswahl beginnt). Die Auswahl muss ein vollständiger Wert bzw. ein vollständiges Element sein – sonst kommt der Fehler des Prüfens, und nichts ändert sich.
- **Fehler:** Ungültiger Inhalt wird nie verändert. Eine Leiste zeigt *Ungültiges JSON in Zeile 3, Spalte 17: …* mit **Gehe zu**, das den Cursor an die Stelle setzt.
- **Große Dateien:** Formatiert wird im Hintergrund mit einer Fortschrittszeile (Prozent und **Abbrechen**); der Editor bleibt bedienbar, auch im [Modus für große Dateien](#große-dateien). Tippst du währenddessen, wird das Ergebnis nicht übernommen (noch einmal starten). Das Ergebnis kommt am Ende auf einmal ins Dokument; bei Dateien mit mehreren hundert MB hält der Editor dabei kurz an (etwa 1,5 s bei 200 MB), die Fortschrittszeile sagt es vorher.
- **Erkennung:** an der Endung (`.json`, `.jsonc`, `.code-workspace`, `.xml`, `.xsd`, `.xsl(t)`, `.svg`, `.csproj`, `.props`, `.targets`, `.resx`, `.plist`, `.xaml`, `.nuspec`, `.config` mit XML …) und an bekannten Dateinamen (`.babelrc`, `.eslintrc`, `composer.lock` …). Der Inhalt (`{`/`[` bzw. `<?xml`) zählt nur bei unbenannten Tabs und Dateien ohne Endung – `Cargo.toml`, eine `.ini` oder ein PowerShell-Skript gilt nie als JSON. Eine in der Statusleiste gewählte Sprache geht vor: *JSON* oder *XML* macht jede Datei formatierbar, jede andere Sprache schaltet das Formatieren für diese Datei ab.
- **JSONC** (JSON mit Kommentaren: `.jsonc`, `tsconfig.json`, `jsconfig.json`, `.vscode/*.json` oder wenn Kommentare vorkommen): Kommentare (`//`, `/* */`) und Kommas am Ende **bleiben** beim Formatieren erhalten; **Minifizieren** entfernt die Kommentare und fragt vorher.
- **XML wird behutsam formatiert:** Elemente werden eingerückt; Kommentare, CDATA, Processing Instructions und die DOCTYPE bleiben; gemischter Inhalt (Text und Tags, z. B. XHTML) und `xml:space="preserve"` bleiben unverändert; Attribute bleiben in einer Zeile.
- **Beim Speichern formatieren:** ein Schalter je Sprache – *JSON beim Speichern formatieren* / *XML beim Speichern formatieren* im Menü Format oder unter *Einstellungen → Editor* – **standardmäßig aus**. Beim Speichern werden nur Dateien bis 1 MB formatiert; größere, ungültige oder schreibgeschützte Dateien werden **unverändert gespeichert** (mit Hinweis in der Statusleiste). Scheitert das Speichern, wird das Formatieren zurückgenommen – das Dokument bleibt, wie es war. Automatisches Speichern und Speichern durch einen Agenten (`editor_save`) formatieren nicht; *Speichern unter* richtet sich nach dem neuen Dateinamen.
- Die Kürzel lassen sich unter *Einstellungen → Tastenkürzel* ändern (*Editor: JSON/XML formatieren* …). Sie sind bewusst nicht Strg+Alt+Buchstabe: Das ist auf vielen Tastaturen AltGr (AltGr+M tippt auf einer deutschen Tastatur µ). Mit mehreren Eingabesprachen schaltet Windows mit Alt+Umschalt eventuell die Sprache um – dafür gibt es eine zweite Belegung: **Strg+Umschalt+I** formatiert (*… (zweite Belegung)*; für Minifizieren und Prüfen ist die zweite Belegung leer und frei wählbar).
- Formatieren über Sprachserver (Modus *Smart*) kommt später zusätzlich.

![Editor mit Menüleiste: links eine formatierte JSON-Datei, rechts eine ungültige mit „Gehe zu“](../images/de/editor-format.png)

## Kodierung, Zeilenenden, Einrückung
- Beim Öffnen erkennt Paneldeck die **Kodierung** (UTF-8 mit oder ohne BOM, UTF-16 LE/BE, Windows-1252) und behält die **Zeilenenden** wie vorgefunden (LF oder CRLF; gemischte Dateien behalten die Mehrheit, mit Hinweis). Beides steht in der Statusleiste und lässt sich dort umschalten (*Kodierung ändern (beim nächsten Speichern)*, *Zeilenende ändern*). Ein Zeichen, das nicht in die gewählte Kodierung passt, hält das Speichern mit einer Meldung an – die Datei bleibt unverändert.
- Die **Einrückung** (Tabs oder Leerzeichen) wird erkannt; **`.editorconfig`** geht vor (`indent_style`, `indent_size`/`tab_width`, `end_of_line`, `charset`, `trim_trailing_whitespace`, `insert_final_newline`).

## Speichern und Hot Exit
- **Strg+S** speichert. Gespeichert wird **atomar**: Paneldeck schreibt eine temporäre Datei neben die Datei und ersetzt sie dann – ein Absturz hinterlässt nie eine halb geschriebene Datei. Schreibgeschützte Dateien werden nicht überschrieben.
- **Automatisch speichern** (standardmäßig aus) speichert vorhandene Dateien nach kurzer Verzögerung ([Einstellungen](#einstellungen)); unbenannte Tabs nie.
- **Hot Exit:** Ungespeicherte Änderungen überstehen Schließen und Neustart von Paneldeck – sie liegen als reiner Text im Datenordner (`hot-exit\`) und werden gelöscht, sobald der Tab gespeichert oder geschlossen ist. *Einstellungen → Daten & Sicherung → Aufräumen* zeigt sie als *Editor: ungespeicherte Puffer*.

## Aktionen beim Speichern
*(Seit 3.2.0.)* Regeln, die beim Speichern einer Datei im Editor-Reiter laufen – je **Datei**, je **Ordner** (mit allen Unterordnern) oder je **Muster**. Verwaltet werden sie unter *Einstellungen → Editor → Aktionen beim Speichern*; ein Rechtsklick auf eine Datei oder einen Ordner im Ordnerbaum (*Aktionen beim Speichern für diese Datei/diesen Ordner …*) oder *Datei → Aktionen beim Speichern …* öffnet den Regel-Dialog mit vorbelegtem Pfad.

![Regel-Dialog der Aktionen beim Speichern: Muster, Auslöser, Aktionen und Befehls-Optionen](../images/de/editor-on-save.png)

**Was eine Regel kann** – eine geordnete Liste von Aktionen:
- **Vor dem Schreiben** (schnell, ein Rückgängig-Schritt): *Leerraum am Zeilenende entfernen*, *Zeilenumbruch am Ende*, *Zeilenenden* (LF/CRLF), *Kodierung*, *Tabs → Leerzeichen* / *Leerzeichen → Tabs* (nur die Einrückung) und *Formatieren (JSON/XML)*.
- **Nach dem Schreiben**, in Listenreihenfolge: **Befehl ausführen** (z. B. `npx eslint --fix ${file}` oder ein Upload mit deinem eigenen `scp`/`rsync`), **Datei kopieren** in einen anderen lokalen Ordner und Paneldeck-Aktionen: **An Sitzung senden** (ein Befehl in ein Terminal – nur, wenn es an seinem Prompt wartet, nie bei offener Freigabe-Frage, nie mit geheimen Variablen; sonst übersprungen mit Hinweis), **Prozess neu starten** (oder eine ganze Gruppe der [Prozessliste](processes.md)) und **Benachrichtigung zeigen**. Ziele werden über ihre Kennung gemerkt – Umbenennen einer Sitzung oder eines Prozesses schadet nicht.
- Ein Fehler überspringt den Rest dieser Regel – außer die Aktion hat *Bei Fehler weiter*. **Die Datei wird immer gespeichert**, auch wenn eine Aktion scheitert.

**Welche Dateien** – Muster in der Syntax von `.gitignore` relativ zum Regelordner (`*.json`, `src/**/*.ts`, `!dist/**` nimmt wieder aus); unter Windows ohne Groß-/Kleinschreibung, und `\` zählt wie `/` als Trenner. Passen mehrere Regeln, laufen alle **in Listenreihenfolge** (ziehen zum Sortieren); *Keine weiteren Regeln danach* hört nach einer gelaufenen Regel auf, *Regeln übergeordneter Ordner nicht anwenden* lässt die Regeln der Ordner darüber aus. Dieselbe eingebaute Aktion zweier Regeln läuft einmal. Eine Regel, deren Ordner fehlt, ist als *Ordner fehlt* markiert. Regeln gelten auch für Dateien von Netzlaufwerken.

**Wann** – *Speichern*, *Speichern unter*, *Alle speichern* und Speichern in der Rückfrage beim Schließen lösen immer aus. **Automatisches Speichern** und **Speichern durch Agenten** lösen nur eine Regel aus, die es erlaubt (*Auch beim automatischen Speichern*, *Auch wenn ein Agent speichert*; beides standardmäßig aus – das gilt auch für die eingebauten Aktionen; beim automatischen Speichern behält *Leerraum am Zeilenende entfernen* den Leerraum in Zeilen mit einem Cursor). *Ohne Aktionen speichern* (Menü *Datei*, Befehls-Palette oder ein eigenes Kürzel unter *Einstellungen → Tastenkürzel*) speichert einmal ohne Regeln; *Pausieren* hält alle Regeln bis zum Neustart von Paneldeck an; der Hauptschalter schaltet sie dauerhaft aus. Hot Exit speichert nie und löst nie aus.

**Variablen** in Befehlen und Texten: `${file}`, `${fileDirname}`, `${fileBasename}`, `${fileBasenameNoExtension}`, `${fileExtname}`, `${relativeFile}` (relativ zum Regelordner), `${ruleFolder}`, `${workspaceFolder}` (der geöffnete Ordner des Editors, sonst der Regelordner) und `${env:NAME}`. Paneldeck **quotet jeden Wert für die gewählte Shell** – Namen mit `% & ' $`, Backticks, `;` oder Leerzeichen bleiben ein Argument. Eine Variable **innerhalb** einer Zeichenkette in Anführungszeichen (`"--out=${file}"`) wird abgelehnt – der Befehl läuft nicht, Dialog und *Für Datei testen …* sagen, warum –, weil ein Dateiname sonst aus den Anführungszeichen ausbrechen könnte; Anführungszeichen, die genau eine Variable umschließen (`"${file}"`), sind in Ordnung und fallen einfach weg. Dieselben Werte stehen auch in der Umgebung: `PANELDECK_FILE`, `PANELDECK_FILE_DIRNAME`, `PANELDECK_FILE_BASENAME`, `PANELDECK_FILE_BASENAME_NO_EXTENSION`, `PANELDECK_FILE_EXTNAME`, `PANELDECK_RELATIVE_FILE`, `PANELDECK_RULE_FOLDER` und `PANELDECK_WORKSPACE_FOLDER` – sicher, wenn ein Skript sie aus der Umgebung liest (`$env:PANELDECK_FILE`, `"$PANELDECK_FILE"`); in **cmd** immer `"%PANELDECK_FILE%"` mit Anführungszeichen schreiben (cmd setzt den Wert ungequotet ein; der Dialog warnt). WSL bekommt sie und die eigenen Variablen der Regel über `WSLENV`. In Git Bash und WSL werden Pfade umgerechnet (`/c/…`, `/mnt/c/…`; eine Datei in `\\wsl.localhost\<Distribution>\…` wird in dieser Distribution zu ihrem Linux-Pfad, eine andere Distribution ist ein Fehler). Ordner enden nie mit `\`. Unbekannte `${…}` bleiben stehen.

**Wie Befehle laufen** – wie in der [Prozessliste](processes.md): verborgen in einer Pseudokonsole in einem eigenen Job Object, mit einer erkannten Shell (nie ein Administrator-Profil), eigenen Umgebungsvariablen (Werte verschlüsselt gespeichert, nie angezeigt, nie exportiert), dem Regelordner als Arbeitsordner (Datei-Regeln: der Ordner der Datei; änderbar). *In einem Terminal zeigen* führt den Befehl stattdessen sichtbar in einem Terminal in der Seitenleiste aus (z. B. für eine Passwortabfrage). **Zeitlimit** 60 s (1 s–60 min je Regel): Strg+C, nach 5 s der ganze Prozessbaum, Status *Zeitlimit*. Wird während eines Laufs erneut gespeichert, wartet genau ein weiterer Lauf mit dem neuesten Stand; *Alle speichern* macht einen Lauf je Datei, höchstens 2 gleichzeitig (*Parallel*, 1–8). Schreibt ein Befehl die Datei um, lädt der Editor sie wie jede andere Änderung auf der Platte neu (bzw. zeigt die Infoleiste, wenn du ungespeicherte Änderungen hast).

**Status und Ausgabe** – die Statusleiste zeigt *⟳ 2 Aktionen* während eines Laufs, danach *✓* und nach einem Fehler ein rotes *✗ Aktionen beim Speichern* (Klick: Ausgabe). Der Reiter der Datei bekommt einen **roten Punkt** bis zum nächsten erfolgreichen Lauf. Fehler zeigen außerdem einen roten Hinweis mit *Ausgabe zeigen* (und eine Windows-Benachrichtigung, wenn Paneldeck nicht im Vordergrund ist); Erfolg steht nur in der Statusleiste, außer die Regel hat *Auch bei Erfolg benachrichtigen*. Das **Ausgabefenster** behält die **letzten 100 Läufe** im Speicher (Zeit, Datei, Regel, Befehl, Exit-Code, Dauer, bis zu 2000 Zeilen Ausgabe), mit *Abbrechen* für einen laufenden Befehl; *Ausgabe in eine Protokolldatei* schreibt zusätzlich eine rotierende Datei nach `save-action-logs\`. Beim Beenden von Paneldeck mit laufenden oder wartenden Aktionen kommt eine Rückfrage wie bei der Prozessliste (*Alle stoppen* verwirft auch wartende Läufe).

**Testen** – *Für Datei testen …* zeigt, welche Regeln zu einer Datei passen (und warum andere übersprungen werden) und die fertigen Befehlszeilen, ohne etwas auszuführen; *Jetzt ausführen* führt die Aktionen nach dem Schreiben für die aktuelle Datei aus; *Regeln für diese Datei* im Kontextmenü des Baums zeigt dasselbe für eine Datei.

**Weitergeben** – *Gewählte exportieren …* schreibt die gewählten Regeln in eine JSON-Datei **ohne geheime Werte**; *Importieren …* zeigt vorher alle Befehle, importierte Regeln kommen **ausgeschaltet** an.

**Kopieren** – Zielordner plus der Pfad relativ zum Regelordner (oder nur der Dateiname), fehlende Ordner werden angelegt, eine vorhandene Datei wird ohne Rückfrage atomar ersetzt. Das Ziel muss **lokal** sein: Netzwerk-/UNC-Pfade und Netzlaufwerke werden abgelehnt – Paneldeck selbst greift nie aufs Netz zu, Uploads nur über deine eigenen Befehle. Ein Ziel im Regelordner (oder die Datei selbst) wird ebenfalls abgelehnt – geprüft am fertigen Ziel, nachdem `..`, Links und Junctions aufgelöst sind.

Agenten können keine Regeln anlegen oder ändern (Dateien im Datenordner können sie auch nicht öffnen, ändern oder speichern); sie können sie mit `list_save_actions` auflisten ([Agenten-Schnittstelle](agent-interface.md#editor)). Gespeichert werden die Regeln nur im Datenordner (`save-actions.json`, [Datenformat](data-and-backups.md#aktionen-beim-speichern-save-actionsjson)), nie im Projekt.

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
- Menüleiste: **Datei → Vergleichen mit …**;
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
| Menüleiste | an | die [Menüleiste](#menüleiste) über jeder Teilung |
| JSON beim Speichern formatieren / XML beim Speichern formatieren | aus | [beim Speichern formatieren](#json-und-xml-formatieren) (Dateien bis 1 MB) |
| Änderungsmarken am Rand | aus | [Änderungsmarken](#vergleich-und-änderungsmarken) |
| Grundlage der Marken | git (Index und HEAD) | oder die gespeicherte Datei |
| Darstellung im Vergleich | Automatisch | nebeneinander, unter ~700 px untereinander |
| Kontextzeilen | 3 | unveränderte Zeilen um jede Änderung im Vergleich |
| Leerraum ignorieren | aus | Vorgabe für neue Vergleiche |
| **Agenten** | alle aus | siehe [Agenten](#agenten) |

Gespeichert in `settings.json` als `editor` ([Datenformat](data-and-backups.md#neue-felder-in-300), [neu in 3.1.0](data-and-backups.md#neue-felder-in-310), [neu in 3.2.0](data-and-backups.md#neue-felder-in-320); für Vergleich und Änderungsmarken: `change_markers`, `marker_base`, `diff_context`, `diff_ignore_whitespace`, `diff_layout`).

![Einstellungen, Reiter Editor](../images/de/editor-settings.png)

## Agenten
Agenten (Claude Code und andere über die [Agenten-Schnittstelle](agent-interface.md#editor) sowie [Chats mit lokalen Modellen](chat.md)) können den Editor benutzen:
- **Lesen** (`editor_list`, `editor_read`) geht ab der Stufe *Nur lesen* – auch ungespeicherte Änderungen. Ebenso das **Prüfen** von JSON/XML (`format_file` mit `op: validate`, seit 3.2.0). **`editor_diff`** (seit 3.2.0, Stufe *Nur lesen* wie `editor_read`; nur externe Agenten) vergleicht ein Dokument mit seiner gespeicherten Datei, git HEAD/Index oder einem anderen offenen Dokument.
- Vier Schalter unter *Einstellungen → Editor → Agenten*, **alle standardmäßig aus** und nur bei der Stufe **Steuern** wirksam (für Chats: die Stufe des Modells):
  - **Agenten dürfen Dateien öffnen** – in einem neuen Tab; nur lokale Pfade, nie Netzwerk-/UNC-Pfade (`\\server\freigabe`, `\\wsl.localhost\…`, Netzlaufwerke) oder Geräte. Ein Agent darf auch einen unbenannten Tab mit Text öffnen.
  - **Agenten dürfen offene Dateien bearbeiten** – die Änderung bleibt **ungespeichert**, ist als Änderung des Agenten markiert (*Von einem Agenten geöffnet oder geändert*) und lässt sich rückgängig machen.
  - **Agenten dürfen speichern** – nur Dateien, die offen sind und schon existieren; unbenannte Tabs speicherst nur du.
  - **Agenten dürfen formatieren** *(seit 3.2.0)* – `format_file` formatiert, minifiziert oder sortiert die Schlüssel eines offenen JSON/XML-Dokuments mit dem eingebauten Formatierer; wie bei einer Bearbeitung bleibt die Änderung ungespeichert, ist als Änderung des Agenten markiert und lässt sich rückgängig machen.
- Agenten nehmen nie den Tastaturfokus: Eine Datei, die ein Agent öffnet oder zeigt, wird nicht fokussiert – was du gerade tippst, landet nie aus Versehen darin.
- Diese Regeln gelten für den Textmodus und den späteren Hex-Modus gleichermaßen.

## Noch nicht
Sprachserver (Modus *Smart*: *Gehe zu Symbol*, Formatierung, Diagnosen) und KI-Vervollständigung kommen mit 3.3.0, das Sprachpaket mit 3.4.0, Ausführen von Dateien und Projekten mit 3.5.0, ein Hex-Modus mit 3.6.0.

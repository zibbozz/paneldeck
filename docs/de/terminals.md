[← README](../../README.de.md) · [English](../en/terminals.md) · **Deutsch**

# Terminals und Sitzungen

Der **Terminal Hub** ist der Hauptbereich von Paneldeck: viele Terminal-Sitzungen in einem Fenster, geordnet in Projekten und Ordnern, mit frei teilbaren Bereichen.

<p align="center"><img src="../images/de/app.png" alt="Paneldeck mit geteilten Terminals, Freigabe-Anfrage und Seitenleiste" width="100%"></p>

## Shells und Emulation
- Terminals mit voller Emulation (ConPTY): **PowerShell 7, Windows PowerShell, CMD, Git Bash und WSL-Distributionen**.
- Farben, Alt-Screen, Maus, Unicode, farbige Emojis, CJK und lückenlose Rahmenzeichen; **fetter und kursiver** Text, Unterstreichungsarten (doppelt, Welle, gepunktet, gestrichelt) und Unterstreichungsfarben. Ein Programm kann einen **blinkenden Cursor** anfordern (*Einstellungen → Darstellung → Blinkender Cursor*, standardmäßig an; nur im fokussierten Terminal).
- **Terminalschrift** (*Einstellungen → Darstellung → Terminalschrift*): Cascadia Mono (eingebaut, Standard), Hack (eingebaut) oder **jede installierte Festbreitenschrift**, z. B. eine Nerd Font. Powerline-Trenner, Rahmen- und Blockzeichen zeichnet Paneldeck selbst, damit sie lückenlos anschließen. Fehlende Zeichen kommen aus den Windows-Systemschriften.
- **Mitgelieferter Konsolen-Host:** Installer und portables ZIP enthalten eine neuere ConPTY von Microsoft (`conpty.dll` und `OpenConsole.exe` neben `paneldeck.exe`). Sie reicht Farbabfragen und synchronisierte Ausgabe in der richtigen Reihenfolge durch, Vollbildprogramme flimmern dadurch weniger. *Einstellungen → Darstellung → Mitgelieferten Konsolen-Host verwenden (empfohlen)* ist standardmäßig an; ist sie aus oder fehlen die Dateien bzw. starten sie nicht, nutzen Sitzungen den Konsolen-Host von Windows (passiert das wiederholt, erscheint ein Hinweis). Die Einstellung gilt für neue Sitzungen.
- Jede Shell bekommt `TERM_PROGRAM=Paneldeck` und `TERM_PROGRAM_VERSION`. Paneldeck beantwortet Farbabfragen (OSC 4/10/11/12), Größenabfragen (CSI 14t/16t) und XTVERSION – auch für Sitzungen im Hintergrund – und meldet Programmen, die danach fragen, Fokuswechsel (DECSET 1004). `reset` setzt von einem Programm geänderte Farben auf das Theme zurück.
- **Profile** (*Einstellungen → Profile*) bündeln Shell, Startbefehl, Umgebungsvariablen und Darstellung – z. B. „Produktion“ rot oder „Claude in WSL“. Projekte und Ordner können ein Profil für neue Sitzungen vorgeben.
- **Umgebung:** Neue Terminals lesen die Umgebungsvariablen frisch aus Windows, eine mit `setx` gesetzte Variable wirkt also sofort. *Einstellungen → Allgemein → Umgebung von Paneldeck erben* (standardmäßig aus) gibt ihnen stattdessen genau die Umgebung, mit der Paneldeck gestartet wurde. Paneldecks eigene Variablen und die des Profils gelten in beiden Fällen; interne `PANELDECK_*`-Variablen eines Paneldeck, das dieses gestartet hat, werden nie weitergereicht.

## Tastatur
- **Kitty-Tastaturprotokoll:** Programme, die es einschalten (z. B. Claude Code), bekommen eindeutige Tasten – Umschalt+Enter, Strg+Rück, Esc und Alt-Kombinationen kommen als `CSI u`-Sequenzen an. Flags, die ein abgestürztes Programm stehen ließ, werden am nächsten Shell-Prompt zurückgesetzt.
- **win32-input-mode** unter ConPTY: PowerShell und cmd erhalten Strg+Leertaste, Strg+Enter, Strg+/, Strg+Umschalt+Buchstabe, Esc und den Ziffernblock als echte Tastenereignisse; Strg+3 … Strg+0 kommen im Terminal an.
- **Strg+Rück** sendet `0x08` wie Windows Terminal (PowerShell löscht das vorige Wort; in bash löscht es ein Zeichen). Alt+Satzzeichen (z. B. Alt+. in bash) funktioniert; Alt+Ziffer geht ans Terminal, wenn es keine Sitzung mit dieser Nummer gibt (sonst wechselt es dorthin). Linkes Strg+Alt wirkt wie AltGr (z. B. Strg+Alt+Q für `@` auf deutscher Tastatur), und IME-Eingaben (Japanisch, Chinesisch, Koreanisch) erscheinen am Cursor.
- *Einstellungen → Darstellung → Umschalt+Enter = neue Zeile in PowerShell* (standardmäßig aus): PSReadLine fügt eine neue Zeile ein, statt den Befehl auszuführen. Programme in PowerShell, die das kitty-Tastaturprotokoll nicht nutzen, bekommen dann ein normales Enter.
- **Broadcast** kodiert Tasten für jedes Zielterminal einzeln.

## Links, Zwischenablage und Einfügen
- **Links** im Terminal öffnen sich nur mit **Strg+Klick**. Programme können Links auch selbst setzen (**OSC 8**, der sichtbare Text kann vom Ziel abweichen); der Tooltip zeigt das echte Ziel und den Rechner. Für diese Links gilt:
  - `http`/`https`-Links mit schlichtem Rechnernamen öffnen direkt; jeder andere Link – andere Schemata, ein Benutzername in der Adresse (`user@host`), Rechnernamen mit Nicht-ASCII-Zeichen – fragt erst mit *Link öffnen?* und dem ganzen Ziel nach.
  - Nie geöffnet, auch nicht nach Rückfrage: überlange oder getarnte Ziele (Steuer-, Bidi- oder unsichtbare Zeichen, auch prozent-kodiert), Skript-Schemata, missbrauchte Windows-Protokolle (auch die Office-Handler), `file:`-Links auf anderes als einen lokalen Laufwerkspfad und alternative Datenströme.
  - Netzwerk- und Gerätepfade wie `\\.\GLOBALROOT\…` oder `\??\UNC\…` gelten nie als lokal – weder bei Links noch beim Arbeitsordner, den die Shell meldet (OSC 7, OSC 9;9).
  - Ausführbare Dateien und Skripte (jeder Typ aus `PATHEXT`, `.py`, `.pyw`, `.pl`, `.rb`, `.ahk`, `.php`, `.lua` und ähnliche, auch z. B. `.settingcontent-ms`, `.library-ms`, `.chm`, `.msc` und Namen mit Punkten oder Leerzeichen am Ende) werden nie gestartet; der Explorer zeigt sie nur markiert in ihrem Ordner.
- **Kopieren aus Programmen (OSC 52):** Programme können Text in die Windows-Zwischenablage legen – z. B. Claude Code in einer WSL- oder SSH-Sitzung; ein kurzer Hinweis nennt die Sitzung. *Einstellungen → Darstellung → Programme dürfen in die Zwischenablage kopieren (OSC 52)* ist standardmäßig an. Es geht nur in eine Richtung (Programme können die Zwischenablage nie lesen), nur die fokussierte Sitzung im Fenster im Vordergrund darf kopieren, höchstens 1 MB und höchstens alle 0,3 s (größere Texte melden einen Hinweis; eine Sequenz über 2 MiB wird abgeschnitten und ohne Hinweis ignoriert), und ein leerer Text leert die Zwischenablage nie. Hinweis: Ein Agent mit Zugriff *Steuern* kann die fokussierte Sitzung ebenfalls etwas kopieren lassen.
- **Einfügen:** Eingefügter Text wird wie bei Windows Terminal gefiltert (Steuerzeichen inkl. ESC und C1 werden entfernt), versteckte Enter-Sequenzen kommen also nicht durch. *Einstellungen → Darstellung → Vor dem Einfügen mehrerer Zeilen nachfragen* (standardmäßig an) fragt *Mehrere Zeilen einfügen?*, wenn das Programm kein Bracketed Paste nutzt – sonst könnte die Shell jede eingefügte Zeile sofort als Befehl ausführen. *Einfügen* wird nach einer kurzen Wartezeit frei. Jeder Bereich rahmt eingefügten Text nach seinem eigenen Bracketed-Paste-Modus; bei eingeschaltetem Broadcast kommt die Frage auch, wenn ein Zielbereich kein Bracketed Paste nutzt.

## Benachrichtigungen, Glocke und Fortschritt
Unter *Einstellungen → Benachrichtigungen*:
- **Benachrichtigungen von Programmen** (OSC 9, OSC 777; standardmäßig an): Ein Programm kann eine Benachrichtigung schicken; sie erreicht dich nur bei Sitzungen, die du gerade nicht ansiehst, auf den üblichen Wegen (Toast, blinkende Taskleiste) und höchstens einige pro Minute.
- **Glocke:** Die Glocke (BEL) einer Sitzung, die du gerade nicht ansiehst, markiert die Sitzung immer in der Seitenleiste; **Toast bei Glocke** (standardmäßig aus) zeigt zusätzlich einen Toast bzw. Hinweis.
- **Fortschritt anzeigen** (standardmäßig an): Fortschritt, den Programme melden (OSC 9;4 – normal, Fehler, angehalten oder unbestimmt), erscheint in der Seitenleiste und am Taskleistenknopf.
- Benachrichtigungstexte und Sitzungsnamen in Toasts werden bereinigt (Steuer-, Bidi- und unsichtbare Zeichen entfernt, Länge begrenzt); Toasts nennen immer Paneldeck und die Sitzung. Fenstertitel, die Programme setzen, werden genauso bereinigt.

## Mausrad
- **Normale Shell:** Das Mausrad scrollt den Verlauf von Paneldeck.
- **Programme mit Mausunterstützung** (z. B. `htop`, `vim` mit `mouse=a`, `tmux`, Midnight Commander): Paneldeck meldet Mausrad und Klicks in jeder Maus-Kodierung an das Programm (SGR, UTF-8 und die normale Kodierung). Die normale Kodierung kann Positionen nur bis Spalte/Zeile 94 tragen – größere Werte würde ConPTY verfälschen –, darüber sendet Paneldeck deshalb nichts statt einer falschen Position; die meisten Programme nutzen ohnehin SGR.
- **Vollbildprogramme ohne Mausunterstützung** (z. B. `less`, `man`): Das Mausrad sendet Pfeiltasten, damit der Text scrollt – aber nur, wenn das Programm es erlaubt (Alternate Scroll Mode, DECSET 1007, standardmäßig an). Die Einstellung *Einstellungen → Darstellung → Mausrad in Vollbildprogrammen* wählt **Pfeiltasten senden** (Standard) oder **Aus**.
- **Umschalt+Mausrad** scrollt immer den Verlauf von Paneldeck, auch wenn das Programm die Maus nutzt. Vollbildprogramme haben keinen Verlauf, dort tut Umschalt+Mausrad deshalb nichts.
- **Nach einem abgestürzten Vollbildprogramm**, das seine Modi nicht zurückgesetzt hat, verhält sich das Mausrad am Shell-Prompt wieder wie in einer normalen Shell (keine Pfeiltasten in die Befehlszeile, kein Zeichensalat aus Mauscodes), bis du den nächsten Befehl ausführst. Das braucht Shell-Integration (die Shell meldet ihren Prompt); ohne sie verhält sich das Mausrad weiter wie im abgestürzten Programm.

## Sitzungen starten
- **Strg+Umschalt+T** oder *Neue Sitzung …* in der Seitenleiste öffnet eine Sitzung; *Neue Claude-Sitzung …* (Funken-Knopf) startet eine mit Claude Code (siehe [Claude Code](claude-code.md)). Der Sprechblasen-Knopf beginnt statt eines Terminals einen **Chat mit einem lokalen Modell** (siehe [Chats mit lokalen Modellen](chat.md)).
- Bei einem frischen Start öffnet Paneldeck **kein Terminal**: Der leere Terminal Hub zeigt einen kurzen Hinweis mit *Neues Terminal* (im Claude-Modus auch *Neue Claude-Sitzung …*) und den Tastenkürzeln. Sitzungen des letzten Laufs werden wie bisher wiederhergestellt. Siehe [Mehrere Fenster](windows.md#der-leere-terminal-hub).
- Von außen: das Explorer-Kontextmenü (*In Paneldeck öffnen* / *Mit Claude in Paneldeck starten*), `paneldeck.exe --open <ordner>` oder ein auf `paneldeck.exe` gezogener Ordner (siehe [Kommandozeile](command-line.md) und [Windows-Integration](windows-integration.md)).

### WSL-Startordner
Eine WSL-Sitzung kann in einem Ordner **innerhalb von WSL** starten:
- Das Ordnerfeld von *Neue Sitzung* hat einen Umschalter **Windows-Pfad / WSL-Pfad**. Mit *WSL-Pfad* wählst du eine der installierten Distributionen (die Standard-Distribution zuerst; Vorlagen und Docker Desktop sind ausgeblendet) und gibst einen Linux-Pfad ein – absolut oder `~` – mit Vervollständigung und Prüfung, ob der Ordner existiert. *Ordner wählen* öffnet im Home-Ordner der Distribution, die Auswahl wird zum Linux-Pfad. *Zuletzt benutzte WSL-Ordner* werden je Distribution gemerkt.
- Standardordner von Baumordnern und Projekten mit WSL-Shell können ebenfalls Linux-Pfade sein.
- *In Paneldeck öffnen* im Explorer auf einem `\\wsl.localhost\<Distro>\…`-Ordner öffnet eine WSL-Sitzung dieser Distribution im passenden Linux-Pfad.
- Eine WSL-Sitzung ohne Startordner startet im Linux-Home (`~`). Eine wiederhergestellte WSL-Sitzung, deren gespeicherter Ordner nicht verwendbar ist, startet mit Hinweis in `~`. Ordner mit `"` oder `\` werden abgelehnt.

## Organisation
- **Projekte** mit eigenem Layout, dazu **Ordner**, Gruppen, Farben, Tags, Filter (`Filtern: Name oder #Tag`) und Notizen. Die Seitenleiste zeigt die Sitzungen als Liste oder Ordnerbaum; Drag & Drop verschiebt Sitzungen und Ordner.
- Ordner können einen Arbeitsordner und **Vorgaben für neue Sitzungen** (Profil) haben; *Als Vorlage speichern …* legt eine Gruppe von Sitzungen als **Sitzungs-Vorlage** ab.
- **Vorlagen-Bibliothek** (Abschnitt *Vorlagen* unten in der Seitenleiste, einklappbar, ab fünf Vorlagen mit Suche): Eine **Terminal-Vorlage** ist eine vollständige Sitzung – Name, Profil/Shell, Startordner (auch als WSL-Pfad), Ordner im Baum, Farbe, Tags, Notizen, Claude an/aus mit [Startoptionen](claude-code.md#startoptionen), Konversation und Start-Prompt. Anlegen aus einer Sitzung (Kontextmenü *Als Terminal-Vorlage speichern …*) oder neu (**+**); bearbeiten, duplizieren oder löschen über ihr Kontextmenü. Gestartet wird per Doppelklick, durch Ziehen auf einen Ordner im Baum (startet dort), in die Sitzungsliste (eigener Ordner) oder auf einen Bereich (ersetzen oder links/rechts/oben/unten teilen; Esc bricht das Ziehen ab), über *Vorlage hier starten* im Kontextmenü eines Ordners oder über die Befehlspalette. Wiederholte Starts zählen hoch: `API`, `API #2`, `API #3`. Gruppen-Vorlagen liegen in derselben Bibliothek und merken sich ebenfalls Farbe, Tags, Notizen und Claude-Startoptionen. `bypassPermissions` wird nie in einer Vorlage gespeichert; zusätzliche Ordner, erlaubte Werkzeuge, eine MCP-Konfiguration und `--safe-mode` gelten nur, wenn du die Vorlage selbst startest, nie für Agenten – und erst nach einer Rückfrage, die sie auflistet („Starten“ / „Ohne diese Optionen starten“ / „Abbrechen“); solche Vorlagen sind mit ⚠ markiert.
- **Bereiche:** rechts teilen (**Strg+Umschalt+D**) oder darunter (**Strg+Umschalt+E**), per Drag & Drop umordnen, einen Bereich minimieren (**Strg+Umschalt+W**) oder eine Sitzung in ein eigenes Fenster abdocken (siehe [Bereiche und Minimieren](#bereiche-und-minimieren)).
- **Mehrere Fenster:** **Strg+Klick** auf den Reiter *Terminal Hub* öffnet ein weiteres Fenster mit eigenen Sitzungen (siehe [Mehrere Fenster](windows.md)).
- **Farben** von Sitzungen, Ordnern, Projekten und Profilen kommen aus der bearbeitbaren Farbpalette (*Einstellungen → Farben*, siehe [Sprache und Darstellung](#sprache-und-darstellung)).
- Beim Neustart stellt Paneldeck Sitzungen, Projekte und Layout wieder her; der Verlauf jedes Terminals wird im Datenordner gespeichert (`scrollback/`).
- **Sitzungen merken** (*Einstellungen → Allgemein*, standardmäßig an): Ist es aus, ist jeder Start frisch – keine Terminals, Chats, Projekte oder Ordner aus dem letzten Lauf, Chatverläufe werden beim Beenden gelöscht. Einstellungen, Todo-Liste und Tickets bleiben. Das Ausschalten fragt nach und sichert den aktuellen Stand als `state.json.bak-<Datum>` (nur die neueste bleibt). Ein Neustart nach einem Update stellt die Sitzungen trotzdem einmal wieder her. Solange es aus ist, enthält `state.json` nur den gekürzten Stand; nach einem Absturz bleibt die Sicherung `state.json.bak-<Datum>` also erhalten.

## Bereiche und Minimieren
- **Knöpfe im Kopf eines Bereichs** (von links nach rechts): *Ticket verknüpfen …* (nur wenn Ticket-Verknüpfungen verfügbar sind), *Rechts teilen*, *Darunter teilen*, *In eigenem Fenster öffnen*, **Neu laden** (siehe [Neu laden und Einstellungen der Sitzung](#neu-laden-und-einstellungen-der-sitzung)), **Bereich minimieren** und *Sitzung schließen* (×). Ein abgedocktes Fenster hat *Wieder andocken* (×, **Strg+Umschalt+W**) und *Minimieren*.
- **Minimieren** nimmt den Bereich aus dem geteilten Layout; sein Nachbar übernimmt den Platz, und das Terminal oder der Chat **läuft weiter**. Das geht auch mit dem letzten Bereich – der Terminal Hub zeigt dann „Alle Bereiche sind minimiert“. Minimieren gibt es im Kopf des Bereichs, in den Kontextmenüs des Bereichs und der Sitzung in der Seitenleiste, in der Befehls-Palette (*Bereich minimieren*) und auf **Strg+Umschalt+W**. In einem abgedockten Fenster schließt *Minimieren* das Fenster, ohne die Sitzung wieder anzudocken.
- **Seitenleiste:** Ein **abgeblendeter** Name heißt, die Sitzung steht gerade nicht im Layout; eine kleine **Unterstrich-Markierung** heißt, sie ist minimiert. Der Tooltip sagt, was ein Klick tut.
- **Eine Sitzung zurückholen** – Klick in der Seitenleiste oder *Wechseln zu:* in der Palette:
  - schon im Layout → sie bekommt nur den Fokus;
  - leerer Terminal Hub → sie wird der einzige Bereich;
  - **minimiert → sie öffnet sich allein:** Sie ersetzt das Layout des Fensters bzw. Projekts, die bisher sichtbaren Bereiche werden zu minimierten Sitzungen und laufen weiter. Ein kurzer Hinweis sagt, wie viele Bereiche minimiert wurden. Der alte Platz in der Teilung wird nicht gemerkt;
  - nur weggewechselt (nicht minimiert) → sie ersetzt den fokussierten Bereich.
- **Strg+Tab** / **Strg+Umschalt+Tab** erreichen auch minimierte Sitzungen: Eine minimierte Sitzung erscheint im fokussierten Bereich, der ersetzte Bereich wird minimiert.
- Agenten bestimmen mit `placement`, wo eine Sitzung erscheint (siehe [Agenten-Schnittstelle](agent-interface.md#platzierung-und-sichtbarkeit)).
- Minimierte Sitzungen bleiben auch nach einem Neustart minimiert. Bereiche behalten eine **Mindestgröße** von etwa 120 × 70 px, auch in tiefen Teilungen und beim Ziehen einer Trennlinie.

## Neu laden und Einstellungen der Sitzung
- **Neu laden** (Knopf im Kopf des Bereichs – auch in abgedockten Fenstern –, *Neu laden …* im Kontextmenü der Seitenleiste, *Sitzung neu laden …* in der Palette, optionales Tastenkürzel *Sitzung neu laden* ohne Vorbelegung) startet die Shell der Sitzung neu bzw. Claude Code nach den Optionen der Sitzung. Läuft ein Programm, fragt es vorher (*Sitzung neu laden?*); hat die Sitzung Hintergrundprogramme, siehe [Sitzungen schließen](#sitzungen-schließen-und-hintergrundprogramme). *Neu starten* einer beendeten Sitzung und *Beendete Sitzungen neu starten* fragen genauso vorher.
- **Einstellungen der Sitzung** (*Einstellungen …* im Kontextmenü der Seitenleiste, *Einstellungen der Sitzung …* in der Palette) ändern eine bestehende Sitzung: Profil, Shell, Startordner, Name, Farbe, Baum-Ordner, Tags, Notizen, Claude an/aus und die Claude-Startoptionen (siehe [Claude Code](claude-code.md#startoptionen)). Änderungen, die erst beim nächsten Start wirken, bieten *Jetzt neu laden?* an. Bei einer WSL-Sitzung hat der Startordner denselben Umschalter **Windows-Pfad / WSL-Pfad** wie *Neue Sitzung*, Linux-Pfade gehen also auch hier.

## Sitzungen schließen und Hintergrundprogramme
- **Schließen** einer Sitzung mit dem × im Kopf des Bereichs, über die Kontextmenüs oder per **Mittelklick** auf die Sitzung in der Seitenleiste (mit derselben Rückfrage wie der Schließen-Knopf, wenn *Einstellungen → Allgemein → Schließen bestätigen* an ist).
- **Alle Sitzungen auf einmal schließen:** ein Papierkorb an Ordnern in der Seitenleiste (erscheint beim Überfahren; inklusive Unterordnern, minimierten Sitzungen und Chats), einer rechts neben dem Filterfeld (alle Sitzungen aller Projekte und Fenster), *Alle Sitzungen des Projekts schließen …* im Projekt-Kontextmenü, die Befehls-Palette und ein optionales Tastenkürzel *Alle Sitzungen schließen* (ohne Vorbelegung). Die Rückfrage listet auf, was beendet wird, und gibt *Alle schließen* nach 3 Sekunden frei. Git-Worktrees und Branches bleiben erhalten.
- **Hintergrundprogramme (Job Objects):** Die Shell jedes Terminals läuft in einem eigenen Windows-Job-Object; Programme, die aus einer Sitzung gestartet werden, gehören dazu, auch losgelöst gestartet (`Start-Process`, `cmd /c start`, doppelter Fork). Das gilt auch für langlebige Programme wie `code .`, einen Browser oder `wt`. Laufen solche Programme noch, wenn du eine Sitzung schließt oder neu lädst, ein Fenster schließt oder Paneldeck beendest, listet die Rückfrage sie auf – Name, PID und Fenstertitel, Programme mit offenem Fenster markiert – und bietet **Weiterlaufen lassen** oder **Alle beenden** (beim Neu laden: *Neu laden, weiterlaufen lassen* / *Neu laden, alle beenden*; bei einem Fenster: *Schließen, weiterlaufen lassen* / *Schließen, alle beenden*). **„Alle beenden“ beendet sie sofort, ohne zu speichern** – Programme mit offenem Fenster besser vorher selbst speichern und schließen. Der Knopf wird nach 3 Sekunden frei, auch beim Beenden. Programme in einer Sitzung können ihren Job nicht verlassen (`CREATE_BREAKAWAY_FROM_JOB` schlägt fehl); das ist gewollt.
  - Aufgelistet werden Programme mit Fenster und alles, was die Shell selbst gestartet hat. Hilfsprozesse mit verborgener Konsole, die zu einem noch im Vordergrund laufenden Programm gehören (z. B. die MCP-Server und Werkzeugaufrufe von Claude Code), zählen nicht – fensterlose Hintergrundaufgaben eines laufenden Programms erscheinen also nicht, solange es läuft. Etwa 6 Sekunden nach Schließen, Neu laden oder Neu starten prüft Paneldeck die alte Sitzung noch einmal: Laufen solche Prozesse noch, nennt ein Hinweis sie und bietet *Alle beenden* an.
  - Beim Schließen einer Sitzung mit **Git-Worktree** werden ihre Hintergrundprogramme ebenfalls aufgelistet; die Worktree-Frage hat ein Kästchen *Hintergrundprozesse beenden*.
- **Agenten-Kette:** Sitzungen, die Agenten angelegt haben, sind markiert; ihr Kontextmenü hat *Agenten-Kette zurücksetzen*, der Tooltip nennt die Kette (siehe [Agenten-Schnittstelle](agent-interface.md#sicherheit)).

## Komfort
- **Befehls-Palette** (**Strg+Umschalt+P**): alle Befehle, Sitzungen und – im Expertenmodus – Tickets.
- **Suche** im Verlauf (**Strg+Umschalt+F**) und über alle Sitzungen (**Strg+Umschalt+G**); zwischen Befehlen springen (**Strg+Umschalt+Pfeil hoch/runter**, braucht Shell-Integration).
- **Prompt-Bibliothek**, **Prompt-Composer** (**Strg+Umschalt+M**) und eine **Warteschlange** je Sitzung (**Strg+Umschalt+Q**).
- **Broadcast** (**Strg+Umschalt+B**): in alle sichtbaren Bereiche gleichzeitig tippen.
- **Dateibaum** (**Strg+Umschalt+O**), **Änderungen** mit Diff, Stage und Commit (**Strg+Umschalt+H**), Git-Info im Kopf. Das Lesen der Änderungen führt nie Befehle aus der Git-Konfiguration eines Repositorys aus (externes Diff, textconv, Filter – außer dem Standard-Filter von Git LFS) und schaut nicht in die Arbeitsordner von Untermodulen; ein geänderter Untermodul-Commit erscheint weiterhin.
- Drag & Drop von Dateien, Bilder aus der Zwischenablage (14 Tage im Datenordner), klickbare Links (Strg+Klick, siehe [Links, Zwischenablage und Einfügen](#links-zwischenablage-und-einfügen)).
- Tray-Symbol, Transparenz, Themes (JSON, siehe [Datenordner](data-and-backups.md)), weiche Animationen.

## Tastenkürzel
Alle Kürzel lassen sich unter *Einstellungen → Tastenkürzel* ändern.

| Kürzel | Aktion |
|---|---|
| `Strg+Umschalt+T` | Neue Sitzung |
| `Strg+Umschalt+D` / `Strg+Umschalt+E` | Bereich rechts / darunter teilen |
| `Strg+Umschalt+W` | Bereich minimieren (im abgedockten Fenster: wieder andocken) |
| `Strg+Umschalt+R` | Sitzung umbenennen |
| `Strg+Umschalt+P` | Befehle (Palette) |
| `Strg+Umschalt+F` | Im Verlauf suchen |
| `Strg+Umschalt+G` | Alle Sitzungen durchsuchen |
| `Strg+Umschalt+B` | Broadcast an/aus |
| `Strg+Umschalt+N` | Notizen |
| `Strg+Umschalt+O` | Dateibaum |
| `Strg+Umschalt+K` | CLAUDE.md / Claude-Dateien |
| `Strg+Umschalt+H` | Änderungen (Diff) |
| `Strg+Umschalt+Q` | Warteschlange der Sitzung |
| `Strg+Umschalt+M` | Prompt-Composer |
| `Strg+Umschalt+Pfeil hoch` / `Strg+Umschalt+Pfeil runter` | Zum vorherigen / nächsten Befehl springen |
| `Strg+Tab` / `Strg+Umschalt+Tab` | Nächste / vorherige Sitzung (auch minimierte) |
| `Strg+1` / `Strg+2` (auch `Alt+Umschalt+1` / `Alt+Umschalt+2`) | Reiter *Terminal Hub* / *Todo-Liste* |
| `Strg`+Klick auf einen Reiter der Fensterleiste | Reiter in einem neuen Fenster öffnen |
| (keins) | *Sitzung neu laden*, *Alle Sitzungen schließen* – Taste unter *Einstellungen → Tastenkürzel* zuweisen |
| `Strg+Umschalt+1`…`9` (fest) | Zu Projekt 1–9 wechseln |
| `Alt+1`…`9` (fest) | Sitzung 1–9 des aktuellen Projekts |
| `Strg+Alt+Pfeile` (fest) | Nachbarbereich fokussieren |

Die Modifikatoren müssen genau passen (`Strg+Umschalt+1` ist nicht `Strg+1`). Die festen Kürzel greifen nur, wenn es ihr Ziel gibt; sonst geht die Taste ans Terminal. Eigene Kürzel brauchen Strg oder Alt (F-Tasten gehen auch allein); bei der Aufnahme bricht Esc ab. *Einstellungen*, der Update-Knopf und der MCP-Status stehen in der Fensterleiste rechts neben den Reitern.

Die Kürzel der Todo-Liste stehen unter [Todo-Liste und Tickets](todo-and-tickets.md#tastenkürzel).

## Sprache und Darstellung
- Englisch oder Deutsch: *Einstellungen → Allgemein → Sprache* (Standard: Englisch). `--lang en|de` wechselt die Sprache nur für einen Start (siehe [Kommandozeile](command-line.md#--lang-ende)).
- Themes, Terminalschrift, Schriftgröße und Transparenz: *Einstellungen → Darstellung*; eigene Themes sind JSON-Dateien in `themes/` im Datenordner (ein Beispiel liegt bei).
- **Farbpalette** (*Einstellungen → Farben*): standardmäßig eine gemeinsame Palette für den Terminal Hub (Farben von Sitzungen, Ordnern, Projekten und Profilen), die Tags der Todo-Liste und den Expertenmodus (Projekte, Typen, Status, Auswahlwerte, Diagramme) – oder mit *Eigene Palette je Bereich* eine je Bereich (beim Umschalten bleiben beide erhalten). Farben hinzufügen (Farbwähler oder Hex), optional benennen (im Tooltip, z. B. „Cooles Blau (#2288FF)“), per Ziehen sortieren und löschen; nach dem Löschen gibt es einige Sekunden lang *Rückgängig*. *Vorgaben wiederherstellen* hängt gelöschte Vorgabefarben wieder an. Die eigene Palette eines Bereichs beginnt mit dessen bisherigen Farben; *Aus der gemeinsamen Palette kopieren* ersetzt sie durch eine Kopie der gemeinsamen (rückgängig machbar). Jede Farbauswahl bietet zusätzlich *Eigene Farbe …* für eine einmalige Farbe, die nicht in die Palette aufgenommen wird. Vorhandene Elemente behalten ihre Farbe.

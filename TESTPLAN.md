# Testanleitung (Windows)

> **Kurzfassung (ca. 30 Minuten)** – wenn wenig Zeit ist, diese Punkte zuerst:
> 1. Start, Icon, Schrift (Abschnitt 1) · 2. Alle Shells starten, AltGr-Zeichen (2, 3) · 3. Einfügen genau einmal (9) ·
> 4. Claude starten, Status/Toast/Kosten (6, 8) · 5. Neustart: Layout + `--resume` (5, 6) · 6. Projekte, Drag & Drop (10) ·
> 7. Emojis/CJK/Rahmen (10) · 8. Tray + Abdocken (11) · 9. WSL (14) · 10. Diagnose kopieren (14).
> Bei jedem ❌: kurz beschreiben und – falls vorhanden – `crash.log` + „Diagnose kopieren“ mitschicken.

Wird mit jeder Version erweitert. Voraussetzung: Windows 10 (1809+) oder 11, x64.
Für jeden Punkt: ✅ funktioniert / ❌ Fehler (kurz beschreiben).

## 0 · Vorbereitung
1. ZIP entpacken, `paneldeck.exe` in einen leeren Ordner legen (z. B. `C:\Tools\Paneldeck`).
2. Doppelklick. SmartScreen-Warnung → „Weitere Informationen“ → „Trotzdem ausführen“.
3. Es entsteht ein Ordner `paneldeck-data` neben der `.exe`.

## 1 · Start & Aussehen
- [ ] Fenster öffnet ohne Konsolenfenster, Titel „… – Paneldeck“.
- [ ] Eigenes Icon in Taskleiste und Fenster; im Explorer: Rechtsklick auf die `.exe` → Eigenschaften → Details zeigt „Paneldeck“ und die Version.
- [ ] Terminal-Schrift ist Cascadia Mono, Text scharf, Zoom/Größe in Einstellungen wirkt.

## 2 · Shells (Neu-Dialog „+ Neu“)
- [ ] Auswahlliste zeigt: Windows PowerShell, CMD; zusätzlich (falls installiert) PowerShell 7, Git Bash, WSL-Distributionen.
- [ ] Jede Shell starten: Prompt erscheint, `dir`/`ls` und `echo äöü €` zeigen korrekte Zeichen.
- [ ] Ordnerauswahl über „…“ funktioniert; neue Sitzung startet im gewählten Ordner.

## 3 · Terminal-Verhalten
- [ ] Tippen, Backspace, Pfeiltasten (Verlauf), Tab-Vervollständigung, Strg+C bricht `ping -t localhost` ab.
- [ ] AltGr-Zeichen auf deutscher Tastatur: `@`, `{`, `[`, `\`, `|`, `~`, `€`.
- [ ] Auswahl mit Maus, Doppelklick = Wort, Dreifachklick = Zeile; Strg+C kopiert bei Auswahl; Rechtsklick kopiert/fügt ein.
- [ ] Mehrzeiliges Einfügen (aus Editor) funktioniert, ohne dass Zeilen zu früh ausgeführt werden (in PowerShell 7 / Claude).
- [ ] Mausrad scrollt durch den Verlauf; `Get-ChildItem C:\Windows -Recurse` o. ä. erzeugt viel Ausgabe ohne Hänger.
- [ ] Fenstergröße ändern: Terminal passt Spalten/Zeilen an (`mode con` oder `$Host.UI.RawUI.WindowSize`).
- [ ] `vim`/`less`/`claude` (Vollbild-Programme) zeichnen korrekt, Beenden stellt den Verlauf wieder her.

## 4 · Sitzungen, Gruppen, Splits
- [ ] Sitzung umbenennen: Doppelklick, F2, Strg+Umschalt+R; leerer Name → automatischer Titel.
- [ ] Rechtsklick-Menü: Farbe, Gruppe (neue + bestehende), Duplizieren, Neu starten, Schließen.
- [ ] Schließen einer laufenden Sitzung fragt nach; `exit` im Terminal → Leiste „Prozess beendet“ → „Neu starten“.
- [ ] Strg+Umschalt+D / E teilt; Trennlinie ziehen; mehrfach verschachteln; „✕“ blendet Bereich aus (Sitzung bleibt in der Liste).
- [ ] Strg+Tab, Alt+1…9, Strg+Alt+Pfeile wechseln Sitzung bzw. Bereich.

## 5 · Persistenz
- [ ] Layout, Namen, Gruppen, Farben, Notizen bleiben nach Schließen und Neustart erhalten.
- [ ] Verlauf (Scrollback) ist nach Neustart über dem frischen Prompt sichtbar („wiederhergestellter Verlauf“).
- [ ] Kopiere den Ordner samt `.exe` an einen anderen Ort → Daten wandern mit (portabel).

## 6 · Claude-Modus
Voraussetzung: `claude` ist installiert und angemeldet.
- [ ] „Claude“-Button → Dialog (Modell, Permission-Mode, Start-Prompt) → Claude startet im Ordner.
- [ ] Statuspunkt: *arbeitet* (pulsiert) → *Freigabe?* bei Rückfrage → *bereit* nach Antwort.
- [ ] Anderes Fenster in den Vordergrund: Toast + Taskleisten-Blinken bei Freigabe/Fertig.
- [ ] Neustart der App: Claude-Sitzung wird mit `--resume` fortgesetzt (richtige Konversation, auch bei zwei Sitzungen im selben Ordner).
- [ ] Einstellungen → Claude-Modus aus: Claude-Elemente verschwinden, Terminals funktionieren weiter.
- [ ] Einstellungen → Hooks global installieren/entfernen: `~\.claude\settings.json` enthält/entfernt die Hub-Einträge, fremde Hooks bleiben.

## 7 · Palette, Suche, Prompts, Broadcast, Notizen
- [ ] Strg+Umschalt+P: Tippen filtert, Pfeile/Enter/Esc.
- [ ] Strg+Umschalt+F: Treffer markiert, Enter/Umschalt+Enter springt.
- [ ] Prompt-Bibliothek: anlegen, senden, mit/ohne automatisches Enter.
- [ ] Broadcast: zwei Bereiche, Eingabe landet in beiden.
- [ ] Notizen: Text eingeben, Symbol in der Seitenleiste.

## 8 · Claude-Daten (0.4.0)
Voraussetzung: mindestens eine Claude-Konversation existiert unter `%USERPROFILE%\.claude\projects`.
- [ ] In einer Claude-Sitzung einen Prompt senden: nach wenigen Sekunden erscheint in der Kopfzeile `ctx … · … in · … out · ≈ $…`; Tooltip in der Seitenleiste zeigt Modell und Details; oben steht die Summe (Σ).
- [ ] Nach App-Neustart sind die Werte sofort wieder da.
- [ ] Button *Verlauf*: Liste erscheint (mit Ladeanzeige), Filter funktioniert, *Fortsetzen* startet `claude --resume …` im richtigen Ordner mit der richtigen Konversation.
- [ ] Git: Sitzung in einem Git-Ordner → `git: <branch> +N` in der Kopfzeile; nach einer Dateiänderung aktualisiert sich N innerhalb weniger Sekunden. Ohne installiertes Git erscheint nur der Branch bzw. nichts (kein Fehler).
- [ ] In einer Claude-Sitzung in einen anderen Git-Ordner wechseln: Git-Info folgt nach dem nächsten Hook-Ereignis.

## 9 · Eingabe (0.5.0)
- [ ] **Einfügen**: Text kopieren (z. B. aus Notepad), Strg+V in PowerShell → Text erscheint **einmal** (nicht doppelt).
- [ ] **Drag & Drop**: Datei aus dem Explorer auf einen Bereich ziehen → Hervorhebung, Pfad erscheint; Pfad mit Leerzeichen wird in Anführungszeichen gesetzt. Mehrere Dateien auf einmal. Bei einer WSL-Sitzung erscheint `/mnt/c/...`, bei Git Bash `/c/...`.
- [ ] **Bild einfügen**: Screenshot mit Win+Umschalt+S, in einer Claude-Sitzung Strg+V → Pfad zu `paneldeck-data\clipboard\clip-….png` erscheint; Claude kann das Bild lesen (z. B. „Beschreibe das Bild“ + Pfad). Mit Text in der Zwischenablage bleibt es beim normalen Einfügen.
- [ ] **Maus**: in Git Bash/WSL `vim` (mit `:set mouse=a`) oder `htop`/`mc` starten: Klicken, Ziehen und Mausrad wirken im Programm; mit Umschalttaste lässt sich Text normal markieren.
- [ ] **Dateibaum**: Button *Dateien* → Baum des Sitzungsordners; Ordner auf-/zuklappen; Doppelklick auf Datei fügt Pfad ein; Rechtsklick → „Neue Sitzung hier“ und „Im Explorer anzeigen“ funktionieren; „versteckte“ blendet Punkt-Dateien ein.

## 10 · Organisation & Text (0.6.0)
- [ ] **Projekte**: `+` neben den Projekt-Chips → Name eingeben; Sitzungen und Layout sind pro Projekt getrennt; Wechsel per Klick und Strg+Umschalt+1…9; Rechtsklick → Umbenennen, Farbe, Standardordner (neue Sitzung startet dort), Löschen.
- [ ] Nach Neustart ist das zuletzt aktive Projekt samt Layout wieder da.
- [ ] **Tags/Filter**: Rechtsklick auf Sitzung → *Tags …* (`a, b`); Filterfeld: Name, Teilwort, `#a` – auch über Projekte hinweg; Tooltip zeigt Tags.
- [ ] **Drag & Drop**: Sitzung aus der Seitenleiste auf Mitte eines Bereichs (ersetzt/tauscht), auf Randzonen (teilt links/rechts/oben/unten); Kopfzeile eines Bereichs auf einen anderen ziehen; Esc bricht ab.
- [ ] **Emojis** (Windows Terminal zum Vergleich): `echo "😀 🚀 ✅ ❌ ❤️ 🔥 👍 🦀"` → farbige Emojis, passend groß, keine Kästchen. Sequenzen (👨‍👩‍👧 🇩🇪) erscheinen als einzelne Emojis.
- [ ] **CJK/Symbole**: `echo "日本語 中文 한국어 Ωmega ∑ ≠ ✻ ⏺"` → alle Zeichen sichtbar (Windows-Systemschriften), breite Zeichen belegen zwei Zellen ohne Überlappung.
- [ ] **Rahmen**: `claude` (oder `tree /f`) → Rahmen und Balken ohne Lücken; Block-/Schattierungszeichen `█▓▒░ ▀▄` sauber.
- [ ] Emoji im Sitzungsnamen (Umbenennen → `API 🚀`) wird in Seitenleiste und Kopfzeile angezeigt (einfarbig).

## 11 · Fenster & System (0.7.0)
- [ ] **Abdocken**: Symbol „Pfeil aus Fenster“ in der Kopfzeile → Sitzung öffnet sich in eigenem Fenster, Hauptfenster zeigt sie nicht mehr; Tippen im abgedockten Fenster funktioniert, Statusanzeige/Kosten/Git erscheinen in dessen Kopfzeile.
- [ ] Abgedocktes Fenster auf zweiten Monitor ziehen; Schließen (X) bzw. Strg+Umschalt+W dockt wieder an (neuer Bereich neben dem fokussierten).
- [ ] Sidebar: Klick auf eine abgedockte Sitzung holt deren Fenster in den Vordergrund.
- [ ] Neustart der App mit abgedockter Sitzung: Fenster wird wieder geöffnet.
- [ ] **Tray**: Symbol erscheint im Infobereich; Tooltip „Paneldeck“ (mit „N warten auf dich“, wenn Freigaben ausstehen); Linksklick bringt das Fenster zurück; Rechtsklick-Menü *Paneldeck anzeigen* / *Beenden*.
- [ ] Einstellung „Schließen minimiert“ an: X-Button minimiert, Claude-Benachrichtigung (Toast) kommt trotzdem; *Beenden* im Tray-Menü beendet die App und speichert den Zustand.
- [ ] **Transparenz**: Deckkraft auf ~60 % → Fenster wird durchscheinend (Terminalhintergrund und Seitenleiste), Text bleibt gut lesbar; 100 % = vollständig deckend, keine Artefakte/schwarze Ränder.

## 12 · UI-Überarbeitung & Animationen (0.8.0)
Auf einem Monitor mit 100 % und – falls möglich – 150 % Skalierung prüfen.
- [ ] **Gesamteindruck**: Seitenleiste etwas dunkler als das Terminal, ruhige Flächen, keine harten Rahmen; Schrift der Oberfläche ist Segoe UI; Terminal weiter Cascadia Mono.
- [ ] **Werkzeugleiste**: alle Symbole haben Tooltips mit Kürzel; aktive Werkzeuge (Dateien, Broadcast, Prompts, Verlauf) sind orange hinterlegt; bei schmaler Seitenleiste bricht die Leiste um.
- [ ] **Sitzungszeilen**: zweizeilig (Name / Status · Ordner oder Branch · Kosten); Statuspunkt pulsiert bei „arbeitet“, roter Ring bei „Freigabe?“; ausgewählte Zeile orange getönt; Hover weich.
- [ ] **Kopfzeilen**: Statuspille, `ctx … · … in · … out` und `git: …` erscheinen nur, wenn Platz ist; Symbole (teilen, abdocken, ausblenden) sind dezent und werden bei Hover hell; fokussierter Bereich hat Akzentlinie oben.
- [ ] **Trenner**: ziehen, Farbe wird beim Anfassen orange.
- [ ] **Leerzustand**: Projekt ohne Sitzungen → Willkommen/Hinweis mit Schaltflächen; *Neue Sitzung* funktioniert.
- [ ] **Einstellungen**: Tabs, Schalter (Animationen, Benachrichtigungen, Schließen minimiert) schalten weich um; Schieberegler sichtbar; Theme-Wechsel (auch „Hell / Light“) färbt Seitenleiste, Kopfzeilen und Dialoge passend ein, Text bleibt gut lesbar.
- [ ] **Animationen**: Hover über Symbole/Zeilen, Dateileiste ein/aus (gleitet), Broadcast-Leiste ein/aus; Einstellung *Animationen* aus → alles schaltet sofort um, keine Ruckler.
- [ ] **Deckkraft** < 100 %: Seitenleiste und Terminal scheinen durch, Kopfzeilen/Dialoge bleiben lesbar.
- [ ] Performance: Mit 6–8 Sitzungen und laufender Ausgabe bleibt die Oberfläche flüssig.

## 13 · Anpassung (0.9.0)
- [ ] **Tastenkürzel**: Einstellungen → Tastenkürzel → „Neue Sitzung“ anklicken, Strg+Alt+T drücken → neues Kürzel wirkt, altes nicht mehr; Tooltip des „+“-Symbols zeigt das neue Kürzel; „Zurücksetzen“ stellt Standard her; doppelt belegte Kürzel werden markiert; Esc beim Aufnehmen bricht ab; Eingaben im Terminal (z. B. Strg+C) bleiben unbeeinflusst.
- [ ] Neustart: geänderte Kürzel bleiben erhalten (`settings.json`, Abschnitt `keys`).
- [ ] **Themes**: Ordner `paneldeck-data\themes` enthält `nord-beispiel.json`; in Einstellungen → Darstellung erscheint „Nord (Beispiel)“; Datei kopieren, Farben ändern, „Neu laden“ → neues Theme in der Liste; defekte JSON-Datei wird ignoriert (kein Absturz).
- [ ] **Schrift**: Cascadia Mono / Hack / Consolas / Lucida Console / Courier New umschalten → Terminal ändert die Schrift sofort, Zeichenraster bleibt sauber (kein Überlappen, Rahmenzeichen lückenlos).
- [ ] **Claude-Dateien**: Dokument-Symbol (oder Strg+Umschalt+K) → Fenster listet CLAUDE.md und settings-Dateien (Projekt und global); vorhandene Dateien laden; bei fehlenden: Text eingeben → Speichern legt Datei (und Ordner `.claude`) an; ungültiges JSON in settings.json wird beim Speichern abgelehnt mit Fehlermeldung; Wechsel zwischen Dateien behält ungespeicherte Änderungen („•“ markiert).
- [ ] Einstellungen-Fenster bei kleiner Fensterhöhe: Inhalt scrollt, Fenster bleibt sichtbar.

## 14 · Härtung (0.10.0)
- [ ] **WSL + Claude** (Voraussetzung: WSL-Distribution mit installiertem `claude`): Claude-Sitzung in einer WSL-Shell starten → Statuspunkt/Pille, Token/Kosten und Git-Info erscheinen wie bei PowerShell. Falls nicht: *Diagnose kopieren* und Beschreibung melden (WSL-Interop/WSLENV).
- [ ] WSL: App schließen und neu starten → Claude-Sitzung wird mit `--resume` wiederhergestellt.
- [ ] **Diagnose**: Einstellungen → Allgemein → *Diagnose kopieren*, in Notepad einfügen: Version, Shells, Schriften, Einstellungen sind plausibel.
- [ ] **Beschädigte Datei**: App beenden, `state.json` mit Notepad zerstören (z. B. nur `{`), starten → App startet leer (kein Absturz), neben `state.json` liegt `state.defekt.json`.
- [ ] **Absturzprotokoll**: Nach normalem Betrieb existiert keine `crash.log`. (Falls doch: Datei mitschicken.)
- [ ] Dauerlauf: 6–8 Sitzungen, eine davon mit viel Ausgabe (`Get-ChildItem C:\ -Recurse` oder `claude`), eine Stunde offen lassen → Speicher bleibt stabil (Task-Manager), keine Hänger.

## 15 · Emoji, Preise, Tray, WSL-Rückfall (0.11.0)
- [ ] **Emoji-Sequenzen** (`echo` oder Text einfügen): `👨‍👩‍👧 👩‍💻 🏳️‍🌈` → je ein Bild; `🇩🇪 🇺🇸 🇯🇵` → Flaggen; `👍🏽 👋🏻` → mit Hautfarbe; `1️⃣ #️⃣` → Keycaps. Nach den Sequenzen entstehen leere Zellen (Terminalraster) – das ist erwartet.
- [ ] **Preise**: Einstellungen → Claude → Preise: Sonnet-Input auf 6,00 setzen → angezeigte Kosten einer Sitzung verdoppeln sich ungefähr sofort; „Auf Standardwerte zurücksetzen“ stellt sie wieder her.
- [ ] **Tray → Schließen**: Einstellungen → Fenster → „Schließen → Tray“ an; X-Button → Fenster weg, kein Taskleistenbutton, Tray-Symbol bleibt; Claude-Sitzung im Hintergrund auf Freigabe laufen lassen → Toast erscheint; Klick aufs Tray-Symbol → Fenster kommt zurück (auch Taskleistenbutton wieder da).
- [ ] **Tray → Minimieren**: „Minimieren → Tray“ an; Minimieren-Button → Fenster verschwindet aus der Taskleiste; Tray-Klick holt es zurück.
- [ ] Tray-Menü *Beenden* beendet die App vollständig (Task-Manager: kein Prozess übrig) und speichert den Zustand.
- [ ] **WSL-Rückfall**: In einer WSL-Claude-Sitzung einen Prompt senden → Token/Kosten erscheinen in der Kopfzeile (auch wenn die Statuspille fehlt oder nur „arbeitet/bereit“ zeigt).

## 16 · Terminal: Links, Shell-Integration, Suche (0.12.0)
- [ ] **Links**: `echo https://example.com` und `echo C:\Windows\System32\drivers\etc\hosts` → bei gedrückter Strg-Taste unterstrichen + Handzeiger; Strg+Klick öffnet Browser bzw. Standardprogramm; Klick ohne Strg markiert nur Text. Relativer Pfad (`echo src\main.rs:12`) in einem Projektordner wird erkannt, nicht vorhandene Pfade nicht. Ordnerpfad → öffnet Explorer.
- [ ] **PowerShell 7 und Windows PowerShell**: Neue Sitzung → Prompt sieht aus wie immer (auch mit Oh-My-Posh/Starship, falls vorhanden); `cd C:\Windows` → Seitenleisten-Untertitel wechselt zu `Windows`; Git-Info/Dateibaum folgen dem Ordner. Kein Eintrag in `$PROFILE` entstanden.
- [ ] **CMD**: `cd C:\Windows` → Seitenleiste zeigt `Windows`.
- [ ] **Git Bash / WSL**: `cd /tmp` (bzw. `cd ~`) → Ordner wird übernommen (WSL: Dateibaum/Git zeigen den Distro-Pfad, ggf. nur lesbar).
- [ ] **Befehlsmarken**: ein paar Befehle ausführen, dann Strg+Umschalt+↑ → springt zum vorherigen Prompt, ↓ zum nächsten.
- [ ] **Befehl fertig**: Einstellungen → „Befehl fertig melden“ auf 5 s; `Start-Sleep 8` ausführen und das Fenster in den Hintergrund legen → Toast mit Dauer; `cmd /c exit 3` o. ä. → „Befehl fehlgeschlagen“ bzw. `Exit 3` in der Seitenleiste.
- [ ] **Shell-Integration aus**: Schalter ausschalten, neue Sitzung → keine Ordner-/Befehlsfunktionen, Terminal normal.
- [ ] **Alle durchsuchen** (Strg+Umschalt+G): in zwei Sitzungen verschiedenen Text ausgeben, nach gemeinsamem Wort suchen → Treffer beider Sitzungen; Klick springt hin und markiert.
- [ ] **Verlauf speichern**: Rechtsklick → „Verlauf als Datei speichern …“ → Speichern-Dialog, Datei enthält den Verlauf.

## 17 · Claude-Workflow (0.13.0)
Voraussetzung: `git` im PATH, `claude` angemeldet.
- [ ] **Warteschlange**: Rechtsklick auf Claude-Sitzung → Warteschlange: zwei kurze Prompts hinzufügen (z. B. „Sage nur A“ / „Sage nur B“) → der erste läuft, nach Claudes Antwort wird der zweite automatisch gesendet; Seitenleiste zeigt `Queue 1` dazwischen; am Ende Hinweis „Warteschlange abgearbeitet“ (im Hintergrund: Toast).
- [ ] Warteschlange pausieren/fortsetzen, umsortieren, Eintrag bearbeiten/löschen; Neustart der App → Warteschlange ist noch da.
- [ ] **Freigabe-Inbox**: Claude bitten, einen Befehl auszuführen, der eine Freigabe verlangt (z. B. `echo hi > test.txt` im Standard-Permission-Mode) → Karte „Wartet auf dich“ erscheint; *Ja* führt aus (Claude macht weiter); bei erneuter Anfrage *Nein* → Claude bekommt Ablehnung. *Öffnen* springt zur Sitzung.
- [ ] **Worktree**: Neue Claude-Sitzung in einem Git-Repo mit „Eigener Worktree“ → Ordner `<repo>-worktrees\<branch>` entsteht, Branch wird angelegt, Sitzung startet dort. Beim Schließen: *Behalten* lässt Ordner/Branch, *Entfernen* löscht beides (bei Änderungen erst „verwerfen“ ankreuzen), *Mergen & entfernen* führt den Branch im Haupt-Repo zusammen und räumt auf (Konfliktfall: Meldung, nichts gelöscht).
- [ ] **Diff**: Datei im Repo ändern → Symbol „Änderungen“ (Strg+Umschalt+H) zeigt sie; Datei anklicken → Diff farbig; Kästchen = vormerken/zurücknehmen; Commit-Nachricht + *Committen* → Commit (in `git log` prüfen); neue (untracked) Datei zeigt Inhalt als `+`-Zeilen.
- [ ] Diff aktualisiert sich selbst, wenn Claude währenddessen Dateien ändert.

## 18 · Composer & Profile (0.14.0)
- [ ] **Composer**: Strg+Umschalt+M → Fenster mit Fokus im Textfeld; zwei Zeilen tippen (z. B. `echo a` / `echo b`), Strg+Enter → beide laufen in der fokussierten Sitzung, Fenster schließt; Umlaute/AltGr (`@`, `€`) funktionieren; „Nur einfügen“ fügt ohne Enter ein; „An alle sichtbaren“ sendet in alle Bereiche; „Als Prompt speichern“ erscheint in der Bibliothek; Entwurf übersteht einen Neustart.
- [ ] Mehrzeiliger Prompt in einer **Claude-Sitzung** (3–4 Zeilen) kommt komplett als ein Prompt an (nicht zeilenweise abgeschickt).
- [ ] **Profile**: Einstellungen → Profile → „+ Neues Profil“: Name `Prod`, Theme `Dracula`, Schriftgröße 16, Farbe rot, Umgebung `HUB_TEST=hallo`, Startbefehl `echo %HUB_TEST%` (CMD) bzw. `echo $env:HUB_TEST` (PowerShell) → neue Sitzung mit diesem Profil (Dialog → Profil) zeigt eigenes Theme/Schrift, rote Farbmarke und die Ausgabe `hallo`.
- [ ] **Eigenes Programm**: Profil mit Programm `wsl.exe` und Argumenten `-d <deine Distro>` (oder `cmd.exe`) startet korrekt; Rechtsklick auf Projekt → *Standardprofil* → neue Sitzungen im Projekt nutzen das Profil.
- [ ] **Administrator**: Profil mit „Als Administrator“ → UAC-Abfrage, danach ein separates, erhöhtes Terminalfenster; die Hub-Sitzung zeigt den Hinweistext.
- [ ] Teilen (Strg+Umschalt+D) nach einem `cd C:\Windows` → neuer Bereich startet in `C:\Windows` und erbt das Profil.

## 19 · Installer, Windows-Integration, Rand-Knöpfe (0.15.0)
- [ ] **Rand-Knopf**: Maus nah an den rechten/unteren/linken/oberen Rand eines Terminals → orangener „+“-Knopf erscheint mittig am Rand; Klick → neues Terminal an dieser Seite, fokussiert; bei gedrückter Maustaste (Text markieren am Rand) erscheint kein Knopf.
- [ ] **Installer**: `Paneldeck-Setup-….exe` starten (SmartScreen → „Trotzdem ausführen“) → Komponentenauswahl (Desktop, Autostart, Kontextmenü); Installation ohne Admin-Abfrage nach `%LOCALAPPDATA%\Programs\Paneldeck`; Startmenüeintrag „Paneldeck“ startet die App; Eintrag in „Apps & Features“.
- [ ] **Kontextmenü**: Rechtsklick auf einen Ordner im Explorer → „In Paneldeck öffnen“ öffnet eine Sitzung in diesem Ordner; „Mit Claude in Paneldeck starten“ startet dort `claude`; Rechtsklick auf Ordnerhintergrund funktioniert ebenso. Läuft Paneldeck schon, wird **dieselbe** Instanz genutzt (kein zweites Fenster).
- [ ] **Autostart**: Setup-Option bzw. Einstellungen → Fenster → „Mit Windows starten“ → nach Neuanmeldung startet Paneldeck minimiert im Tray.
- [ ] **Einstellungen → Fenster**: Schalter für Autostart und Kontextmenü spiegeln den echten Zustand („Status aktualisieren“); Ausschalten entfernt die Einträge (Registry/Explorer).
- [ ] **Zweiter Start**: `paneldeck.exe` doppelt starten → es erscheint kein zweites Fenster, das vorhandene kommt nach vorn.
- [ ] **Deinstallation** (Apps & Features): entfernt Programm, Startmenü, Kontextmenü, Autostart; fragt nach den Daten (`paneldeck-data`).
- [ ] **Info-Seite**: Version, Ordner, „Lizenzen anzeigen“ zeigt die Liste.

## 20 · Hotfix 0.15.1
- [ ] **Hooks bei laufender App**: In einer Claude-Sitzung einen Prompt senden → Statuspunkt wechselt auf „arbeitet“ und danach auf „bereit“ (das war in 0.15.0 defekt).
- [ ] **Helles Theme** (Einstellungen → Darstellung → „Hell / Light“): Tab-Beschriftungen der Einstellungen und Projekt-Pillen in der Seitenleiste sind gut lesbar.

## 21 · Sicherheit & Shell-Integration (0.15.2)
- [ ] **Exit-Code**: In Git Bash/WSL `false` und `(exit 42)` → Seitenleiste zeigt `Exit 1` bzw. `Exit 42`; mit `ls`+`echo $?` ändert sich `$?` nicht durch den Hook.
- [ ] **Befehl fertig**: Einstellungen → „Befehl fertig melden“ auf 5 s; in Git Bash `sleep 8` mit Fenster im Hintergrund → Toast mit Dauer ≈ 8 s (nicht länger durch Tippzeit); ein kurzer Befehl nach langem Tippen löst keinen Toast aus.
- [ ] **Claude-Start mit Prompt**: Neue Claude-Sitzung mit Start-Prompt `Erkläre $(echo hallo) und \`date\`` → der Text kommt **wörtlich** bei Claude an (keine Shell-Ausführung); der Prompt erscheint als erster Eintrag der Warteschlange und wird gesendet, sobald Claude bereit ist.
- [ ] **Modell-Feld**: `opus; echo X` im Neu-Dialog → es wird nichts ausgeführt (Option wird verworfen/bereinigt).
- [ ] **Strg+Klick**: auf eine `.bat`/`.exe`-Datei im Terminal → wird NICHT gestartet, sondern im Explorer markiert; auf `.txt`/`.md` → öffnet im Standardprogramm; ein Text wie `\\server\share\x.txt` wird nicht als Link unterstrichen.
- [ ] **PowerShell-Profil mit Argumenten**: Profil mit Programm `pwsh.exe` und Argumenten `-NoLogo -Command "Write-Host hi"` startet ohne Fehler.
- [ ] **WSL-Profil**: Profil mit Programm `wsl.exe`, Argumente `-d <Distro>`, Umgebung `FOO=bar` → `echo $FOO` in der Sitzung ergibt `bar`; Ordner-Tracking und Claude-Status funktionieren.
- [ ] **Editor**: eine Latin-1-`CLAUDE.md` (Notepad „ANSI“) öffnen → Warnung, Speichern gesperrt.
- [ ] **Slowloris/Kontextmenü**: Explorer-Kontextmenü „In Paneldeck öffnen“ funktioniert, auch während andere Programme viele Verbindungen öffnen (kein Hänger).

## 22 · Robustheit & Bedienbarkeit (0.15.3)
- [ ] **Defekte Zustandsdatei**: `state.json` im Datenordner mit Müll füllen, App starten → Hinweis erscheint, App startet leer, Datei `state.defekt-….json` liegt daneben.
- [ ] **Unsinnige Einstellungen**: in `settings.json` `font_size` auf 1000 und `opacity` auf 0 setzen → App startet mit begrenzten Werten (Schrift ≤ 40, Deckkraft ≥ 30 %).
- [ ] **UI-Skalierung**: Einstellungen → Darstellung → Regler auf 1,5 → ganze Oberfläche wächst; Strg+Plus ändert nur die Terminalschrift, nicht die UI.
- [ ] **Kleines Fenster** (z. B. 900×500): Einstellungen scrollen, Dialog ragt nicht aus dem Fenster.
- [ ] **Esc**: schließt Einstellungen/Diff/Composer; im Composer-Textfeld zuerst Fokus weg, dann schließen; beim Aufnehmen eines Tastenkürzels bricht Esc nur die Aufnahme ab.

## 23 · Workflow-Sicherheit (0.15.4)
- [ ] **Freigabe-Schutz**: Inbox „Ja“ unmittelbar (<0,5 s) nach Auftauchen der Frage → ignoriert; danach funktioniert es.
- [ ] **Projekt löschen**: Rechtsklick auf Projekt → erster Klick ändert nur die Beschriftung („Wirklich löschen?“), zweiter löscht; nach 4 s ist die Bestätigung wieder weg.
- [ ] **Kopfzeile**: Sitzung mit sehr langem Namen in schmalem Split → Name endet mit „…“, Pille nicht überlagert; Fenster breit ziehen → Pille, Token/Kosten, Git erscheinen wieder.

## 24 · Lesbarkeit (0.15.5)
- [ ] **Kontrast**: `printf '\e[34mblau\e[0m \e[30;40mschwarz auf schwarz\e[0m'` (bash) in dunklem und hellem Theme → Text bleibt lesbar; `ls --color` und Claude-Ausgaben sehen sonst unverändert aus.
- [ ] **Auswahl**: farbigen Text markieren → bleibt lesbar.

## 25 · Ordnerbaum (0.16.0)
- [ ] **Umschalten**: Icon neben dem Filterfeld → Baum; nochmal → Liste. Auswahl bleibt nach Neustart und ist pro Projekt getrennt.
- [ ] **Ordner anlegen**: Ordner-Plus-Icon → „Backend“; Rechtsklick → „Neuer Unterordner“ → „API“. Verschachtelung 3+ Ebenen funktioniert; Auf-/Zuklappen bleibt nach Neustart.
- [ ] **Drag & Drop**: Sitzung auf Ordner ziehen → landet dort; auf freie Fläche → „Kein Ordner“; Esc während des Ziehens bricht ab. Sitzung auf ein Terminal-Pane ziehen funktioniert weiter wie vorher.
- [ ] **Sitzungsmenü** „In Ordner verschieben“ listet alle Ordner (auch leere).
- [ ] **Neue Sitzung hier**: Ordner mit Arbeitsordner/Profil (Menü „Vorgaben“) → neue Sitzung startet dort mit Profil; Unterordner erbt die Vorgabe.
- [ ] **Raster**: „Als Raster öffnen“ zeigt alle laufenden Sitzungen des Ordners nebeneinander.
- [ ] **Prompt an alle**: Text geht an alle laufenden Sitzungen im Ordner (inkl. Unterordner).
- [ ] **Löschen/Schließen**: erster Klick ändert nur die Beschriftung, zweiter führt aus; Ordner löschen lässt Sitzungen eine Ebene höher zurück.
- [ ] **Umbenennen**: Ordner mit Unterordnern umbenennen → alle Sitzungen folgen; doppelter Name wird abgelehnt.
- [ ] **Migration**: bestehende Gruppen aus 0.15.x erscheinen als Ordner.
- [ ] **Sammelstatus**: Claude-Sitzung in Ordner wartet auf Freigabe → roter Punkt an der Ordnerzeile (auch zugeklappt).
- [ ] **X im Terminal-Kopf**: Klick auf X fragt wie „Sitzung schließen“ (bei laufender Sitzung); Strg+Umschalt+W blendet nur aus.

## 26 · Hotfix UI-Skalierung (0.16.1)
- [ ] Regler „UI-Skalierung“ ziehen → Oberfläche bleibt während des Ziehens unverändert und skaliert erst beim Loslassen; Klick auf die Leiste und Pfeiltasten wirken sofort.

## 27 · Vorlagen, Kosten, Verlaufssuche (0.17.0)
- [ ] **Vorlage speichern/starten**: mehrere Sitzungen in Ordnern anlegen → Vorlagen-Symbol → Namen eingeben → Speichern; Vorlage starten → alle Sitzungen entstehen mit Ordnern/Profilen/Arbeitsordnern; mit „Raster“ erscheinen sie nebeneinander. Gleicher Name überschreibt; Löschen braucht zwei Klicks.
- [ ] **Ordner als Vorlage**: Rechtsklick auf Ordner → „Als Vorlage speichern …“ enthält nur dessen Sitzungen.
- [ ] **Befehlspalette**: „Vorlagen …“, „Vorlage starten: …“, „Kosten-Übersicht“ vorhanden.
- [ ] **Kosten-Übersicht**: mit mehreren Claude-Sitzungen öffnen (Klick auf Σ unten links): Zahlen plausibel, Balken für heute hervorgehoben, Ordnerkosten summieren Unterordner.
- [ ] **Tagesbudget**: Einstellungen → Claude → Tagesbudget auf 0,01 $ → nach etwas Claude-Nutzung erscheint genau einmal pro Tag ein Hinweis (Toast, wenn Fenster im Hintergrund).
- [ ] **Volltextsuche**: Claude-Verlauf → Begriff aus einem alten Gespräch eingeben → „Volltext“/Enter → Treffer mit Ausschnitt; „Fortsetzen“ öffnet die Konversation; ✕ beendet die Suche.
- [ ] **Ordner verschieben**: Rechtsklick auf Unterordner → „Verschieben nach“ → anderer Ordner / „Oberste Ebene“; Sitzungen wandern mit; Verschieben in den eigenen Unterordner ist nicht angeboten.
- [ ] **„+“ am Rand**: Maus an den Rand eines Terminals → „+“ erscheint → Klick erzeugt ein neues Terminal an dieser Seite (links/rechts/oben/unten); Textmarkieren am Rand lässt das „+“ nicht aufblitzen.
- [ ] **Ablage-Symbole**: Sitzung aus der Seitenleiste auf ein Pane ziehen → Symbole an allen Rändern, aktives Ziel groß mit Beschriftung; Loslassen führt Teilen/Ersetzen aus; Esc bricht ab.

## 28 · Ordner ziehen (0.17.1)
- [ ] **Ordner → Ordner**: Ordner auf einen anderen ziehen → wird Unterordner (Sitzungen wandern mit); auf freie Fläche → oberste Ebene; auf eigenen Unterordner → kein Ziel; Esc bricht ab.
- [ ] **Ordner → Ansicht**: Ordner mit 3 laufenden Sitzungen auf den rechten Rand eines Terminals ziehen → alle 3 erscheinen als Raster rechts daneben; Mitte → ersetzt das Ziel; Beendete Sitzungen werden nicht geöffnet; leerer Ordner → Hinweis.
- [ ] Klick (ohne Ziehen) klappt den Ordner weiterhin auf/zu.

## 29 · Ablage-Linie im Baum (0.17.2)
- [ ] Sitzung im Baum ziehen → Linie mit Punkt erscheint an der Einsortier-Stelle; auf Unterordner-Ebene ist sie eingerückt, auf oberster Ebene nicht.
- [ ] Ordner ziehen → Linie steht zwischen den Unterordnern alphabetisch richtig, bzw. vor den Sitzungen des Zielordners.
- [ ] Eingeklappter Zielordner → nur Rahmen, keine Linie; Loslassen verschiebt trotzdem.
- [ ] Freie Fläche → Linie am Ende der obersten Ebene.
- [ ] (0.17.3) Auf einen leeren Ordner ziehen: nur Rahmen, keine Linie.
- [ ] (0.17.3) Auf einen Ordner mit Inhalt ziehen, unter das letzte Element: Linie liegt innerhalb des Rahmens.

## 30 · Windows-Test vor 1.0 (0.18.0 / 0.19.0 – nur unter Windows prüfbar; Entwicklung läuft jetzt nativ unter Windows, `cargo build --release` genügt)
- [ ] **Strg+C**: in `ping -t localhost` unterbricht es; mit markiertem Text kopiert es stattdessen (und hebt die Markierung auf). **Strg+X** in `nano`/WSL; **Strg+Einfg** kopiert, **Umschalt+Einfg** fügt ein; **Umschalt+Entf** sendet Entf.
- [ ] **Bild einfügen**: Screenshot in die Zwischenablage (Win+Umschalt+S) → Strg+V im Claude-Terminal fügt den Pfad der PNG-Datei ein; mit Text in der Zwischenablage kommt der Text. Zwei Bilder kurz nacheinander → zwei verschiedene Dateien.
- [ ] **Beendete Shell**: in einem Terminal `exit` → Leiste „beendet“ erscheint (ConPTY meldet kein EOF; der Prozessstatus wird abgefragt).
- [ ] **Synchronized Update**: `printf '\e[?2026hTest'` in WSL/Git Bash → „Test“ erscheint nach < 1 s.
- [ ] **Esc im Vorlagen-Fenster** schließt es, ohne dass die App hängt.
- [ ] **Einzelinstanz**: Explorer → mehrere Ordner markieren → „In Paneldeck öffnen“ (Hub nicht gestartet) → nur ein Fenster, alle Ordner geöffnet. Zweiter Start der .exe holt das laufende Fenster nach vorn.
- [ ] **Kontextmenü auf Laufwerk**: Rechtsklick in den Hintergrund von `C:\` → „Mit Claude in Paneldeck starten“ startet in `C:\` mit Claude.
- [ ] **Deinstallation**: danach enthält `~/.claude/settings.json` keine Hub-Hooks mehr (Fremd-Hooks bleiben); leeres Programmverzeichnis ist weg.
- [ ] **Admin-Profil**: Hub beenden und neu starten → keine UAC-Abfrage; „Neu starten“ in der Sitzung löst sie aus.
- [ ] **Lange Hooks**: sehr langen Prompt (> 64 KiB einfügen) abschicken → Status wechselt trotzdem auf „arbeitet“ und danach „fertig“.
- [ ] **UI bleibt flüssig** beim Start mit mehreren großen Transkripten und mit einer WSL-Sitzung, deren Distribution gestoppt ist.
- [ ] **Netzwerkpfad im Terminal**: `echo \\\\gibtsnicht\\share\\a.txt` → Strg+Hover über dem Pfad löst keine Netzwerkabfrage aus (kein Hängen).
- [ ] **Worktree „Entfernen“** mit Commit auf dem Branch → Worktree weg, Branch bleibt mit Hinweis; „Mergen & entfernen“ löscht ihn.
- [ ] **Sitzungsbudget** (Einstellungen → Claude) auf 0,01 $ → Hinweis genau einmal je Sitzung.
- [ ] **Fehlerbericht kopieren** (Einstellungen → Info) liefert Diagnose + Ende des Absturzprotokolls.

**Automatischer Lauf am 2026-11-13** (Claude, nativ unter Windows 11, Build 0.19.0, isolierte Kopie mit eigenem Datenordner; Tastatur/Maus per Win32 gesendet, Ergebnis per Screenshot geprüft):
- ✅ Strg+C ohne Auswahl bricht `ping -t` ab; mit Auswahl kopiert es, ping läuft weiter, Auswahl wird aufgehoben. Strg+Einfg kopiert, Umschalt+Einfg fügt ein, Strg+X kommt als `X|Control`, Umschalt+Entf als `Delete|Shift` an (nano unter WSL nicht geprüft).
- ❌→✅ **Bild einfügen** ging nur beim allerersten Strg+V der Sitzung (Kurzschluss in `ctrl_v_pressed`, `PREV` blieb `true`). Behoben nach dem Lauf; danach 3 Bilder im Sekundenabstand und 2 schnell hintereinander = 5 verschiedene PNG-Dateien; Text in der Zwischenablage kommt als Text.
- ✅ Beendete Shell (`exit` → „Prozess beendet. Neu starten“, Neustart funktioniert). ✅ Synchronized Update (PowerShell und WSL). ✅ Esc im Vorlagen-Fenster schließt es, kein Hänger (nur wenn kein Textfeld den Fokus hat – so gewollt).
- ✅ Einzelinstanz: zweiter Start beendet sich; drei gleichzeitige `--open` ergeben ein Fenster mit allen drei Ordnern. (Fenster „nach vorn“ nicht eigens geprüft.)
- ✅ Kontextmenü-Befehl auf `C:\` (`--claude --open "C:\"`) startet Claude in `C:\` in der laufenden Instanz.
- ✅ Deinstallation (Setup lautlos, Schein-Home): Hub-Hooks entfernt, Fremd-Hook und `permissions` bleiben, Startmenü/Desktop/Registry/Programmordner weg. ❌→✅ Eine `settings.json` **mit BOM** (Notepad, PowerShell 5.1 `-Encoding UTF8`) ließ `--uninstall-hooks` scheitern (Hooks blieben stehen); auch Hub-Dateien (`settings.json`, `state.json`, Themes) galten mit BOM als beschädigt. Behoben nach dem Lauf, geprüft: Tagesbudget aus BOM-Datei geladen, keine `.defekt`-Datei.
- ✅ Lange Hooks: 200-KiB-Prompt über den Hook-Client → Status „arbeitet“, danach „bereit“. ✅ Netzwerkpfad: Strg+Hover über `\\gibtsnicht.invalid\share\a.txt` – keine Verzögerung (20/20 Stichproben reagierend).
- ✅ WSL (Ubuntu-24.04): Umlaute/€, Ordner-Tracking (`/tmp`), Exit-Code in der Seitenleiste, Hook-Variablen kommen per `WSLENV` an. ✅ Diagnose kopieren liefert Version, Pfade, Shells, Hook-Server.
- ✅ **Zweiter Lauf (gleicher Tag, danach):** Sitzungsbudget-Hinweis kommt genau einmal je Sitzung (künstliches Transkript, ≈ $15 > $0,01). Fehlerbericht (Button liegt im Tab **Info**, nicht unter Allgemein) enthält das Ende von `crash.log`. Tray: Schließen minimiert das Fenster ohne Taskleisteneintrag, zweiter Start holt es zurück. Abdocken, Eingabe im abgedockten Fenster, Schließen dockt wieder an. Drag & Drop aus dem Explorer (Pfad mit Leerzeichen in Anführungszeichen; WSL: `/mnt/c/...`). Worktree: „Entfernen“ lässt den Branch mit ungemergtem Commit stehen (Hinweis), „Mergen & entfernen“ mergt nach `main` und löscht Branch und Worktree. Admin-Sitzung wird beim Programmstart nicht gestartet, keine UAC-Abfrage. UI mit vier 222-MB-Transkripten: Fenster nach 1,3 s, 0 von 165 Stichproben nicht reagierend, 285 MB RAM.
- ⏳ **Weiterhin nicht geprüft:** UAC-Abfrage bei „Neu starten“ einer Admin-Sitzung (sichere Eingabe, nur per Hand), Claude-Sitzung in WSL (dort ist `claude` nicht installiert), UI mit gestoppter WSL-Distribution, Esc-Abbruch eines Drags, mehrere Dateien gleichzeitig ziehen.

## 31 · Agenten-Schnittstelle / MCP (0.20.0)
Automatisch: `powershell -ExecutionPolicy Bypass -File tools\smoke-mcp.ps1 [-Visible]` (45 Prüfungen, mit `-Visible` 47; Temp-Ordner, keine Maus/Tastatur; `release.sh` ruft es auf).
- [x] **Protokoll**: `initialize` (Versionsaushandlung), `ping`, Notifications ohne Antwort, Fehlercodes -32700/-32601/-32602, `tools/list` je Stufe (Aus: volle Liste, Lesen: 5, Steuern: 14).
- [x] **Stufen**: Aus → `mcp_disabled`; Nur lesen → Steuer-Werkzeuge `forbidden`; per Mausklick in Einstellungen → Agenten umgeschaltet, wirkt sofort, wird in `settings.json` gespeichert; Chip „MCP · ●“ in der Seitenleiste nur bei Lesen/Steuern.
- [x] **Lesen**: `list_sessions` (Status, Kosten, Git), `read_terminal` (Ende/Bildschirm), `get_usage`, `search_history`, `screenshot` (Fenster und Sitzungs-Ausschnitt; minimiert → `window_hidden`).
- [x] **Steuern**: `create_session` (PowerShell, cmd, Git Bash, Claude; ohne Fokusklau; unbekannte Shell/Ordner abgelehnt), `send_text`, `send_keys` (ungültige Taste abgelehnt), `rename_session`, `close_session` (eigene ja, fremde nein), `focus_session`.
- [x] **`run_command`**: PowerShell, cmd, Git Bash, WSL – Ausgabe, Exit-Code (PowerShell/bash), Zeitlimit; mehrere Befehle hintereinander; mehrzeilige Prompts (Git Bash).
- [x] **Claude-Sitzung** (Hooks simuliert): `queue_prompt` sendet sofort, `wait_for_status` (Zeitlimit ohne Hooks, Treffer, asynchron bis Stop-Hook), bei Freigabe-Frage wird nichts gesendet.
- [x] **Echter Agent**: Claude Code (`claude -p … --mcp-config … --strict-mcp-config`) ruft `list_sessions` auf und beschreibt die Sitzungen.
- [x] **Registrierung**: Button *Entfernen* führt `claude mcp remove` im Hintergrund aus und zeigt die Meldung; Deinstallation (`--uninstall-hooks`) entfernt eine auf diese .exe zeigende Registrierung, eine fremde bleibt.
- [ ] **Von Hand (Benutzerebene)**: *Registrieren* klicken → `claude mcp list` zeigt `paneldeck` als verbunden; in einer echten Claude-Code-Sitzung `list_sessions` und `run_command` benutzen.
- [ ] **Grenzen** (bekannt): `run_command` bei PowerShell/cmd mit `quality: approx` (Ausgabe-Abgrenzung geschätzt, PowerShell-Exit-Code nach einem nativen Programm kann veraltet sein); Vollbildprogramme und Claude-Sitzungen werden abgelehnt; Screenshot braucht ein sichtbares Fenster; Tray-/minimierter Hub liefert Text-Werkzeuge, aber keine Screenshots.

## 32 · Agenten-Schnittstelle, Kinder-Sitzungen und Installer-Update (0.21.0 / 1.0.0)
Automatisch: `tools\smoke-mcp.ps1` (57 Prüfungen) und `tools\update-test.ps1` (27 Prüfungen; beide ruft `release.sh` auf; Temp-Ordner, Schein-Profil, keine Eingriffe in echte Einstellungen oder Registry).
- [x] **`whoami`**: ein Agent in einer Paneldeck-Sitzung erkennt sich selbst (Sitzungs-ID, Ordner, Shell); außerhalb von Paneldeck `session: null`. Die Sitzung des Agenten wird nie beschrieben oder geschlossen (`forbidden`).
- [x] **Kinder-Sitzungen**: `create_session` aus einer Agenten-Sitzung heraus legt die neue Sitzung unter dem Ordner der Eltern-Sitzung an (`parent` in der Antwort); `list_sessions` zeigt Eltern/Kinder; der Agent schließt nur Sitzungen, die er selbst angelegt hat (Einstellung „fremde Sitzungen schließen“ ändert das).
- [x] **`run_command` genau**: Ausgabe ohne Prompt-/Echo-Zeilen, Exit-Code bei PowerShell/bash; langsame Prompts (Git Bash) werden über die Echo-Zeile abgegrenzt.
- [x] **Installer-Update** (`update-test.ps1`, alter Installer → neuer Installer): Version, Programmdatei und Lizenzdateien aktualisiert; Nutzerdaten, Autostart und Kontextmenü bleiben erhalten; bei **laufendem** Hub (Datei gesperrt) wird `paneldeck.exe.old` angelegt und vom neuen Hub beim Start gelöscht; Aufräumen ohne Registry-Reste.
- [ ] **Von Hand**: Installer starten und die Sprachauswahl (English/Deutsch) prüfen; auf einem System mit anderer Sprache (z. B. Französisch) erscheint Englisch; Komponentennamen, Kontextmenü-Einträge und die Deinstallations-Verknüpfung folgen der gewählten Sprache.
- [ ] **Von Hand**: portables ZIP entpacken, `paneldeck.exe` starten (Einstellungen → Allgemein → Sprache: Standard English, Umschalten auf Deutsch wirkt sofort; die Daten liegen in `paneldeck-data` neben der .exe).

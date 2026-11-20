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
- [ ] **Zeile in Terminalbreite mit der ConPTY von Windows (2.4.2, #264)** – nur auf einem Windows mit älterem conhost (z. B. Windows 11 26200): Einstellungen → Darstellung → *Mitgelieferten Konsolen-Host verwenden* aus, neue Git-Bash-Sitzung, `clear`. `printf 'x%.0s' $(seq 1 $COLUMNS); sleep 1; echo` → die x-Zeile füllt genau eine Zeile, darunter die Leerzeile, dann der Prompt. Einige Sekunden warten, einen Befehl tippen, der länger als eine Zeile ist (z. B. `echo ok # ` und viele `z`), Enter → das Echo steht genau einmal da, keine doppelte Zeile; danach dasselbe mit `printf '%*s\n' "$COLUMNS" '' | tr ' ' x`. Gegenprobe mit eingeschaltetem Konsolen-Host: unverändert richtig.

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
- [ ] Neustart der App: Claude-Sitzung mit *Fortsetzen nach Neustart = Fortsetzen* startet mit `--continue` (nur wenn es im Ordner schon eine Konversation gibt, sonst neu).
- [ ] **Fortsetzen nach Neustart (2.4.1, #302)** – Sitzungseinstellungen → Reiter Claude → Auswahl *Aus* / *Fortsetzen* / *Konversation auswählen*:
  - [ ] Neue Claude-Sitzung: erster Start ist nur `claude` mit `--settings …` (und Optionen), ohne `--session-id`/`--continue`/`--resume` (Befehlszeile in der Vorschau bzw. Prozessliste).
  - [ ] *Fortsetzen*: Prompt senden, Neu laden → `claude --continue`, dieselbe Konversation erscheint. In einem leeren Ordner: Neu laden startet neu (keine Meldung „No conversation found“).
  - [ ] *Aus*: Neu laden beginnt immer eine neue Konversation.
  - [ ] *Konversation auswählen*: Neu laden → Fenster „Konversation auswählen“ mit den Konversationen des Ordners (neueste zuerst, Titel/Alter/Größe) und „Neue Konversation“; ↑/↓, Enter, Doppelklick; *Starten* → `--resume <id>` der gewählten; *Abbrechen*/Esc oder „Neue Konversation“ → neu; Ordner ohne Konversation → ohne Rückfrage neu. Während der Auswahl zeigt der Bereich „Wartet auf die Auswahl der Konversation …“.
  - [ ] Zwei Sitzungen mit *Konversation auswählen*, App neu starten (globale Einstellung „beim Start fortsetzen“ an): die Auswahl erscheint nacheinander, je Sitzung einmal.
  - [ ] WSL-Sitzung mit *Konversation auswählen*: die Liste kommt von der Linux-Seite (`~/.claude/projects`), die Oberfläche hängt dabei nicht.
  - [ ] Nach `/exit` im gestarteten Claude von Hand `claude` starten und einen Prompt senden: Die Sitzung übernimmt diese Konversation nicht als ihre eigene (*Konversation auswählen* zeigt weiter die der Sitzung vorausgewählt). Bei *Fortsetzen* nimmt `--continue` trotzdem die neueste des Ordners – also auch die von Hand gestartete (so gewollt). „Nie übernommen“ gilt nur für *Aus* und für Neustarts ohne Fortsetzen.
  - [ ] *Konversation auswählen*, während eine der Konversationen in einer anderen Sitzung läuft: Eintrag „Läuft in Sitzung <Name>“, nicht wählbar, ↑/↓ überspringen ihn.
  - [ ] Während die Auswahl offen ist, bekommt das Terminal kein Enter/Esc; im Bereich der wartenden Sitzung: *Auswahl zeigen* (holt das Hauptfenster, auch aus dem Tray) und *Ohne Auswahl neu starten*. Hauptfenster im Tray: Hinweis/Toast, dass eine Auswahl wartet.
  - [ ] Dialog „Neue Sitzung“ → *Fortsetzen – auch beim ersten Start (letzte im Ordner)*: erster Start mit `--continue`.
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
- [ ] WSL: App schließen und neu starten → Claude-Sitzung (*Fortsetzen*, nach einem Prompt) wird mit `--continue` wiederhergestellt.
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
- [ ] **X im Terminal-Kopf**: Klick auf X fragt wie „Sitzung schließen“ (bei laufender Sitzung); Strg+Umschalt+W minimiert nur (ab 2.2.0, siehe Abschnitt 60).

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

## 32a · Agenten-Werkzeuge für Todo-Liste und Tickets (Modus-abhängig)
Automatisch: `tools/smoke-mcp.ps1` (Abschnitte „Aufgaben-Werkzeuge“; der Ticket-Teil präpariert die Test-Datenbank per Python/sqlite3 mit dem Projekt `PAN`, Typ „Task“ mit Workflow, Pflichtfeld „Area“, Übergangsregel „Result“, Dropdown „Priority“, und schaltet den Expertenmodus ein) und die Unit-Tests `planning::agent`, `mcp`, `hooks`.
- [x] **Der Agent hat nur die Möglichkeiten des eingestellten Modus**: Expertenmodus aus → `tools/list` enthält `list_todos`, `add_todo`, `set_todo_done`, keine Ticket-Werkzeuge; an → Ticket-Werkzeuge (`list_projects`, `get_project`, `search_tickets`, `get_ticket`, `get_time_entries`, `create_ticket`, `update_ticket`, `transition_ticket`, `add_comment`, `log_time`, `link_ticket`, `link_terminal`) und `list_todos` mit `read_only: true`; ein Werkzeug des anderen Modus liefert `wrong_mode` (Meldung erklärt den Modus). Stufe „Nur lesen“: nur lesende Werkzeuge (`forbidden` vor `wrong_mode`).
- [x] **Todo-Liste**: Karte anlegen (Tags automatisch), abhaken/wieder öffnen, Filter (Tags, Zustand, Seiten); **kein** Werkzeug löscht Karten oder Tickets und keines ändert Typen/Workflows/Projekte.
- [x] **Ticket-Regeln wie im Fenster**: Pflichtfelder (`required_missing` mit `details.missing_fields`), nur Felder des Typs, Hierarchie-Regeln, Workflow (`transition_not_allowed`, `conditions_not_met`, `transition_needs_fields` – die Felder lassen sich in derselben Operation mitgeben), Typ ohne Workflow (`no_workflow`), System-Projekt „Todolist“ schreibgeschützt, Zeit (`1h 30m`, bloße Zahl = Stunden), Verknüpfungen (Duplikat, mit sich selbst).
- [x] **Quelle und Protokoll**: Verlauf zeigt „Agent (Sitzungsname)“; das Aufrufprotokoll (`mcp-log.jsonl`) enthält keine Kommentar-/Beschreibungstexte.
- [x] **`link_terminal`**: eigene Sitzung (Prozessbaum) oder selbst angelegte Sitzung → `session:<id>`; fremde Sitzung → `forbidden`; schreibt nie ins Terminal.
- [ ] **Von Hand – Hinweis (Toast)**: Mit einem Agenten (Stufe „Steuern“) eine Karte bzw. ein Ticket anlegen, ändern, den Status wechseln, kommentieren, Zeit erfassen, verknüpfen. Je Aktion erscheint unten rechts ein kurzer einzeiliger Hinweis mit „Agent (Sitzungsname): …“; bis zu 5 gleichzeitig, jeder ca. 10 s; Aktionen ohne Änderung (z. B. erneut abhaken) erzeugen keinen. Die Todo-Liste bzw. Ticketansicht zeigt die Änderung ohne Neustart.
- [ ] **Von Hand – Einstellungen → Agenten**: Zeile „Aktive Werkzeug-Gruppe“ wechselt mit dem Schalter „Expertenmodus“ (Todo-Reiter) zwischen „Einfache Todo-Liste“ und „Projekte und Tickets“, in Deutsch und Englisch.

## 33 · Version 2.0.0 – Überblick und Vorbereitung
Neu in 2.0.0: Fensterleiste mit Reitern, lokale Datenbank mit Sicherungen, Reiter „Todo-Liste“ (einfach und Expertenmodus), Boards, Terminal-Verknüpfung, Sprints, Abfragesprache, Schnellmenü, Dashboards, Export/Import, MCP-Werkzeuge für Todo-Liste und Tickets. Die Abschnitte 34–46 sind nach Funktionen gegliedert.
- **Vorbereitung A (frischer Start):** `paneldeck.exe` in einen leeren Ordner kopieren, starten – es entstehen `paneldeck-data\paneldeck.db` und (nach der ersten Sicherung) `backups\`. Fenster startet immer auf dem Reiter **Terminal Hub**, Expertenmodus ist aus.
- **Vorbereitung B (Update von 1.0.x):** Datenordner einer 1.0.x-Installation mit Sitzungen, Projekten, Prompts, Themes und `settings.json` von Hand kopieren, 2.0.0 darauf starten → alles ist wie vorher da (Sitzungen, Layout, Verlauf, Prompts, Themes, Einstellungen); neue Einstellungen haben Standardwerte; es entsteht `paneldeck.db`.
- **Vorbereitung C (Testdaten für Tickets):** Tickets für die Prüfungen über die Ticket-Oberfläche (Abschnitt 40, `Strg+N`) oder über einen Agenten (`create_ticket`, Stufe „Steuern“, Expertenmodus an) anlegen bzw. die in `tools\smoke-mcp.ps1` präparierte Datenbank benutzen. Beispielprojekt `PAN` mit den Standard-Typen und ca. vier Tickets: „Fix login“, „Dark mode“, „Login page redesign“, „Export CSV“.
- **Immer beide Sprachen prüfen:** *Einstellungen → Allgemein → Sprache* auf Englisch und Deutsch stellen; kein sichtbarer Text darf nur in einer Sprache erscheinen.
- **Nie** mit dem echten Datenordner testen: Testinstanzen laufen aus einer Kopie der .exe mit eigenem Datenordner. Es darf immer nur **eine** Testinstanz laufen (Fokusklau); nach dem Test beenden.

## 34 · Fensterleiste und Reiter (2.0.0)
- [ ] **Aussehen:** Oben eine eigene Leiste (Höhe ca. 34 px) mit den Reitern „Terminal Hub“ und „Todo-Liste“ (Symbole, aktiver Reiter hervorgehoben), in der Mitte der Fenstertitel, rechts Minimieren/Maximieren/Schließen; keine zweite Windows-Titelleiste. Die bisherige Oberfläche liegt unverändert darunter.
- [ ] **Umschalten:** Klick auf den Reiter; `Strg+1` / `Strg+2`; `Alt+Umschalt+1` / `Alt+Umschalt+2` – auch aus einem Terminal heraus (die Taste kommt nicht im Terminal an). Tooltips der Reiter nennen das Kürzel. *Einstellungen → Tastenkürzel* zeigt die vier Einträge und lässt sie ändern.
- [ ] **Fenster:** Ziehen an der Leiste verschiebt das Fenster; Doppelklick maximiert/stellt wieder her; Ränder und Ecken skalieren (Mauszeiger wechselt); im maximierten Zustand wird nichts am Bildschirmrand abgeschnitten (Inhalt eingerückt); Minimieren/Schließen funktionieren (mit „Beim Schließen in den Tray“ und ohne); der Tray-Wiederherstellen-Weg funktioniert.
- [ ] **Windows 11:** abgerundete Fensterecken (nicht maximiert); Mauszeiger über dem Maximieren-Knopf zeigt nach kurzer Zeit das **Snap-Layouts-Menü**, ein Klick darin ordnet das Fenster an; Win+Pfeil-Tasten und Aero-Snap an den Bildschirmrändern funktionieren. Unter Windows 10 keine Rundung und kein Flyout, sonst alles gleich.
- [ ] **Mehrere Monitore / Skalierung:** Fenster auf einen Monitor mit anderer Skalierung ziehen → Leiste und Knöpfe bleiben scharf und klickbar; die Skalierung der Oberfläche (Einstellungen) ändert auch die Leiste mit.
- [ ] **Abgedockte Fenster** (Sitzung abdocken) bleiben benutzbar: verschieben, schließen (dockt wieder an), Eingabe; der Reiter-Wechsel betrifft nur das Hauptfenster.
- [ ] **Hoher Kontrast / Themes:** Mit einem hellen und einem dunklen Theme sowie dem Windows-Kontrastmodus bleibt die Leiste lesbar; mit eingestellter Fenster-Transparenz bleibt die Leiste lesbar und ohne Artefakte.
- [ ] **Akzentfarbe:** Wechsel in den Reiter „Todo-Liste“ mit einem Expertenprojekt mit Farbe → Akzentfarbe der Oberfläche und der aktive Reiter in der Leiste nehmen die Projektfarbe an; zurück zum Terminal Hub → Standardfarbe.

## 35 · Datenbank, Sicherungen und Wiederherstellen (2.0.0)
Automatisch: Unit-Tests `db`, `backup`. Alles Weitere von Hand (Datenordner-Kopie!).
- [ ] **Anlegen:** Beim ersten Start entsteht `paneldeck.db` im Datenordner; während des Betriebs liegen daneben `paneldeck.db-wal` und `paneldeck.db-shm`, nach dem Beenden verschwinden sie.
- [ ] **Einstellungen → Daten & Sicherung:** Tab vorhanden (Deutsch/Englisch); Intervall (Standard 24 h, 0 = aus), „Behaltene Kopien“ (Standard 2), Ordner (Standard `backups\` im Datenordner), „Ordner wählen …“, „Standardordner“, „Ordner öffnen“ (Explorer), „Jetzt sichern“, Liste der Sicherungen (Datum, Art, Größe), „Wiederherstellen …“.
- [ ] **Jetzt sichern:** erzeugt `manual-JJJJMMTT-HHMMSS.db` und zeigt „Sicherung angelegt: …“; die Liste aktualisiert sich; die Datei lässt sich mit einem SQLite-Werkzeug öffnen. Manuelle Sicherungen werden nie automatisch gelöscht, auch nicht über der Anzahl der Kopien.
- [ ] **Automatisch:** Intervall auf 1 h, `backups\`-Zeitstempel/Datenbank-Erstellungsdatum so manipulieren (bzw. 1 h warten), dass die Sicherung fällig ist → es entsteht eine `auto-…`-Sicherung; bei „Kopien = 2“ bleiben nach der dritten nur die zwei neuesten `auto-…`; Intervall 0 → keine neue.
- [ ] **Nachholen beim Start:** Paneldeck schließen, Zeitstempel der letzten `auto-…`-Sicherung > Intervall zurückliegend (Datei umbenennen), mehrere Intervalle verpasst, starten → **genau eine** neue Sicherung innerhalb etwa einer Minute.
- [ ] **Anderer Ordner:** Ordner wählen (z. B. auf einem anderen Laufwerk) → die nächste Sicherung landet dort; „Standardordner“ springt zurück; ein nicht beschreibbarer Ordner führt zu einer verständlichen Fehlermeldung, nicht zu einem Absturz.
- [ ] **Wiederherstellen:** Todo-Karte anlegen, „Jetzt sichern“, Karte löschen, in der Liste auf „Wiederherstellen …“ → Sicherheitsabfrage; „Wiederherstellen und beenden“ → Paneldeck beendet sich; beim nächsten Start ist die Karte wieder da, und in der Liste steht zusätzlich `pre-restore-…` („Vor Wiederherstellung“). Eine beschädigte oder keine SQLite-Datei (Datei mit Text überschreiben) wird abgelehnt („Sicherung ist nicht lesbar“), Paneldeck beendet sich dann nicht.
- [ ] **Vor Update:** Datenordner einer 1.0.x-Installation ohne `paneldeck.db` → kein Update-Backup (nichts zu sichern). Eine `paneldeck.db` mit älterem Schema (z. B. von einer Vorabversion) → vor dem Start der Migration entsteht `pre-migration-vX-vY-…`; es bleiben höchstens die letzten drei Sicherungen dieser Art.
- [ ] **Neuere Datenbank:** In einer Kopie `PRAGMA user_version = 99` setzen, starten → Fehlermeldung „Die Datenbank hat die Schema-Version 99 …“ (nicht abstürzen, Datenbank unverändert, Terminal Hub bleibt benutzbar).
- [ ] **Portabilität:** Datenordner samt `paneldeck.db` auf einen anderen Rechner/Ordner kopieren → Todo-Liste und Projekte sind identisch vorhanden.

## 36 · Einfache Todo-Liste (2.0.0)
Vorbereitung: Reiter „Todo-Liste“, Expertenmodus aus.
- [ ] **Leerzustand:** Hinweis „Noch keine To-dos. Oben schreiben und Enter drücken.“
- [ ] **Anlegen:** Schnell-Eingabezeile: Text + Enter → Karte erscheint oben/unten wie vorgesehen, Zeile leer, Fokus bleibt. Leere Eingabe + Enter legt nichts an. `Strg+N` setzt den Fokus in die Zeile (aus jedem Zustand des Reiters).
- [ ] **Formular „Mehr …“:** Titel, Text, Tags; nur Titel → Karte ohne Trennlinie und kleiner; nur Text → ebenso; beides → leichte Trennlinie; weder noch → „Anlegen“ gesperrt; `Strg+Enter` legt an; „Weniger“ klappt ein, der Entwurf bleibt.
- [ ] **Abhaken:** Kästchen unten rechts → Titel und Text durchgestrichen und leicht durchsichtig; erneut klicken → wieder offen. Rückgängig (`Strg+Z`) nimmt es zurück.
- [ ] **Bearbeiten:** „Karte bearbeiten“ (Kontextmenü der Karte) ändert Titel/Text/Tags; `Strg+Enter` speichert, `Esc` verwirft.
- [ ] **Sortieren:** Karte am Körper anfassen und ziehen → eine Einfügemarke zeigt die Zielstelle; nach dem Loslassen bleibt die Reihenfolge, auch nach Neustart. Ziehen am Kästchen/an der Mülltonne löst kein Verschieben aus. `Esc` während des Ziehens bricht ab.
- [ ] **Löschen:** Mülltonne oben rechts → Warnung „unwiderruflich“; der Löschen-Knopf ist 3 Sekunden ausgegraut mit Restzeit im Text, danach aktiv; „Abbrechen“/`Esc` bricht ab. Eine gelöschte Karte kommt mit `Strg+Z` nicht zurück.
- [ ] **Tags:** In „Mehr …“ tippen: Vorschläge erscheinen, Tab übernimmt, Enter legt einen neuen Tag an (wird als neu markiert); Groß-/Kleinschreibung zählt nicht (vorhandene Schreibweise gewinnt); Tags erscheinen als farbige Chips auf der Karte; Klick auf einen Chip filtert.
- [ ] **Filter:** Mehrere Tags → nur Karten mit allen Tags; „offen/erledigt“; „Filter löschen“; „Keine Karte passt zu den Filtern.“
- [ ] **Erledigte:** „Erledigte ausblenden“/„anzeigen“ (Meldung „Alle Karten sind erledigt und ausgeblendet.“); „Erledigte löschen“ → Sicherheitsabfrage mit Zähler und 3-Sekunden-Timer.
- [ ] **Tag-Verwaltung:** „Tags verwalten“: Umbenennen (Enter), Farbe ändern, Zusammenführen („Einen Tag mit diesem Namen gibt es schon …“ mit Zähler), Löschen (Hinweis, von wie vielen Karten der Tag entfernt wird); jede Änderung ist mit `Strg+Z` rückgängig zu machen, Löschen eines benutzten Tags nicht endgültig verloren (Karten behalten ihre übrigen Tags).
- [ ] **Rückgängig/Wiederholen:** `Strg+Z`, `Strg+Y`, `Strg+Umschalt+Z`; Schaltflächen „Rückgängig“/„Wiederholen“ mit Tooltip; in einem Textfeld gehört `Strg+Z` dem Feld (außer die Schnell-Eingabezeile ist leer). Nach einer neuen Änderung ist „Wiederholen“ leer. Einstellung *Daten & Sicherung → Änderungshistorie* (3/5/10/25/50/100, Standard 25): bei 3 lassen sich nur drei Schritte zurücknehmen.
- [ ] **Persistenz:** Neustart → Karten, Reihenfolge, Tags, Farben sind wie vorher; die Rückgängig-Historie ist nach dem Neustart leer (so gewollt).
- [ ] **Viele Karten:** 1.000 Karten (z. B. per Agent `add_todo` oder Skript) → Scrollen und Tippen bleiben flüssig.
- [ ] **Deutsch/Englisch:** alle Texte, Tooltips, Dialoge in beiden Sprachen; Karten und Tags behalten ihre Texte.
- [ ] **Datenbank nicht verfügbar** (Datei gesperrt/neuere Version): Der Reiter zeigt „Die Datenbank ist nicht verfügbar – die Todo-Liste kann nicht benutzt werden.“, kein Absturz.

## 37 · Expertenmodus: Projekte, Typen, Felder, Status, Workflows, Vorlagen (2.0.0)
Vorbereitung: Reiter „Todo-Liste“, Schalter **Expertenmodus** im Kopf einschalten.
- [ ] **Schalter:** Standard aus nach frischem Start; einschalten → Projekt-Ansicht (Leerzustand mit „Neues Projekt“); der Zustand bleibt nach Neustart erhalten; ausschalten → einfache Liste mit allen Karten unverändert.
- [ ] **Projekt anlegen (Assistent):** Name, Schlüssel (Vorschlag aus dem Namen, 2–10 Zeichen, nur gültige Zeichen; doppelter Schlüssel wird abgelehnt), Farbe, Beschreibung, Arbeitsordner („Durchsuchen …“), Schalter „Sprints verwenden“, „Versionen/Meilensteine verwenden“, „Farbe neuer Tags“ (Standardfarbe/Projektfarbe/Zufällig), **„Standard-Typen und Workflows anlegen“**: an → Epic, User story, Dev task, Subtask, Defect mit Workflow To Do/In Progress/Done, Hierarchie, Feldern (Description, Priority mit Standard „Medium“/„Normal“, bei User story zusätzlich Epic, Release version nur wenn Versionen an, Story points; bei Defect Steps to reproduce); aus → komplett leeres Projekt ohne Status, Typen und Felder.
- [ ] **Projekt-Auswahl:** Umschalten zwischen mehreren Projekten; die Akzentfarbe des Reiters (und des aktiven Reiters in der Fensterleiste) folgt der Projektfarbe; Projekt-Übersicht zeigt Name, Schlüssel, Farbe, Beschreibung, Arbeitsordner, Zähler.
- [ ] **Projekt-Einstellungen:** alles ändern und speichern; eine Änderung des Kürzels wirkt auf alle Ticket-Kennungen (`PAN-12` → `NEU-12`), ein vergebenes oder ungültiges Kürzel wird mit Meldung abgelehnt; Sprints/Versionen aus- und wieder einschalten → Daten bleiben erhalten.
- [ ] **Projekt löschen:** Dialog mit 3-Sekunden-Timer und Zähler der betroffenen Tickets; danach sind Tickets, Verknüpfungen, Boards weg; andere Projekte unverändert.
- [ ] **Todolist-Projekt:** Im Expertenmodus erscheint „TODO“ schreibgeschützt (Schlüssel `TODO-<n>`, Typ „Todolist“ in Grau, Tags als Labels, erledigt = Status-Kategorie „Done“); keine Änderung möglich; „In Ticket umwandeln“ (nach Merge von Ticket-Aktionen, Abschnitt 40): Titel/Text/Tags werden übernommen, die Karte wird danach als umgewandelt abgehakt, eine zweite Umwandlung ist nicht möglich.
- [ ] **Typ-Editor:** anlegen (Name, Farbe, Symbol optional, Workflow an/aus), umbenennen, löschen, erlaubte Eltern-Typen je Typ (Epic ohne Eltern); Standardnamen sind übersetzt (Deutsch: Epic, Userstory, Dev-Aufgabe, Unteraufgabe, Fehler); nach Umbenennen gilt der eigene Name in beiden Sprachen.
- [ ] **Feld-Editor:** je Typ eine Tabelle; Feld anlegen für **alle zwölf Feldtypen** (Text, Rich Text, Checkbox, Dropdown, Zahl, Ticket-Verweis, Terminal, Version, Meilenstein, Datum, Datum+Uhrzeit, Mehrfachauswahl/Labels); Optionen je Typ (Dropdown-Werte mit Farbe/Reihenfolge, Zahl min/max/Nachkommastellen/Einheit, Textlänge, Rich-Text-Höhe in Zeilen, Ziel-Typen des Ticket-Verweises); Pflichtfeld, Standardwert, Hilfetext, Abschnitt, „auf Karte anzeigen“; ungültige Eingaben (min > max, doppelte Dropdown-Werte, falsche Farbe) → verständliche Fehlermeldung in der Sprache der Oberfläche; Reihenfolge ändern; der Feldtyp lässt sich nach dem Anlegen nicht mehr ändern; **Story points** ist optional und lässt sich löschen.
- [ ] **Status-Editor:** Pool des Projekts (Name, Farbe, Kategorie To Do/In Progress/Done, Reihenfolge); Standardnamen übersetzen sich live (Zu erledigen / In Arbeit / Erledigt); ohne Status arbeiten Typen ohne Workflow.
- [ ] **Workflow-Editor:** Typ wählen, „Workflow aktivieren“ → Status des Typs wählen; Übergangsmatrix (Von × Nach); „Alle erlauben“, „Linearer Ablauf“, „Alles löschen (frei)“; **ohne definierte Übergänge gilt freie Bewegung**; Regeln je Übergang (Zahnrad): **Pflichtfelder**, **Bedingungen** („wenn Feld = Wert → erlauben/sperren“), **Standardwerte setzen**; alles unabhängig voneinander nutzbar; ein Typ ohne Workflow hat kein Status-Feld; „Workflow ausschalten“ fragt vorher; Hinweis, wenn der Workflow unvollständig definiert ist.
- [ ] **Verknüpfungstypen:** mitgelieferte Typen (blockiert/ist blockiert durch, ist in Beziehung mit, hat Duplikat/ist Duplikat von) wechseln mit der Sprache; **Terminal** ist mit Schloss markiert, nicht änderbar, nicht löschbar; eigener Typ mit Ziel Ticket/Terminal/URL/Git-Branch, symmetrisch oder gerichtet; Reihenfolge ändern.
- [ ] **Arten der Arbeit:** Standard Entwicklung/Test/Planung/Schärfung (Deutsch/Englisch); hinzufügen, umbenennen, löschen.
- [ ] **Löschen in Benutzung:** Status, Typ, Feld, Verknüpfungstyp, Art der Arbeit, die noch von Tickets/Feldern benutzt werden → Dialog „Ersatz wählen oder verwerfen“ mit Zähler; Ersatz wählen → Tickets zeigen den Ersatz; bei Typen Zuordnung der Felder; bewusst verwerfen → Werte weg, Warnung vorher.
- [ ] **Vorlagen:** „Typen, Felder und Workflows exportieren …“ schreibt eine JSON-Datei; „Vorlage importieren …“ in ein leeres und in ein bestehendes Projekt: nur Fehlendes wird angelegt, Gleichnamiges wird gemeldet („Konflikte (nicht importiert)“), Bestehendes bleibt unverändert; eine fremde/defekte Datei wird mit Meldung abgelehnt; Dateien mit Umlauten und BOM werden gelesen.
- [ ] **Deutsch/Englisch:** jede Maske, jeder Dialog, jede Fehlermeldung in beiden Sprachen.

## 38 · Kanban-Boards (2.0.0)
Vorbereitung: Projekt `PAN` mit Standard-Typen und ca. 30 Tickets in verschiedenen Status (Abschnitt 33, Vorbereitung C). Reiter „Boards“.
- [ ] **Standard-Board:** Beim ersten Öffnen eines Projekts mit Workflow-Typen liegt ein Board mit je einer Spalte pro Status bereit; Karten zeigen Schlüssel, Titel, Typ-Symbol und die am Feld eingestellten Kartenfelder (Priorität, Story points, Epic) sowie Stern/Flagge; der Spaltenkopf zeigt die Anzahl.
- [ ] **Ziehen erlaubt/verboten:** Karte in eine Spalte ziehen, die der Workflow erlaubt → Status wechselt (Verlauf des Tickets zeigt „Oberfläche“); in eine nicht erlaubte Spalte → die Karte springt zurück, ein Hinweis nennt den Grund (Workflow).
- [ ] **Abbruch:** `Esc` während des Ziehens bricht ab, die Karte bleibt.
- [ ] **Umsortieren:** Karte innerhalb einer Spalte nach oben/unten ziehen → neue Reihenfolge (Backlog-Rang) bleibt nach Neustart und gilt auch in anderen Ansichten.
- [ ] **Auto-Scroll:** Beim Ziehen einer Karte an den Rand einer langen Spalte bzw. des Boards scrollt die Ansicht mit.
- [ ] **Rückgängig/Wiederholen auf dem Board:** Karte in eine andere Spalte ziehen, `Strg+Z` → Karte und Status sind wie vorher (Hinweis „Rückgängig“), `Strg+Y` bzw. `Strg+Umschalt+Z` stellt wieder her; ebenso Umsortieren sowie Stern und Flagge einer Karte. Bei offenem Textfeld, Dialog, laufendem Ziehen oder offenem Schnellmenü passiert nichts.
- [ ] **Version, Meilenstein, Terminal auf der Karte:** In den Kartenfeldern des Boards *Version*, *Meilenstein* und *Terminal* einschalten → die Karten zeigen die Werte (Terminal: Sitzungsname, eine beendete Sitzung grau); ohne Wert bleibt das Feld weg.
- [ ] **Lange Spalten:** Mehr als 25 Karten → „n weitere …“ am Ende der Spalte, ein Klick lädt weitere 25.
- [ ] **Pflichtfeld-Dialog:** Übergang mit Pflichtfeldern (Regel im Workflow-Editor, z. B. „Resolution“ beim Wechsel nach Done) → beim Ziehen öffnet sich „Pflichtfelder ausfüllen“; „Verschieben“ ist erst mit gültigen Werten möglich, „Abbrechen“ lässt die Karte stehen; ein Feld, das hier nicht ausfüllbar ist (z. B. Rich Text), nennt den Weg über das Ticket. Bedingungen eines Übergangs („gesperrt durch eine Bedingung“) werden beim Ziehen erklärt.
- [ ] **Board konfigurieren:** „Neues Board“ / „Bearbeiten“: Name (nicht leer), Beschreibung, Spalten mit **mehreren Status je Spalte** (Status hinzufügen/entfernen, Spalte nach links/rechts, Spalte löschen → Status wandern nach „nicht auf dem Board“), Typ-Filter (mehrere Typen; keine Auswahl = alle mit Workflow), Karteninhalt (Schlüssel, Typ-Symbol, weitere Felder in Reihenfolge); Quelle „Alle Tickets“ (gespeicherter Filter und „Nur aktiver Sprint“ sind grau mit Hinweis, bis Abfrage-Leiste und Sprints da sind – danach hier prüfen).
- [ ] **Konfigurationsfehler:** Board ohne Namen, Spalte ohne Titel/ohne Status, derselbe Status in zwei Spalten → klare Fehlermeldung (Deutsch/Englisch), nichts wird gespeichert.
- [ ] **Mehrere Boards:** zweites Board anlegen, zwischen den Boards wechseln, „Duplizieren“, „Löschen“ (Sicherheitsabfrage: nur das Board, nicht die Tickets).
- [ ] **Schnellfilter:** Textsuche (Titel, Schlüssel, Text), Typ-Filter (mehrere), „nur Favoriten“ (Stern), „nur markierte“ (Flagge), „Filter löschen“; Rechtsklick auf eine Karte → Stern/Flagge setzen und entfernen.
- [ ] **Story-Points-Summe** je Spalte (Tooltip „Summe der Story Points“), Spalten ohne Story-Points-Feld zeigen keine Summe.
- [ ] **Deutsch/Englisch:** Spaltentitel der Standardnamen übersetzen sich beim Sprachwechsel live; Dialoge in beiden Sprachen.
- [ ] **Viele Tickets (10.000):** Board und Scrollen bleiben flüssig.
- [ ] **Von Hand – Boards**: Board bearbeiten → „Quelle“ ist wählbar (Alle Tickets / gespeicherte Filter, global mit Zusatz), „Umfang → Nur aktiver Sprint“ ist in Projekten mit Sprints wählbar (sonst abgeblendet mit Hinweis); danach zeigt das Board nur die Treffer des Filters bzw. die Tickets des aktiven Sprints (ohne aktiven Sprint keine Karten); ein Filter mit ungültiger Abfrage zeigt eine rote Meldung statt Karten. Karten ziehen funktioniert wie zuvor.
- [ ] **Von Hand – Abfragen mit Sprints/Versionen**: In einem Projekt mit Sprints und Versionen: `sprint = activeSprint()`, `sprint IS EMPTY` (Backlog), `sprint = "<Name>"`, `"Release version" = <Version>`, `ORDER BY sprint` liefern die erwarteten Tickets (ohne Sprints im Projekt gibt es das Feld `sprint` nicht).

## 39 · Terminal-Verknüpfung (2.0.0)
Vorbereitung: Expertenmodus an, Projekt `PAN` mit ca. vier Tickets (Abschnitt 33, Vorbereitung C), zwei Sitzungen „A“ und „B“.
- [ ] **Sichtbarkeit:** Das Verknüpfungs-Symbol im Terminalkopf erscheint nur bei eingeschaltetem Expertenmodus **und** mindestens einem Projekt; bei ausgeschaltetem Expertenmodus ist der Terminal Hub unverändert (keine Symbole, kein Chip, keine Ticket-Treffer in der Palette).
- [ ] **Mini-Suche:** Klick auf das Symbol im Kopf von A → Suchfeld mit Fokus; Rechtsklick auf Kopf bzw. Seitenleisten-Eintrag öffnet ein Menü mit „Ticket verknüpfen …“, das dieselbe Mini-Suche öffnet. Eingaben `pan1`, `PAN-2`, `login` finden die erwarteten Tickets (Treffer nach Relevanz: Schlüssel zuerst, dann Titel); `Esc` schließt.
- [ ] **Verknüpfen:** Enter auf einem Treffer verknüpft; ein Hinweis (Toast) bestätigt; der **Chip** mit der Ticket-Kennung (`PAN-2`) erscheint im Terminalkopf und in der Seitenleiste.
- [ ] **Doppelte Verknüpfung:** dasselbe Ticket nochmal wählen → Treffer ist gedimmt, ein Warn-Hinweis erscheint, es entsteht keine zweite Verknüpfung.
- [ ] **Mehrere Tickets:** drei Tickets mit A verknüpfen → Chip „PAN-1 +2“; Tooltip bzw. Liste zeigt alle drei.
- [ ] **Chip anklicken:** öffnet das Ticket im Reiter „Todo-Liste“ (der Expertenmodus wird bei Bedarf eingeschaltet; „Zurück“ bzw. `Esc` führt zum Terminal Hub zurück, bei Ticket-Verweisen erst zum vorherigen Ticket; wird nach dem Öffnen von Hand ein Reiter oder Modus gewechselt, führt „Zurück“ zur Liste); **Entfernen** der Verknüpfung (Kreuz im Chip/Liste) nimmt den Chip weg.
- [ ] **Sitzung schließen:** A schließen → die Verknüpfung bleibt in der Datenbank (Ticket zeigt sie später als „beendet“, grau); Sitzung per „Fortsetzen“/Neustart der App wieder aktiv → Verknüpfung wieder „läuft“ (gleiche Sitzungs-ID).
- [ ] **Befehls-Palette:** `Strg+Umschalt+P`, `pan-2` bzw. `login` → Ticket-Treffer (höchstens 8) unter den Befehlen; Enter öffnet das Ticket und schaltet auf den Reiter „Todo-Liste“.
- [ ] **Feld „Terminal“:** Ein Typ mit Feld vom Typ „Terminal“ (Abschnitt 37) → im Ticket Auswahldialog der laufenden Sitzungen; Auswahl speichert `session:<id>`; ein beendeter Eintrag erscheint grau mit „beendet“.
- [ ] **Sprung zur Sitzung:** Im Ticket mit Terminal-Feld bzw. Terminal-Verknüpfung auf einen **laufenden** Eintrag klicken → der Terminal Hub öffnet und fokussiert diese Sitzung; bei einem beendeten Eintrag (grau) passiert nichts Falsches (kein Absturz, kein Sprung zu einer anderen Sitzung).
- [ ] **Agent:** `link_terminal` verknüpft die eigene Sitzung des Agenten → Chip erscheint ohne Neustart; fremde Sitzung → Fehler.
- [ ] **Deutsch/Englisch** und **schmale Seitenleiste:** Bei schmaler Seitenleiste weicht der Chip dem Namen (kein Überlappen); Texte in beiden Sprachen.

## 40 · Tickets: Anlegen, Detailansicht, Aktionen (2.0.0)
Vorbereitung: Projekt `PAN` mit Standard-Typen (Abschnitt 33, Vorbereitung C), Reiter „Tickets“.
- [ ] **Anlegen-Dialog:** Typ wählen (nur Typen des Projekts, die als Wurzel oder Kind erlaubt sind), Titel, Felder des Typs mit Pflichtfeld-Markierung und Hilfetext, Standardwerte vorbelegt, „Weiteres anlegen“-Haken; Anlegen nur im Reiter „Todo-Liste“; neuer Schlüssel zählt hoch (`PAN-n`), nie wiederverwendet (auch nach Löschen).
- [ ] **Detailansicht:** Standard = eigene Seite mit Zurück-Knopf, per Symbol auf Seitenpanel umschaltbar (und zurück); die Wahl wird gemerkt; alle Felder des Typs nach Abschnitten (ein-/ausklappbar), Status-Auswahl nur mit erlaubten Übergängen (Pflichtfeld-Dialog, Bedingungen), Eltern/Kinder-Liste mit Fortschritt („2 von 5 erledigt“) und „Kind anlegen“, Verknüpfungen (Ticket, Terminal, URL, Branch).
- [ ] **Rich-Text-Felder:** Umschalter „Text | Visuell“: Text = editierbar mit Formatierungsleiste (Fett, Kursiv, Überschrift, Listen, Aufgabenliste, Zitat, Code, Tabelle), Visuell = nur gerendert, nicht editierbar; Ticket-Schlüssel im Text (`PAN-12`) sind klickbar; keine Dateien/Bilder-Anhänge.
- [ ] **Kommentare:** anlegen (Markdown), anzeigen mit Zeit; im Verlauf sichtbar.
- [ ] **Verlauf:** jede Änderung mit Zeit, Feld, alt/neu und **Quelle** (Oberfläche / Schnellmenü / Agent mit Sitzungsname / Import); Mehrfach-Änderungen einer Aktion stehen zusammen.
- [ ] **Rückgängig/Wiederholen:** `Strg+Z` / `Strg+Y` im Ticketbereich (Anlegen, Feldänderung, Statuswechsel, Verschieben); die Anzahl richtet sich nach der Einstellung; nach einer neuen Änderung ist Wiederholen leer; **Löschen ist endgültig** (3-Sekunden-Timer wie bei Karten) und nicht rückgängig.
- [ ] **Todo-Karte umwandeln und rückgängig:** In der Todolist-Übersicht eine Karte in ein Ticket umwandeln → die Karte ist abgehakt und verknüpft; `Strg+Z` (auch in der Übersicht) entfernt das Ticket und öffnet die Karte wieder, `Strg+Y` stellt die Umwandlung wieder her.
- [ ] **Chip-Spalte und Scrollen in der Ticket-Liste:** Bei Fensterbreite ab 900 px zeigt die Liste eine Spalte mit Chips für Version, Meilenstein und Terminal (leer, wenn nichts gesetzt; unter 900 px fehlt sie). Ein weit unten liegendes Ticket öffnen und mit „Zurück“ (`Esc`) schließen → die Liste scrollt so, dass seine Zeile sichtbar ist; liegt die Zeile schon ganz im Bild, bleibt die Liste stehen.
- [ ] **Klonen:** Dialog mit je einem Haken pro Feld (alle an) und angezeigten Werten; Klon bekommt neuen Schlüssel, Startstatus und das Eltern-Ticket des Originals; Option „Kinder mitkopieren“; Kommentare, Zeiten, Verknüpfungen, Stern, Flagge werden nicht kopiert.
- [ ] **Typ ändern / verschieben:** Zieltyp (Standard: aktueller Typ, auch anderes Projekt); Liste der **wegfallenden Felder (mit Werten)** und der **neuen Felder** mit Checkbox „Wert direkt setzen“ und passendem Eingabefeld; Status wird zugeordnet (gleich → gleicher Name → gleiche Kategorie → Start); Projektwechsel vergibt neuen Schlüssel, der Verlauf hält „Verschoben von PAN-12“; Eltern/Kinder werden nur gelöst, wenn die Typ-Regeln es verlangen (Vorschau warnt).
- [ ] **Flagge und Stern:** im Ticket, auf der Board-Karte und in Listen setzen/entfernen; wirken in der Abfragesprache (`IS favorite`, `IS blocked`, `SORT BY favorites`).
- [ ] **Zeiterfassung:** Knopf „Zeit erfassen“ → Dialog: Dauer (`30m`, `1h 30m`, `2d`; nur Zahl = Stunden; ungültig → Meldung), Art der Arbeit (Dropdown aus dem Projekt, Standard Entwicklung), Start (nur Datum → 08:00; leer → jetzt minus Dauer), Kommentar (Rich Text); Eintrag erscheint in der Übersicht mit Gesamtsumme und im Verlauf; Bearbeiten und Löschen mit Bestätigung.
- [ ] **Todo umwandeln:** Im Projekt „TODO“ („In Ticket umwandeln“): Zieltyp/-projekt wählen; Titel (sonst erste Textzeile), Text (in „Description“) und Tags (in das erste Labels-Feld, sonst verworfen und gemeldet) werden übernommen; die Karte wird abgehakt und als umgewandelt gemerkt; nur einmal möglich.
- [ ] **Liste:** Reiter „Tickets“ mit Spalten (Schlüssel, Titel, Typ, Status, Priorität, Punkte) und Sortierung per Klick auf die Spaltenköpfe, Textsuche (Titel, Schlüssel, Text), Typ-/Status-Chips, „nur Favoriten“, „nur Markierte“, „Filter löschen“; Klick auf eine Zeile öffnet das Ticket; 10.000 Tickets bleiben flüssig.
- [ ] **Zurück-Weg:** Ticket aus Board, Suche, Backlog, Dashboard, Übersicht und aus dem Terminal Hub (Chip) öffnen → „Zurück“ bzw. `Esc` führt zur Ausgangsansicht (bei Ticket-Verweisen zum vorherigen Ticket); ein Ticket eines anderen Projekts wechselt das Projekt.
- [ ] **Editoren für Version, Meilenstein, Terminal:** In einem Typ mit Feldern dieser Arten Auswahl öffnen: Versionen/Meilensteine **des Projekts** (leeren geht nur bei Nicht-Pflichtfeldern), Terminal = Liste der Sitzungen; Wert speichern, Ticket neu öffnen → Wert steht da.

## 41 · Abfragesprache, Suche und Hilfeseite (2.0.0)
Automatisch: Unit-Tests `query`, `planning::search`, `planning::query_help` (Parser, Prüfer, Vorschläge, SQL-Übersetzung, jedes Beispiel der Hilfe).
- [ ] **Abfrage-Leiste:** `type = Epic AND "Release version" = 2.0.0 AND status IN (Done, "Development finished") ORDER BY priority` → Treffer wie erwartet; Syntax-Hervorhebung, rote Unterstreichung bei Fehlern mit Meldung in der Sprache der Oberfläche und Vorschlag („meinten Sie …?“); die Position des Fehlers stimmt auch mit Umlauten.
- [ ] **Autovervollständigung:** nach dem Tippen von `st` erscheinen Felder, nach `status =` die Werte, nach `ORDER BY` Felder/Schlüsselwörter, nach `IS` Prädikate (`favorite`, `blocked`), Funktionen mit Klammern; Pfeiltasten/Tab/Enter übernehmen, `Esc` schließt.
- [ ] **Operatoren:** `=`, `!=`, `~`, `!~`, `<`, `>`, `<=`, `>=`, `IN`, `NOT IN`, `IS EMPTY`, `IS NOT EMPTY`, `AND`/`OR`/`NOT`, Klammern, Freitext in Anführungszeichen; `!=` trifft auch leere Felder, `<`/`>` nie; Mehrfachwert-Felder (Labels): `=` = „enthält“; Sortieren nach `ORDER BY`/`SORT BY` mehrerer Felder, leere Werte zuletzt.
- [ ] **Funktionen:** `activeSprint()` (leer ohne aktiven Sprint, kein Fehler), `currentDate()`, `linkedTerminal()` (das Terminal, aus dem gesucht wird).
- [ ] **Grenzen:** sehr langer Text, tief geschachtelte Klammern und sehr viele Bedingungen werden mit einer Meldung abgelehnt; eine absichtlich teure Abfrage bricht nach dem Zeitlimit mit Meldung ab; kein Absturz.
- [ ] **Gespeicherte Filter:** speichern (Name, je Projekt oder global), aufrufen, ändern, löschen; als Quelle eines Boards wählen (Board-Dialog, Abschnitt 38); wird ein Feld/Status gelöscht, auf das ein Filter verweist, meldet der Filter den Fehler statt zu fehlen.
- [ ] **Schlüsselwort-Editor:** eigenes Prädikat/Sortier-Schlüsselwort anlegen (Name, Bedingung, Sortierung), in der Suche benutzen; eingebaute (`favorite`, `blocked`, `favorites`) sind nicht löschbar.
- [ ] **Hilfeseite:** Fragezeichen-Knopf neben dem Suchfeld → Hilfe öffnet sich wie ein Ticket über der aktuellen Ansicht, mit **X oben rechts** statt Zurück-Knopf; Abschnitte Einstieg, Beispiele (einfach bis komplex, jeweils mit Screenshot des Ergebnisses), Fehlersuche, Referenz, Grammatik; Volltextsuche in der Hilfe; Hilfetext zweisprachig, Screenshots nur englisch; Beispiel anklicken übernimmt die Abfrage in die Suche.
- [ ] **Reiter „Suche“:** Abfrage + Enter (oder „Suchen“) → Treffer als Tabelle; Umschalter „Suchen in“ (dieses Projekt / alle Projekte – Felder gleichen Namens werden zusammengefasst); Klick auf eine Zeile öffnet das Ticket (Zurück kommt wieder zu den Treffern); Verlauf mit `↑`/`↓` im Feld; ungültige Abfrage zeigt Meldung statt Treffer. **Abfrage-Leiste in der Ticket-Liste:** eine gültige Abfrage ersetzt dort die lokalen Filter, „Abfrage löschen“ kehrt zurück, ein Projektwechsel löscht die Abfrage.
- [ ] **Screenshots der Hilfe:** Jedes Beispiel hat seine Datei (`Example::screenshot_file`) und zeigt genau die Treffer, die der Text nennt (Abgleich mit der Referenz-Auswertung).

## 42 · Schnellaktionen und Schnellmenü (2.0.0)
- [ ] **Editor (Reiter „Schnellaktionen“):** Schnellaktion anlegen: Name der Schaltfläche (z. B. „Create Devtask“), Aktionsart (**Ticket anlegen**, **Status ändern**, **Terminal-/Claude-Sitzung starten**), je Art die Parameter; bei „Ticket anlegen“: Ziel-Typ, Defaults als hinzufügbare Liste (nur Felder, die der Typ hat), Relation (z. B. „Devtask from“ aus Sicht einer Userstory), Release version, Meilenstein, Titel, Beschreibung (Rich Text = größeres Feld); Duplizieren, Reihenfolge, Löschen.
- [ ] **Ausführen:** Schaltfläche im Ticket legt das Kind-Ticket mit den Defaults an (Pflichtfelder fehlen → Dialog); „Status ändern“ beachtet Workflow und Pflichtfelder; „Sitzung starten“ öffnet eine Sitzung im Arbeitsordner des Projekts, verknüpft sie mit dem Ticket und sendet auf Wunsch den Ticketinhalt als Prompt.
- [ ] **Schnellmenü (`.`):** Im Ticket (Fokus nicht in einem Textfeld) öffnet die Taste `.` – auf deutscher und US-Tastatur ohne Umschalt – das Menü; Eingabe sucht tolerant über **Felder des Tickettyps**, **Status** (nur erlaubte Übergänge) und **Schnellaktionen**; Enter bei einem Feld → Wert direkt in der Menüzeile eingeben (Dropdown, Datum, Zahl, Checkbox je nach Feldtyp), Enter speichert, ungültige Eingabe zeigt eine Meldung; `Esc` schließt; Änderungen stehen im Verlauf mit Quelle „Schnellmenü“.
- [ ] **Gleiche Regeln:** Pflichtfelder/Bedingungen eines Übergangs gelten im Schnellmenü wie im Fenster und für Agenten.
- [ ] **Pflichtfeld-Formular im Schnellmenü:** Status wählen, dessen Übergang Pflichtfelder verlangt → ein kleines Formular fragt sie ab (statt Fehlermeldung); „Abbrechen“ lässt das Ticket unverändert, „OK“ setzt Werte und Status in einem Schritt.
- [ ] **Pflichtfeld-Formular bei Schnellaktionen:** Eine Schnellaktion „Ticket anlegen“ bzw. „Status ändern“ mit leerem Pflichtfeld des Ziel-Typs/Übergangs ausführen → ein Formular fragt genau die fehlenden Pflichtfelder ab; „OK“ legt an bzw. wechselt den Status, „Abbrechen“ ändert nichts.
- [ ] **`Esc`/`Enter` in offenen Auswahllisten:** Im Schnellmenü bei einem Feld mit Auswahlliste (z. B. Priorität): `Esc` schließt nur die Liste (das Menü bleibt), `Enter` übernimmt den markierten Eintrag; ein zweites `Esc` schließt das Menü.
- [ ] **Schnellaktionen als Knöpfe im Ticket:** Ticket eines Typs mit Schnellaktionen öffnen (Seite und Seitenpanel) → unter dem Status steht die Leiste „Schnellaktionen“ mit genau den Aktionen dieses Typs in der Reihenfolge des Editors; ein Ticket eines Typs ohne Aktionen zeigt keine Leiste. Der Hinweistext eines Knopfs nennt, was er tut (Status setzen / Ticket anlegen / Sitzung starten). Klick auf „Status ändern“ verschiebt das Ticket (Hinweis „Rückgängig“ macht es rückgängig, der Knopf verschwindet im Zielstatus und kommt zurück); bei offenem Pflichtfeld öffnet sich das Formular, `Esc` schließt es ganz (nicht zurück zur Liste); „Ticket anlegen“ ohne Titelvorgabe fragt zuerst den Titel; „Sitzung starten“ öffnet die Sitzung wie im Schnellmenü. Ungültige Aktionen sind ausgegraut (Hinweis nennt den Grund). Aktion im Reiter „Schnellaktionen“ umbenennen/umsortieren/löschen/neu anlegen → die Leiste im offenen Ticket zieht nach. Mit mehr als sechs Aktionen übernimmt das Menü „⋯ weitere“ den Rest. Die Taste `.` und `Esc` im Ticket funktionieren unverändert.
- [ ] **Typ-Defaults:** Im Typ-Editor gesetzte Standardwerte aller Felder erscheinen im Anlegen-Dialog und bei „Ticket anlegen“-Aktionen.
- [ ] **Vorlagen-Rundreise:** Typ mit Schnellaktionen exportieren und in ein anderes Projekt importieren → Aktionen sind mitgekommen (mit richtigen Verweisen auf Typ/Felder).

## 43 · Dashboards (2.0.0)
- [ ] **Standard-Dashboard:** eine große Kachel „Suche“; Dashboard-Wechsler (Auswahl), neues Dashboard, umbenennen, löschen (Sicherheitsabfrage), **Stern** markiert das Standard-Dashboard (wird beim Start des Reiters geladen); je Projekt oder **global**.
- [ ] **Kachel hinzufügen:** Suche (Dialog zum Hinterlegen eines Abfragetexts, der beim Öffnen läuft, z. B. für ein Defect-Dashboard), Sprint, Burndown, Zahl/Diagramm (Anzahl nach Status/Typ/Priorität als Torte/Balken), Zeiterfassung (je Tag/Woche/Ticket), Favoriten/Markierte, **Alle Projekte** (Klick springt zum Projekt).
- [ ] **Anordnen:** Kacheln per Drag & Drop verschieben und in der Größe ändern (12-Spalten-Raster, keine Überlappung; Packen schließt Lücken); Layout bleibt nach Neustart.
- [ ] **Export/Import:** Dashboard als JSON exportieren, importieren (immer neu, Name „ (2)“ bei Konflikt), fremde Datei wird abgelehnt.
- [ ] **Leere und fehlerhafte Kacheln:** ungültige Abfrage in einer Kachel zeigt eine Meldung in der Kachel (nicht das ganze Dashboard); Projekt gelöscht → Kachel meldet es.

## 44 · Sprints, Versionen und Meilensteine (2.0.0)
Automatisch: Unit-Tests `planning::sprints` (Sprints, Burndown, Velocity, Versionen).
- [ ] **Optional:** Im Projekt „Sprints verwenden“ **aus** → keine Sprint-Oberfläche, Board und Liste funktionieren ohne; **an** → Backlog und Sprint-Board erscheinen. Schalter „Versionen/Meilensteine“ ebenso; erneutes Ausschalten und Einschalten lässt alle Daten unverändert.
- [ ] **Sprint anlegen:** Name, Ziel, Dauer-Vorlage (z. B. 10 Arbeitstage → Ende = Freitag der 2. Woche), Start/Ende änderbar.
- [ ] **Backlog → Sprint:** Ticket per Ziehen in einen (auch geplanten) Sprint verschieben und zurück; Summe je Sprint (Story points, Ersatzweise Ticketanzahl); Epic-Leiste filtert; Mehrfachauswahl mit Sammelaktion; Drag & Drop-Priorisierung im Backlog.
- [ ] **Rückgängig im Backlog:** Ticket in einen Sprint ziehen, `Strg+Z` → Ticket wieder im Backlog (Burndown/Bericht so, als wäre nichts passiert); `Strg+Y` / `Strg+Umschalt+Z` stellt es wieder her; ebenso „Verschieben nach …“. Nach dem Starten, Abschließen oder Löschen des Sprints lässt sich ein früherer Zug nicht mehr umkehren (`Strg+Z` bleibt wirkungslos); Sprint starten/abschließen/löschen und die Aktionen im Reiter „Releases“ selbst sind nicht rückgängig zu machen.
- [ ] **Sprint-Zeitraum:** Start 01.01.2026, Ende mit Tippfehler (z. B. Jahr 0026 oder mehr als 4000 Tage später) → Meldung, der Sprint wird nicht gespeichert.
- [ ] **Starten:** Sprint starten; ein **zweiter** Sprint im selben Projekt lässt sich nicht starten (Fehlermeldung Deutsch/Englisch).
- [ ] **Im aktiven Sprint:** Ticket erledigen (Status Kategorie „Done“), hinzufügen, entfernen → Burndown zeigt Treppe bzw. Sprung; die Ideal-Linie ist an Wochenenden waagerecht (Wochenenden optional ausgeschlossen); fehlen Story points, zählt die Ticketanzahl.
- [ ] **Abschließen:** offene Tickets nach Wahl ins Backlog oder in den nächsten Sprint; **Sprint-Bericht** (geplant/erledigt/offen); nach drei abgeschlossenen Sprints zeigt die **Velocity** drei Balken.
- [ ] **Versionen und Meilensteine:** anlegen (Name, Fälligkeit), „veröffentlicht“ setzen, Fortschrittsübersicht (erledigte/alle Tickets); im Feld „Release version“ erscheinen nur Versionen **des Projekts**; Löschen einer benutzten Version fragt nach Ersatz oder bewusstem Verwerfen.
- [ ] **Abfrage:** `sprint = activeSprint()` liefert die Tickets des aktiven Sprints; im Board „Nur aktiver Sprint“ (Abschnitt 38) funktioniert jetzt.

## 45 · Export und Import (2.0.0)
Der Import nimmt **genau** das Exportformat von Paneldeck.
- [ ] **Projektsicherung (JSON):** Projekt mit Typen, Feldern, Workflows, Tickets, Kommentaren, Verlauf, Zeiten, Boards, Filtern, Sprints exportieren; die Datei ist gültiges JSON mit Kennung und Format-Version.
- [ ] **Import als neues Projekt:** Import legt **immer ein neues** Projekt an; bei gleichem Kürzel fragt Paneldeck nach einem neuen Kürzel (Tickets werden umnummeriert, Verknüpfungen angepasst); bestehende Daten bleiben unverändert (Gegenprobe: vorher/nachher `paneldeck.db`-Kopie vergleichen).
- [ ] **Tickets als CSV:** Export (eine Zeile je Ticket mit Schlüssel, Typ, Status, Titel und den Feldern; Trennzeichen, Anführungszeichen, Zeilenumbrüche und Umlaute werden korrekt maskiert); Import derselben Datei in ein Projekt mit gleichen Typen; eine fremde CSV (z. B. aus Jira) wird mit klarer Meldung abgelehnt.
- [ ] **Importgrenzen:** Eine Projektsicherung mit Ticketnummer/Zähler nahe `9223372036854775807` oder einem Sprint von mehr als 4000 Tagen wird mit Meldung abgelehnt, es entsteht kein Projekt; eine Datei über 128 MB wird nicht gelesen (Meldung). Beim CSV-Export beginnt eine Zelle mit Tabulator/Wagenrücklauf im „Excel-sicheren“ Modus mit einem Schutzzeichen (kein Formel-Start).
- [ ] **Typ-/Feld-Vorlagen:** wie in Abschnitt 37 (Konflikte werden gemeldet, nie überschrieben).
- [ ] **Dashboards und Boards:** Export/Import als JSON (Abschnitt 43/38); Verweise auf fehlende Status/Felder werden im Bericht gemeldet.
- [ ] **Atomar:** Datei mittendrin abschneiden → Import bricht mit Meldung ab, das Projekt entsteht nicht halb.

## 46 · Agenten-Werkzeuge und Hinweise (2.0.0)
Automatisch: siehe Abschnitt 32a (`tools\smoke-mcp.ps1` mit 136 Prüfungen, Unit-Tests). Zusätzlich von Hand:
- [ ] **Werkzeugzahl:** `tools/list` meldet 33 Werkzeuge in der Stufe „Steuern“ (15 bisherige + 18 neue); mit Expertenmodus aus sind es 18, mit Expertenmodus an 31 (`add_todo`, `set_todo_done` bzw. die fünfzehn Ticket-Werkzeuge gibt es nur im jeweiligen Modus, `list_todos` immer).
- [ ] **Modus wechseln:** Während eine Agent-Sitzung läuft, den Expertenmodus umschalten → der nächste Aufruf eines Werkzeugs des anderen Modus liefert `wrong_mode` mit Erklärung; nach dem Neuladen der Werkzeugliste (`/mcp` in Claude Code) stimmt die Liste.
- [ ] **Echter Agent (Claude Code):** „Lege ein To-do an: …“ (Modus aus), „Lege in PAN ein Ticket vom Typ Devtask an und verknüpfe diese Sitzung damit“ (Modus an) → Karte/Ticket erscheint ohne Neustart; Toast „Agent (Sitzungsname): …“ erscheint unten rechts, bis zu fünf gestapelt (älteste werden verdrängt), jeder ca. 10 s, Klick schließt; im Ticket-Verlauf steht die Quelle „Agent (Sitzungsname)“.
- [ ] **Grenzen:** Ein Agent kann nichts löschen und keine Typen/Workflows/Projekte ändern; Pflichtfelder und Übergangsregeln werden wie im Fenster geprüft (`required_missing`, `transition_not_allowed`, `conditions_not_met`, `transition_needs_fields`); `link_terminal` nur für die eigene oder eine selbst angelegte Sitzung.
- [ ] **Einstellungen → Agenten:** Zeile „Aktive Werkzeug-Gruppe“ folgt dem Schalter „Expertenmodus“ (Deutsch/Englisch); Stufe „Aus“ → alle Werkzeuge `mcp_disabled`.
- [x] **Abfrage und Sprints lesen** (`search_tickets` mit `query`, `list_sprints`, `get_sprint`, `list_versions`; Expertenmodus, Stufe „Nur lesen“): `query` (Abfragesprache, nur lesend) ersetzt die einfachen Filter (mit `text`/`type`/`status`/`starred`/`flagged`/`parent`/`no_parent`/`order` zusammen → `bad_args`; `project`, `limit`, `offset` gelten weiter); eine ungültige Abfrage liefert `invalid_query` mit `details.problems` (`position`, `message`), ein zu langer Text `too_large`, ein Abbruch nach 2 s `timeout`. `list_sprints` (Name, Ziel, Termine, Status, Summen; Filter `state`), `get_sprint` (Kennung oder Name mit `project`: Tickets, Summen, Bericht) und `list_versions` (Versionen und Meilensteine mit Fortschritt) antworten bei ausgeschalteten Projekt-Schaltern mit `feature_disabled`; Texte stehen mit `note` als nicht vertrauenswürdig. Automatisch in `tools/smoke-mcp.ps1` und den Tests von `planning::agent`.

## 47 · Gesamtprüfung vor dem Release 2.0.0
- [ ] **Neuinstallation und Update** (Installer über 1.0.x, laufend und ruhend; `tools\update-test.ps1`): Daten, Einstellungen, Autostart, Kontextmenü bleiben; `paneldeck.db` entsteht; ein Update über eine laufende Instanz hinterlässt `paneldeck.exe.old`, das beim Start gelöscht wird.
- [ ] **Portables ZIP:** entpacken, starten; `paneldeck-data` mit `paneldeck.db` liegt neben der .exe; zweiter Start öffnet keine zweite Instanz.
- [ ] **Deinstallation** entfernt Hooks/MCP-Registrierung wie bisher; der Datenordner bleibt (mit `paneldeck.db`).
- [ ] **Rückfall auf 1.x:** Eine 1.0.x-Version lässt `paneldeck.db` unbenutzt, aber ungestört (README weist darauf hin).
- [ ] **Leistung:** 10.000 Tickets (z. B. per Agent oder Skript angelegt): Start der Todo-Liste < 2 s, Board/Liste/Suche flüssig, Terminal Hub bleibt davon unberührt; Speicherbedarf der .exe-Datei wächst nicht merklich.
- [ ] **Beide Sprachen:** kompletter Durchlauf der Abschnitte 34–46 auf Deutsch; Stichprobe: kein englischer/deutscher Resttext an der falschen Stelle.
- [ ] **Dokumentation:** README (Englisch/Deutsch) und Changelog stimmen mit dem Programm überein; Screenshots für alle `TODO screenshot`-Kommentare in den READMEs sind erzeugt.

## 48 · Kommandozeile, Datenordner und Profile (2.1.0)
Vorbereitung: Kopie der .exe in einem eigenen Ordner unter `E:\paneldeck-test` (eigener Datenordner daneben), Schein-`USERPROFILE`. Automatisch: Unit-Tests `cli`, `help`, `paths` (Parser, Hilfetext mit jedem Beispiel, Suchreihenfolge, Profile, Fehler).
- [ ] **Version:** cmd im Ordner der .exe: `paneldeck.exe --version` → Version erscheint in der Konsole, kein Fenster öffnet sich.
- [ ] **Hilfe in der Konsole:** PowerShell: `.\paneldeck.exe --help --lang de` → deutsche Hilfe mit korrekten Umlauten; ohne `--lang` in der gespeicherten Sprache; jede Option hat Überblick, Erklärung und Beispiel.
- [ ] **Hilfe ohne Konsole:** Win+R `<Pfad>\paneldeck.exe --help` → Meldungsfenster mit der Hilfe (Info-Symbol).
- [ ] **Unbekannte Option:** Win+R `<Pfad>\paneldeck.exe --bogus` → Meldungsfenster mit Fehler-Symbol, „Unknown option: --bogus“ und Hinweis auf `--help`; `--bogus --lang de` → deutsch. In der Konsole: Exit-Code 2 (`echo %ERRORLEVEL%` bzw. `$LASTEXITCODE`).
- [ ] **Profil nebeneinander:** Standardinstanz läuft; `--profile work` starten → zweites, leeres Fenster; *Einstellungen → Info* zeigt Datenordner `…\profiles\work`, Profil „work“ und die Herkunft; ein zweiter Start mit `--profile work` holt dieses Fenster nach vorn (keine dritte Instanz).
- [ ] **`--lang`:** Gespeichert ist Englisch; mit `--lang de` starten → Oberfläche deutsch, *Info* nennt die Startoption; beenden, ohne Option neu starten → Englisch; `settings.json` enthält weiter `"En"`; `prompts.json`, Standard-Projektname und Texte des Kontextmenüs bleiben englisch. Eine Sprache, die während eines `--lang`-Starts unter *Einstellungen → Allgemein* gewählt wird, wird gespeichert.
- [ ] **`--backup-dir D:\x`:** *Daten & Sicherung* zeigt den Ordner mit „(festgelegt mit --backup-dir)“, „Ordner wählen“/„Standard“ sind gesperrt, „Jetzt sichern“ schreibt dorthin; Neustart ohne Option → wieder der alte Ordner.
- [ ] **`--no-tray`** bei eingeschaltetem *Schließen → Tray* / *Minimieren → Tray* → kein Tray-Symbol, Minimieren = Taskleiste, X beendet; der Reiter *Fenster* zeigt den Hinweis.
- [ ] **`--reset-settings`** (Paneldeck beendet) → Hinweis mit dem Namen der Sicherung (`settings.json.bak-<Zeit>`), Standard-Einstellungen, Sitzungen und To-dos bleiben; bei laufender Instanz → Fehlermeldung, Exit-Code 1, das Fenster der laufenden Instanz springt nicht nach vorn.
- [ ] **Unbrauchbarer Datenordner:** `--data-dir` unterhalb einer vorhandenen *Datei* → Fehlermeldung nennt den Ordner, Exit-Code 1, kein Rückfall auf einen anderen Ordner. Ebenso mit `PANELDECK_DATA`.
- [ ] **`PANELDECK_DATA`:** Variable auf einen leeren Ordner setzen, starten → Datenordner dort, *Info* nennt „festgelegt mit PANELDECK_DATA“; `--data-dir` hat Vorrang vor der Variable.
- [ ] **Diagnose:** *Info → Diagnose kopieren* → Zeile mit Herkunft des Datenordners, Profil und Startoptionen.
- [ ] **Ordner auf die .exe ziehen:** einen Ordner, eine Datei und `C:\` auf die kopierte .exe ziehen → Sitzung öffnet sich im Ordner (bei der Datei in ihrem Ordner).
- [ ] **Übergabe an die laufende Instanz:** Paneldeck läuft; `--open <ordner> --lang de` → nur die Sitzung öffnet sich, die Sprache bleibt; `--tray` zeigt das Fenster.
- [ ] **Brücke aus einem Profil-Terminal:** `--profile work`, Agenten-Stufe *Lesen*; im Terminal dieser Instanz `paneldeck.exe mcp` (über Claude) → erreicht den Hub „work“, nicht die Standardinstanz; `paneldeck.exe mcp --profile work` aus einem fremden Terminal ebenso; läuft „work“ nicht, nennt `hub_not_running` den gesuchten Ordner.
- [ ] **Beide Sprachen:** Hilfe, Fehlermeldungen und Meldungsfenster auf Deutsch und Englisch.

## 49 · Integrationen je Instanz (2.1.0)
Vorbereitung wie Abschnitt 48; Registry-Prüfungen nur mit Testinstanzen, danach alles wieder ausschalten. Automatisch: Unit-Tests `launch`, `integration` (Befehle, Namen, Hash, Pläne), `tools\update-test.ps1` (Deinstallation).
- [ ] **MCP-Name je Profil:** Instanz `--profile work`, *Einstellungen → Agenten* zeigt „Servername dieser Instanz: paneldeck-work“ und die Befehlszeile mit `mcp --profile work`; *Registrieren* → `claude mcp list` zeigt `paneldeck-work`; JSON-Knopf → Schlüssel `paneldeck-work`; *Entfernen* → nur `paneldeck-work` verschwindet.
- [ ] **Standardinstanz unverändert:** Name `paneldeck`, Befehl genau wie in 2.0.0 (`"<exe>" mcp`).
- [ ] **Autostart je Instanz:** Instanz „work“: Autostart an → Run-Wert `Paneldeck (work)` = `"<exe>" --profile work --tray`; aus → nur dieser Wert verschwindet; der Schalter der Standardinstanz ist davon unabhängig.
- [ ] **Kontextmenü – der Letzte gewinnt:** in der Standardinstanz einschalten, dann in „work“ → der Befehl trägt `--profile work`; die Standardinstanz zeigt „→ anderes Paneldeck“ mit dem Befehl im Tooltip; dort wieder einschalten übernimmt es.
- [ ] **Kontextmenü auf einem Ordner und auf dem Ordner-Hintergrund** (auch im Hintergrund der Laufwerkswurzel `C:\`) → öffnet in der richtigen Instanz im richtigen Ordner. Auf dem Laufwerkssymbol selbst (z. B. unter *Dieser PC*) gibt es den Eintrag nicht – registriert sind nur `Directory\shell` und `Directory\Background\shell`.
- [ ] **Datenordner mit Leerzeichen und abschließendem Backslash:** `--data-dir "D:\Pd Test\"` → Namen mit 8-stelligem Hex-Hash (`Paneldeck (<hash>)`, `paneldeck-<hash>`); der Run-Befehl hat den doppelten Backslash vor dem schließenden Anführungszeichen; nach der Anmeldung startet die Instanz mit genau diesem Ordner.
- [ ] **`%` im Pfad:** .exe in einem Ordner mit `%` im Namen → roter Hinweis im Reiter *Agenten*, *Registrieren*/*Kopieren* gesperrt; Autostart/Kontextmenü melden einen Fehler statt etwas Falsches zu schreiben.
- [ ] **Deinstallation** (Testinstallation, `tools\update-test.ps1` bzw. von Hand mit `/D=<Temp>`) mit Autostart von „work“ und registriertem `paneldeck-work` → beides weg, fremde Einträge bleiben, der Rest von `.claude.json` ist unverändert, die Deinstallation endet zügig auch ohne `claude` im PATH.
- [ ] **Beide Sprachen:** Statuszeilen und Hinweise in *Fenster* und *Agenten* auf Deutsch und Englisch.

## 50 · Systemprompt für Agenten (2.1.0)
Vorbereitung: eigener Datenordner ohne `system-prompt.md`. Automatisch: Unit-Tests `sysprompt` (Platzhalter, Laden, Größe, BOM, Werkzeugbeschreibungen in beiden Sprachen), `tools\smoke-mcp.ps1` (`instructions` in `initialize`).
- [ ] **Ohne Datei:** *Einstellungen → Agenten → Systemprompt* zeigt die englische eingebaute Vorlage, „Eingebaute Vorlage“; *Speichern*, *Verwerfen*, *Zurücksetzen* sind gesperrt.
- [ ] **Sprache:** „Deutsch“ wählen → deutsche Vorlage, `settings.json` enthält `"mcp_prompt_lang": "De"`, Vorschau deutsch; zurück auf Englisch. Die Sprache der Oberfläche bleibt davon unberührt.
- [ ] **Stufe und Modus:** Stufe *Lesen*, Expertenmodus aus → Vorschau ohne `send_text`/`add_todo`; *Steuern* → beide erscheinen; Expertenmodus an → `create_ticket` erscheint, `add_todo` verschwindet; der Modus heißt „expert mode“/„Expertenmodus“ ohne Zusatz.
- [ ] **Bearbeiten:** `Test {{version}} {{foo}}` tippen → „ungespeicherte Änderungen“, gelbe Warnung zum unbekannten Platzhalter; Vorschau-Überschrift „– ungespeicherte Änderungen“; *Speichern* → `system-prompt.md` existiert, Anzeige „Eigene Vorlage“.
- [ ] **Verwerfen/Zurücksetzen:** *Änderungen verwerfen* → gespeicherter Text kommt zurück. *Auf Standard zurücksetzen* → `system-prompt.md.bak` entsteht, die Meldung nennt sie; nochmal zurücksetzen ersetzt die `.bak`.
- [ ] **Größe:** Text über 64 KB → rote Meldung „zu groß“, *Speichern* gesperrt.
- [ ] **Kopieren / Als Datei speichern:** *Kopieren* → gerenderter Text in der Zwischenablage; *Als Datei speichern …* → `.md`-Dialog, Meldung „Gespeichert“; Abbrechen → nichts geschrieben, keine Meldung.
- [ ] **Änderung von außen:** Datei im Editor ändern (ohne eigene Änderungen in Paneldeck) → der Editor lädt neu (BOM entfernt); mit ungespeicherten Änderungen → orange Warnung, *Verwerfen* lädt die Datei.
- [ ] **Ungültige Datei:** `system-prompt.md` als UTF-16 speichern → gelbe Meldung „system-prompt.md ist kein gültiges UTF-8 – es gilt die eingebaute Vorlage.“
- [ ] **Agent:** Claude Code (oder `tools\smoke-mcp.ps1`) verbindet sich → `instructions` in `initialize` ist der gerenderte Prompt (eigene Vorlage, falls gespeichert).
- [ ] **Beide Sprachen:** alle Texte des Abschnitts auf Deutsch und Englisch.

## 51 · Mehrere Fenster und leerer Start (2.1.0)
Vorbereitung: eigener Datenordner. Automatisch: `tools\smoke-mcp.ps1` (Start ohne Terminal, Feld `window`), Unit-Tests der Fensteransicht.
- [ ] **Leerer Start:** leerer Datenordner → kein Terminal, mittig ein Hinweis mit „Neues Terminal“ (im Claude-Modus auch „Neue Claude-Sitzung …“); die Kürzel-Hinweise folgen den eigenen Tastenbelegungen und der Sprache.
- [ ] **Neues Terminal** → öffnet sich; schließen → wieder der leere Hub.
- [ ] **Wiederherstellen:** eine Sitzung offen, beenden, neu starten → Sitzung wiederhergestellt, kein zusätzliches Terminal.
- [ ] **Tooltips der Reiter:** „Strg+Klick: in neuem Fenster öffnen“ (EN: „Ctrl+click: open in a new window“).
- [ ] **Neues Fenster:** Strg+Klick auf *Terminal Hub* → zweites Fenster (versetzt, gleiche Größe, eigene Fensterleiste, runde Ecken, Titel „Paneldeck (2)“ bzw. „<Sitzung> – Paneldeck (2)“), leerer Hub mit „Noch keine Sitzung in diesem Fenster“; das Hauptfenster bleibt unverändert.
- [ ] **Eigene Sitzungen:** In Fenster 2 neues Terminal + Teilen → nur in der Seitenleiste von Fenster 2; Tippen landet im richtigen Terminal. Reiterwechsel in Fenster 2 betrifft nur Fenster 2. Ziehen, Doppelklick, Größe ändern, Minimieren funktionieren (kein Snap-Menü an weiteren Fenstern).
- [ ] **Todo-Fenster:** Strg+Klick auf *Todo-Liste* in Fenster 2 → Fenster 3 mit der Todo-Liste; dort anlegen/ändern → erscheint im Hauptfenster und umgekehrt.
- [ ] **Sprung über Fenster:** Palette im Hauptfenster, eine Sitzung von Fenster 2 wählen → Fenster 2 kommt mit dieser Sitzung nach vorn; ebenso der Terminal-Chip eines Tickets.
- [ ] **Abdocken:** eine Sitzung aus Fenster 2 abdocken, das abgedockte Fenster schließen → sie kehrt nach Fenster 2 zurück.
- [ ] **Hinweise:** Sitzung in Fenster 2 wird fertig bzw. braucht Freigabe → Hinweis/Toast in Fenster 2, Taskleiste von Fenster 2 blinkt.
- [ ] **Schließen:** Fenster 2 mit laufenden Sitzungen schließen → Rückfrage „Fenster schließen?“ immer (auch bei ausgeschaltetem „Schließen bestätigen“) mit der Anzahl; *Abbrechen* (oder `Esc`) behält alles; *Fenster schließen* beendet die Sitzungen (`list_sessions` listet sie nicht mehr). Ein Fenster ohne Sitzungen bzw. nur mit beendeten Sitzungen schließt ohne Frage. Hauptfenster schließen → alle Fenster zu.
- [ ] **Schließen mit Abgedocktem und Worktree:** Fenster 2 mit einer abgedockten Sitzung schließen → die Rückfrage nennt sie unter „Abgedockte Sitzungen bleiben offen und gehören danach zum Hauptfenster“; nach dem Schließen läuft sie weiter und gehört zum Hauptfenster. Mit einer Worktree-Sitzung → *Fenster schließen* ist gesperrt (Tooltip „Erst die Sitzungen mit Worktree schließen.“), *Schließen …* daneben öffnet die gewohnte Worktree-Rückfrage (behalten/entfernen/mergen); danach lässt sich das Fenster schließen.
- [ ] **Tray:** Hauptfenster in den Tray → Fenster 2 bleibt offen und aktualisiert sich; Hauptfenster minimieren → Fenster 2 zeichnet weiter.
- [ ] **Neustart:** Beenden mit Fenster 2 (zwei Sitzungen, verschoben) → der nächste Start stellt Fenster 2 an Position und in Größe mit seinen Sitzungen wieder her; das wiederhergestellte Fenster nimmt dem Hauptfenster nicht den Fokus. Liegt die gespeicherte Lage auf keinem Monitor mehr, wird das Fenster auf einen sichtbaren Monitor gesetzt. Start mit `--tray` → weitere Fenster bleiben verborgen, bis das Hauptfenster aus dem Tray geholt wird.
- [ ] **Rückwärts:** optional `state.json` mit 2.0.0 öffnen → lädt, alle Sitzungen im Hauptfenster.
- [ ] **MCP:** `list_sessions` zeigt `window: 2`; `focus_session` holt Fenster 2 nach vorn; `screenshot` dieser Sitzung → `not_visible` mit Erklärung; `create_session` eines Agenten in Fenster 2 → Kind in Fenster 2.
- [ ] **Getrennte Ansicht:** Akzentfarbe je Fenster, Scroll-Positionen und Textfelder gehen nicht auf andere Fenster über; der Rückgängig-Hinweis wirkt in seinem Fenster.
- [ ] **Beide Sprachen:** Schließen-Dialog, Tooltip, leerer Hub auf Deutsch und Englisch.

## 52 · Selbst-Update (2.1.0)
Vorbereitung (lokaler Test-Feed, nichts wird veröffentlicht): Test-Build mit dem Cargo-Feature `update-test-feed`, z. B. `CARGO_TARGET_DIR=/e/paneldeck-test/target-upd cargo build --release --features update-test-feed`; „neue“ Version mit `tools/update-feed.sh <version> <feed-ordner> [<paneldeck.exe>]` anlegen (`latest.json`, ZIP, Installer, `SHA256SUMS.txt`; `NO_SUMS=1` ohne Prüfsummen, `BAD_SUM=1` mit falscher ZIP-Prüfsumme); die Testkopie unter `E:\paneldeck-test` mit `PANELDECK_UPDATE_TEST_FEED=<feed-ordner>` starten. Release-Builds enthalten diesen Weg nicht. Automatisch: Unit-Tests `app::update` (Versionen, Adressen, Prüfsummen, ZIP, Tausch, Fälligkeit).
- [ ] **Abschnitt „Updates“** unter *Einstellungen → Info*: Version, „(portabel)“ bzw. „(installiert)“, Knopf *Nach Updates suchen*, Schalter *Automatisch prüfen* aus, „Letzte Prüfung: noch nie“, Datenschutz-Hinweis.
- [ ] **Prüfen** (Feed 2.9.9) → Spinner, „Version 2.9.9 ist verfügbar“, Dialog öffnet sich, Knopf links vom Minimieren mit Tooltip.
- [ ] **Später** → Dialog und Knopf verschwinden; *Details …* unter *Einstellungen → Info* öffnet ihn wieder. *Später* während des Downloads → der Download läuft weiter, danach erscheint nur der Knopf „Neu starten“ (keine Abfrage).
- [ ] **Was ist neu?** → Browser öffnet die Release-Seite des Tags.
- [ ] **Überspringen** → „Übersprungene Version“ mit *Zurücksetzen*; die automatische Prüfung zeigt sie nicht mehr; eine Prüfung von Hand markiert sie mit „(übersprungen)“.
- [ ] **Automatisch:** Schalter an, neu starten → nach ca. 3 s erscheint der Knopf ohne Dialog; höchstens einmal am Tag; mit `--no-update-check` keine Prüfung.
- [ ] **Jetzt aktualisieren** mit einem laufenden Terminal → Fortschritt am Knopf und im Dialog, dann Neustart-Abfrage mit „1 Terminal läuft noch“; *Nicht jetzt* → Knopf „Neu starten“; *Neu starten und aktualisieren* → die neue .exe startet mit wiederhergestellter Sitzung (und gleichem `--profile`/`--data-dir`); `paneldeck.exe.old` ist gelöscht, nachdem die neue Version lief.
- [ ] **Ohne Terminal** → nach dem Download trotzdem die Bestätigung „Jetzt neu starten, um zu aktualisieren?“ vor dem Neustart; ein Klick auf *Neu starten und aktualisieren* im Update-Dialog gilt ohne laufendes Terminal selbst als Bestätigung.
- [ ] **Download-Ordner:** während des Downloads liegt die Datei unter `%LOCALAPPDATA%\Paneldeck\updates\<pid>-<zufall>\`; Ordner beendeter Instanzen räumt der nächste Start auf.
- [ ] **Abbrechen** des Downloads → neutrale Meldung „abgebrochen“.
- [ ] **Prüfsummen:** Feed mit `BAD_SUM=1` → „Prüfsumme stimmt nicht – der Download wurde verworfen.“, nichts installiert; Feed mit `NO_SUMS=1` → „lässt sich nicht automatisch installieren“, *Jetzt aktualisieren* gesperrt.
- [ ] **Hauptversion:** Feed 9.0.0 → orange Warnung „Neue Hauptversion“; vor dem Update entsteht eine Datenbank-Sicherung.
- [ ] **Installierte Kopie** (Testinstallation mit `/D=<Temp>`, Schein-Profil) → stilles Setup in denselben Ordner, Neustart klappt; eine gelöschte Desktop-Verknüpfung und ein ausgeschaltetes Kontextmenü bleiben aus.
- [ ] **Agenten** können kein Update auslösen (kein Werkzeug dafür).
- [ ] **Beide Sprachen:** alle Texte auf Deutsch und Englisch; `Esc` in einem Terminal schließt den Update-Dialog nicht.

## 53 · Gesamtprüfung vor dem Release 2.1.0
- [ ] **Abschnitte 48–52** komplett durchlaufen, einmal auf Deutsch und einmal auf Englisch.
- [ ] **Update von 2.0.0:** Installer über 2.0.0 (laufend und ruhend, `tools\update-test.ps1 -Exe <exe> -TempRoot E:\paneldeck-test` → „35 bestanden“): Daten, Einstellungen, Sitzungen, Autostart, Kontextmenü bleiben; `settings.json` und `state.json` von 2.0.0 laden ohne Verlust (neue Felder fehlen dort und werden ergänzt).
- [ ] **Portables ZIP:** entpacken, starten → leerer Terminal Hub, `paneldeck-data` neben der .exe; zweiter Start öffnet keine zweite Instanz; `--profile test` läuft daneben.
- [ ] **Alte Verknüpfungen/Skripte:** Startaufrufe aus 2.0.0 (`--open`, `--claude`, `--tray`, `mcp`) funktionieren unverändert; eine unbekannte Option ist jetzt ein Fehler (Exit-Code 2) – im Release-Text genannt.
- [ ] **Release-Dateien:** `tools/release.sh` erzeugt Installer, ZIP und `SHA256SUMS.txt`; `sha256sum -c SHA256SUMS.txt` im Ordner `dist/` stimmt; nach dem Veröffentlichen die Dateien aus dem Release laden und vergleichen.
- [ ] **Defender:** `powershell -NoProfile -ExecutionPolicy Bypass -File tools\defender-scan.ps1 dist\paneldeck-2.1.0.exe dist\Paneldeck-Setup-2.1.0.exe` → Exit-Code 0 (bei Treffer: Release anhalten, Fehlalarm melden, siehe `CLAUDE.md`).
- [ ] **Smoke-Test:** `tools\smoke-mcp.ps1 -Exe <exe> -WorkRoot E:\paneldeck-test` → alle Prüfungen bestanden („Ergebnis: 181 bestanden“, Stand 2.1.0).
- [ ] **Selbst-Update gegen das echte Release** (nach dem Veröffentlichen, mit einer 2.1.0-Kopie ist noch nichts Neueres da): *Nach Updates suchen* → „Paneldeck ist aktuell“; Datenschutz: keine Netzwerkzugriffe ohne Prüfung (Schalter aus, Start ohne Klick).
- [ ] **Dokumentation:** README (Englisch/Deutsch) und die Anleitungen unter `docs/en/` und `docs/de/` stimmen mit dem Programm überein, alle Links funktionieren; Changelog (beide Sprachen) und `docs/release-notes/2.1.0.en.md` sind vollständig.

## 54 · Chat-Sitzungen (2.2.0)
Vorbereitung: Kopie der .exe unter `E:\paneldeck-test`, eigener Datenordner, Schein-`USERPROFILE`; ein Server unter *Einstellungen → Modelle* (Ollama auf `http://localhost:11434` mit einem kleinen Modell, z. B. `qwen2.5:7b`). Automatisch: Unit-Tests `chat::*` (Verlaufsdatei, Logik, Sitzung), headless-egui-Tests `chat_ui` (Texte EN/DE).
- [ ] **Neuer Chat:** Sprechblasen-Knopf in der Seitenleiste → Dialog *Neuer Chat* → *Chat beginnen* → Chat-Bereich mit Hinweis „Neuer Chat“, Eingabe hat den Fokus; die Zeile in der Seitenleiste zeigt Chat-Symbol, Status und Modell.
- [ ] **Senden:** `hallo`, Umschalt+Enter, `welt`, Enter → rechtsbündige Blase mit zwei Zeilen; die Antwort streamt, währenddessen *Stopp* und Status „arbeitet“; danach ↻ unter der Antwort, *Kopieren* an Codeblöcken, „Tokens: N Eingabe · M Antwort“.
- [ ] **Kopieren:** *Kopieren* an einem Codeblock → in ein Terminal einfügen → der Code kommt genau einmal an.
- [ ] **Neu erzeugen:** ↻ → die Antwort wird ersetzt, nicht verdoppelt.
- [ ] **Bearbeiten:** ✎ neben der letzten eigenen Nachricht → Text im Eingabefeld mit „Letzte Nachricht wird bearbeitet – Senden ersetzt sie.“; ändern + Enter → alte Frage und Antwort ersetzt. `Esc` bzw. *Abbrechen* bricht ab, nichts geändert.
- [ ] **Stopp:** lange Antwort anfordern, *Stopp* während des Streamens → der Teiltext bleibt, Status wieder bereit.
- [ ] **Fehler:** Ollama beenden, senden → roter Kasten „Keine Verbindung zum Server … (Ollama: Port 11434)“ mit *Wiederholen*, Status rot; Ollama starten, *Wiederholen* → Antwort kommt.
- [ ] **Scrollen:** lange Unterhaltung, während des Streamens nach oben scrollen → Ansicht bleibt stehen; wieder nach unten → folgt dem Text.
- [ ] **Teilen:** Tippen im Chat, `Strg+Umschalt+D` → neuer Bereich ist ein Terminal; getippte Zeichen landen nicht im Terminal.
- [ ] **Text von außen:** Chat fokussiert: Doppelklick auf eine Datei im Dateibaum, Datei auf den Chat ziehen, Prompt-Snippet, Composer *Senden* → der Text landet im Eingabefeld des Chats, wird aber **nicht** gesendet.
- [ ] **Schließen:** leeren Chat schließen → keine Frage. Chat mit Verlauf (Schließen bestätigen an) → Frage „Der Chatverlauf wird gelöscht (eine laufende Antwort wird abgebrochen).“; *Abbrechen* behält ihn, Schließen löscht `chats/<id>.json`. Weiteres Fenster bzw. Ordner-*Alle schließen* mit Chats → Warnung nennt die Verläufe.
- [ ] **Neustart:** beenden und neu starten → der Chat ist mit Verlauf wieder da (kein Prozess, kein Terminal).
- [ ] **Fenster:** Chat abdocken; Strg+Klick auf *Terminal Hub* → weiteres Fenster mit einem Chat → senden geht, die Antwort streamt auch, wenn das Hauptfenster im Vordergrund ist.
- [ ] **Andere Einstiege:** Baumansicht, Rechtsklick auf Ordner → *Neuer Chat hier* (Chat landet im Ordner); Palette *Neuer Chat*; leerer Terminal Hub *Neuer Chat*.
- [ ] **Kontextmenü:** kein *Neu starten*, keine Warteschlange; *Verlauf als Datei speichern …* schreibt eine `.md`; *Duplizieren* → leerer Chat mit demselben Modell.
- [ ] **Datei:** `chats/<id>.json` kaputt machen (z. B. `{` am Ende abschneiden), neu starten → leerer Chat mit Hinweis, daneben `chats/<id>.defekt-<Zeit>.json`.
- [ ] **Beide Sprachen:** alle Chat-Texte auf Deutsch und Englisch.

## 55 · Modelle und Server (2.2.0)
Vorbereitung wie 54, aber zuerst ohne Server. Automatisch: Unit-Tests `llm::*` (Host-Regel, URL-Formen, Adapter, Datenstrom, DPAPI, Kürzung), `llm::config` (Einstellungen, tolerantes Lesen), `models_ui`.
- [ ] **Ohne Server:** *Neuer Chat* → der Dialog erklärt, dass kein Server eingerichtet ist; *Einstellungen öffnen* → Reiter *Modelle*.
- [ ] **Vorschlag:** Reiter *Modelle* ohne Server → „Vorschlag: Ollama auf diesem Rechner (http://localhost:11434)“; *Hinzufügen* (noch nichts abgerufen); *Verbinden* → Kreisel → Modelle mit „Werkzeuge ✓“ bzw. „keine Werkzeuge“ und „Kontext Nk“. Ollama beendet → „Keine Verbindung zum Server … (Ollama: Port 11434)“.
- [ ] **Erlaubte Adressen:** *+ Server hinzufügen* mit `http://8.8.8.8` → rot „Adresse nicht erlaubt“, *Speichern* gesperrt; ebenso `http://100.64.0.1`, `http://127.1`, `http://0x7f.0.0.1`, `http://user:pw@localhost`. `http://192.168.x.y:1234/v1` als OpenAI-kompatibel → *Speichern* geht.
- [ ] **Hostname:** ein Name, der auf eine öffentliche Adresse auflöst (z. B. `http://example.com`) → *Verbinden* meldet „Adresse nicht erlaubt“ mit der aufgelösten Adresse.
- [ ] **HTTPS selbst signiert** (optional, eigener Testserver im LAN): *Verbinden* → „Das Zertifikat des Servers ist nicht vertrauenswürdig …“ mit dem Hinweis auf *Vertrauenswürdige Stammzertifizierungsstellen* bzw. `http://`.
- [ ] **Umleitung** (optional, Testserver mit 301): Meldung „Der Server leitet weiter …“, der Umleitung wird nicht gefolgt.
- [ ] **API-Schlüssel:** *Zeigen*/*Verbergen*; *Speichern* → 🔑 am Server; `settings.json` enthält nur einen Base64-Blob, nirgends den Klartext. Bearbeiten mit leerem Feld behält den Schlüssel; *Schlüssel entfernen* löscht ihn. Datenordner unter einem anderen Windows-Benutzer öffnen (optional) → Chat meldet „Der gespeicherte API-Schlüssel lässt sich nicht entschlüsseln …“.
- [ ] **Neues Modell:** nach *Verbinden* steht ein neues Modell auf Stufe *Aus*; in `settings.json` fehlt `level_chosen`.
- [ ] **Modell-Einstellungen:** Stufe Aus/Lesen/Steuern (die Werkzeugliste wächst mit der Stufe), ein Werkzeug abwählen, *Vor Schreib-Aktionen nachfragen*, *Max. Schritte*, *Werkzeugaufrufe* Automatisch/An/Aus, eigene Kontextlänge → eine Wahl der Stufe setzt `level_chosen: true`; alles bleibt nach Schließen und erneutem Öffnen der Einstellungen und nach einem Neustart erhalten.
- [ ] **Dialog *Neuer Chat*:** aus Seitenleiste, Palette, leerem Hub und Ordnermenü → Server/Modell vorausgewählt (letzte Wahl), Enter beginnt den Chat; ein Ordner-Chat landet im Ordner.
- [ ] **Antwort:** echte Antwort streamt, ↻ und „Tokens: …“ erscheinen; *Stopp* bricht ab.
- [ ] **Ohne Werkzeuge:** Modell ohne Werkzeug-Fähigkeit (oder *Werkzeugaufrufe: Aus*) → Hinweis „Dieses Modell kann keine Werkzeuge nutzen – nur Chat.“
- [ ] **Kontext:** eigene Kontextlänge 512 + langer Text → Hinweis „Antwort an der Token-Grenze des Modells abgeschnitten.“ bzw. bei langem Chat die Markierung „Ältere Nachrichten sieht das Modell nicht mehr“.
- [ ] **Automatischer Kontext (Ollama):** ohne eigene Kontextlänge, Stufe *Steuern* → `ollama ps` zeigt 16384 als Kontext; eigene Kontextlänge 4096 bei Stufe *Steuern* → einmal je Chat der Hinweis „Der Kontext dieses Modells (4096 Tokens) ist zu klein für seine Werkzeug-Stufe …“.
- [ ] **Server entfernt:** Server eines Chats entfernen → Hinweis im Chat; Senden → Fehler mit Hinweis auf *Einstellungen → Modelle*.
- [ ] **Modell gelöscht:** `ollama rm <modell>` → Senden → „Nicht gefunden … (HTTP 404)“ mit Hinweis auf *Verbinden*.
- [ ] **Unlesbarer Eintrag:** in `settings.json` einen Server mit `"kind": "gemini"` ergänzen → erscheint nur lesbar mit „Unlesbarer Eintrag … bleibt unverändert erhalten“ und *Entfernen*; nach dem Speichern anderer Einstellungen steht er unverändert in der Datei.
- [ ] **Beide Sprachen:** Reiter, Dialoge und Fehlermeldungen auf Deutsch und Englisch.

## 56 · Werkzeuge und Freigaben in Chats (2.2.0)
Vorbereitung wie 54 mit einem Modell mit Werkzeug-Fähigkeit (z. B. `qwen2.5:7b`), daneben ein Terminal. Automatisch: Unit-Tests der Werkzeug-Schleife (Filter nach Stufe/Modus/abgewählt, Prüfung beim Ausführen, Schritte + *Weiter*, Stopp mit offenen Aufrufen, „Immer für diesen Chat“ in `chats/<id>.json`), End-to-End-Test mit einem Schein-Ollama-Server, headless-egui-Test der Freigabe-Knöpfe EN/DE.
- [ ] **Frage nach der Stufe:** neues Modell (Stufe nie gewählt), erste Nachricht senden → die Nachricht bleibt im Eingabefeld, der Chat fragt „Darf dieses Modell Paneldeck nutzen?“ mit *Aus* / *Lesen* / *Steuern*, Seitenleiste „braucht dich“; *Lesen* → die Nachricht wird gesendet, `settings.json` enthält `level_chosen: true`; der nächste Chat mit dem Modell fragt nicht mehr. Stufe stattdessen in *Einstellungen → Modelle* wählen → die Frage verschwindet, die Nachricht bleibt im Feld.
- [ ] **Lesend:** Stufe *Lesen*, „Welche Sitzungen sind offen?“ → eingeklappte Zeile `list_sessions` mit Kreisel, dann ✓; aufgeklappt Parameter und Ergebnis (JSON); danach die Antwort des Modells.
- [ ] **Stufe Aus:** Stufe *Aus* → das Modell bekommt keine Werkzeuge (nur Chat), keine Werkzeugzeilen.
- [ ] **Abgewählt:** `list_sessions` abwählen → das Modell kann es nicht aufrufen (bzw. bekommt einen Fehler zurück, ohne dass etwas ausgeführt wird).
- [ ] **Freigabe:** Stufe *Steuern*, *Nachfragen* an: „Schreib `echo hi` in das Terminal“ → Kasten „Das Modell möchte ein Werkzeug mit Schreibzugriff nutzen:“ mit `send_text → session N "Name"`, den vollständigen Argumenten (bei langem Text scrollbar, Hinweis „+N Zeichen – nach unten scrollen …“) und *Erlauben* / *Immer für diesen Chat* / *Ablehnen*; der Chat ist in der Seitenleiste als „braucht dich“ markiert (Aufmerksamkeitsring). Enter im Eingabefeld gibt **nicht** frei; Enter mit Fokus auf der Freigabe-Zeile erlaubt.
- [ ] **Erlauben** → Text erscheint im Terminal, Zeile ✓. **Ablehnen** → Zeile ⊘ abgelehnt, nichts im Terminal, das Modell erfährt die Ablehnung. **Immer für diesen Chat** → weitere `send_text` dieses Chats **auf dieselbe Sitzung** laufen ohne Frage; auf eine andere Sitzung fragt er wieder; ein anderer Chat fragt weiter; `chats/<id>.json` enthält `always_allow: [{"tool":"send_text","session":N}]`; nach einem Neustart gilt es für diesen Chat noch.
- [ ] **Kein „Immer“:** `create_session` und Aufrufe ohne Ziel-Sitzung (z. B. `add_todo`) zeigen nur *Erlauben* / *Ablehnen*. `wait_for_status` und `focus_session` laufen ohne Frage.
- [ ] **Widerrufen:** Kopfzeile „Dauerhafte Freigaben (1)“ → *Widerrufen* → der Eintrag verschwindet (auch aus der Datei), der nächste `send_text` fragt wieder.
- [ ] **Eigene Sitzung / Regeln:** der Chat darf sich nicht selbst schließen; ein Terminal mit offener Claude-Freigabe-Frage wird nicht beschrieben; `close_session` auf ein fremdes Terminal → abgelehnt; ein vom Chat angelegtes Terminal ist sein Kind und darf geschlossen werden.
- [ ] **Max. Schritte:** *Max. Schritte* = 2, Aufgabe mit vielen Aufrufen → „Nach 2 Werkzeugaufrufen angehalten“ mit *Weiter*; *Weiter* erlaubt zwei weitere. Abgewiesene Aufrufe (abgewähltes Werkzeug) zählen mit.
- [ ] **Unsinnige Aufrufe:** Modell ruft immer wieder ein nicht erlaubtes Werkzeug auf → „Das Modell rief immer wieder Werkzeuge auf, die nicht laufen konnten – angehalten.“ mit *Weiter*, keine Endlosschleife.
- [ ] **Kontext voll:** eigene Kontextlänge klein (z. B. 4096), Stufe *Lesen*, mehrmals große Ausgaben lesen lassen (`read_terminal` mit vielen Zeilen) → Ergebnisse sind gekürzt (das Ende bleibt, Hinweis auf nicht vertrauenswürdigen Inhalt), ältere werden durch „elided“ ersetzt; schließlich „Die Werkzeug-Ergebnisse passen nicht mehr in den Kontext des Modells …“ mit *Weiter* → wartende Aufrufe stehen als „nicht ausgeführt“ (`skipped`) im Verlauf, das Modell antwortet.
- [ ] **Screenshot:** Stufe *Lesen*, „Mach einen Screenshot“ → Ergebnis ohne Bilddaten („omitted – images are not passed to chat models“).
- [ ] **Stopp mit offenen Aufrufen:** während eine Freigabe wartet *Stopp* → der Aufruf wird abgebrochen; die nächste Nachricht funktioniert (kein „hängender“ Werkzeugaufruf im Verlauf).
- [ ] **Beenden während eines Werkzeugs:** Paneldeck beenden, während `run_command` läuft → nach dem Neustart steht der Aufruf als unterbrochen im Chat, weiterschreiben geht.
- [ ] **Aufrufprotokoll:** *Einstellungen → Agenten* bzw. `mcp-log.jsonl` listet die Aufrufe des Chats (`how: "chat"` und `model`; bei externen Agenten fehlen beide Felder).
- [ ] **Unabhängig von der MCP-Stufe:** *Einstellungen → Agenten* auf *Aus* → der Chat nutzt trotzdem die Werkzeuge seiner Modell-Stufe.
- [ ] **Beide Sprachen:** Freigabe-Knöpfe, Status und Hinweise auf Deutsch und Englisch.

## 57 · MCP-Zugriff auf Chats (2.2.0)
Vorbereitung: MCP-Stufe *Steuern*, ein Chat mit einem eingerichteten Modell. Automatisch: Unit-Tests der neuen Werkzeuge (Schemas, Stufen, Fehlercodes, Kürzung, nicht vertrauenswürdig, keine Geheimnisse), `tools\smoke-mcp.ps1 -Exe <exe> -WorkRoot E:\paneldeck-test` (mit Schein-Ollama-Server: Chat anlegen, senden, lesen, Fehlercodes; Zahl siehe Skript-Ausgabe).
- [ ] **list_sessions:** Chat mit `kind: "chat"` und `model`; Terminals mit `kind: "terminal"`.
- [ ] **list_models:** Server mit `id`, `name`, `kind`, `host:port` und Modellen (`level`, `tools`, `context_length`, `last_used`); kein API-Schlüssel, keine vollständige Adresse in der Antwort.
- [ ] **create_session** mit `kind: "chat"`, `server`, `model` → neuer Chat erscheint in der Seitenleiste als Kind des Agenten; unbekanntes Modell → `bad_args` mit den Namen der eingerichteten Server/Modelle (keine Schlüssel).
- [ ] **send_chat_message** „ping“ → `{ok: true, index}`; die Nachricht erscheint im Chat-Fenster mit „Von Agent … gesendet“, die Antwort streamt; `wait_for_status` bis `ready`; `read_chat` zeigt Frage und Antwort (als nicht vertrauenswürdig gekennzeichnet).
- [ ] **busy / permission_pending:** `send_chat_message` während die Antwort streamt bzw. während du die letzte Nachricht bearbeitest → `busy`; während eine Freigabe wartet bzw. die Frage nach der Stufe offen ist → `permission_pending`. Während der Freigabe: `read_chat` meldet `approval`, `wait_for_status` mit `approval` kehrt zurück (mit `needs_you`); freigeben kann der Agent nicht.
- [ ] **read_chat-Felder:** langer Chat mit `last: 2` → `more_above: true`; sehr lange Nachricht → `truncated`.
- [ ] **Kein Umweg:** Agent legt einen Chat (Stufe *Steuern*) an und lässt ihn `send_text` auf die Sitzung des Agenten aufrufen → `forbidden`.
- [ ] **Fehlercodes:** `read_terminal` auf den Chat → `not_a_terminal`; `read_chat` auf ein Terminal → `not_a_chat`; `send_chat_message` bei Stufe *Lesen* → `forbidden`; Server des Chats entfernt bzw. Modell nicht mehr in der Liste des Servers → `no_model`; `create_session` mit unbekanntem Modell ohne Server → `bad_args`.
- [ ] **Agent im Terminal:** Claude Code in einem Paneldeck-Terminal fragt „Frag den Chat X nach …“ → Nachricht kommt im Chat an, Claude liest die Antwort.
- [ ] **Beide Sprachen:** Fehlermeldungen der neuen Werkzeuge in beiden Sprachen (Codes unübersetzt).

## 58 · Maximieren ohne Rand (#134, 2.2.0)
- [ ] **Hauptfenster:** maximieren (Doppelklick auf die Fensterleiste bzw. Knopf) → kein durchsichtiger Rand, der Inhalt reicht an alle Kanten des Arbeitsbereichs, die Taskleiste bleibt sichtbar; wiederherstellen → gewohnte Größe mit runden Ecken.
- [ ] **Weiteres Fenster:** dasselbe in einem weiteren Fenster (Strg+Klick auf einen Reiter).
- [ ] **Skalierung 150 %:** dasselbe bei 150 % Anzeigeskalierung und auf einem zweiten Monitor (falls vorhanden).

## 59 · Mausrad (#141, 2.2.0)
Vorbereitung: Testinstanz wie 54 mit Git Bash oder WSL (`less`, `vim`, `htop`). Automatisch: Unit-Tests `termview` (Kodierungen SGR/UTF-8/normal, Grenze 94, DECSET 1007, Umschalt+Rad, Prompt nach Absturz).
- [ ] **Shell:** viel Ausgabe, Mausrad → der Verlauf von Paneldeck scrollt.
- [ ] **Programm mit Maus:** `vim` mit `:set mouse=a` bzw. `htop` → das Mausrad scrollt im Programm, Klicks setzen den Cursor/wählen eine Zeile; bei breitem Fenster auch rechts von Spalte 94 (SGR).
- [ ] **Vollbild ohne Maus:** `less` auf einer langen Datei → das Mausrad scrollt den Text (Pfeiltasten). *Einstellungen → Darstellung → Mausrad in Vollbildprogrammen* = *Aus* → nichts passiert; `settings.json` enthält `"alt_screen_wheel": "off"`; zurück auf *Pfeiltasten senden*.
- [ ] **DECSET 1007:** `printf '\e[?1049h\e[?1007l'` (Git Bash) → Mausrad sendet keine Pfeiltasten; `printf '\e[?1007h\e[?1049l'` stellt zurück.
- [ ] **Umschalt+Mausrad:** in der Shell bei eingeschaltetem Maus-Modus (z. B. `printf '\e[?1000h'`) → Umschalt+Rad scrollt den Verlauf; in `less` → nichts.
- [ ] **Nach Absturz:** Vollbildprogramm mit Maus hart beenden (z. B. `vim` per `kill -9` aus einer zweiten Sitzung) → am Prompt keine Pfeiltasten und kein Zeichensalat beim Mausrad (Shell-Integration nötig); nach dem nächsten Befehl normales Verhalten.
- [ ] **Beide Sprachen:** Einstellung und Hinweis auf Deutsch und Englisch.

## 60 · Bereiche minimieren (#142, 2.2.0)
Vorbereitung: Testinstanz wie 54 mit drei Terminals und einem Chat in einem geteilten Layout. Automatisch: Unit-Tests `layout` (Minimieren, Platz merken, Ketten, Normalisieren, tolerantes Lesen) und `state` (`minimized`/`slot`).
- [ ] **Knopf:** Kopf eines Bereichs zeigt Ticket verknüpfen (falls verfügbar), Rechts teilen, Darunter teilen, In eigenem Fenster öffnen, Minimieren, × → Minimieren: der Nachbar übernimmt den Platz, die Sitzung läuft weiter (z. B. `ping -t` zählt weiter); in der Seitenleiste abgeblendet mit Unterstrich-Markierung, Tooltip „Minimiert – läuft weiter …“.
- [ ] **Weitere Wege:** Kontextmenü des Bereichs, Kontextmenü der Sitzung in der Seitenleiste, Palette *Bereich minimieren*, Strg+Umschalt+W – alle minimieren.
- [ ] **Letzter Bereich:** alle Bereiche minimieren → leerer Hub „Alle Bereiche sind minimiert“; Klick in der Seitenleiste → einziger Bereich.
- [ ] **Alter Platz:** linken von zwei Bereichen (Anteil z. B. 30 %) minimieren und zurückholen → wieder links mit gleicher Breite; ebenso oben/unten. Nachbar danach auch minimieren bzw. schließen → das Zurückholen folgt der Kette an die richtige Stelle.
- [ ] **Nachbarn weg:** minimieren, dann alle früheren Nachbarn schließen → Zurückholen teilt den fokussierten Bereich, die Sitzung rechts.
- [ ] **Weggewechselt:** Sitzung, die nicht minimiert, aber nicht im Layout ist (abgeblendet ohne Markierung) → Klick ersetzt den fokussierten Bereich. Schon sichtbare Sitzung → nur Fokus.
- [ ] **Palette und MCP:** *Wechseln zu:* zeigt „minimiert“ und holt zurück; `list_sessions` meldet `minimized: true`, `focus_session` holt die Sitzung an ihren Platz.
- [ ] **Strg+Tab** überspringt minimierte Sitzungen.
- [ ] **Chat und Fenster:** einen Chat minimieren und zurückholen (Verlauf bleibt); in einem weiteren Fenster dasselbe; im abgedockten Fenster *Minimieren* → Fenster zu, Sitzung minimiert in der Seitenleiste, × dockt dagegen wieder an.
- [ ] **Neustart:** mit minimierten Sitzungen beenden und starten → weiterhin minimiert; `state.json` enthält `minimized`/`slot`; Zurückholen an den alten Platz.
- [ ] **Robust:** in `state.json` eine unbekannte Sitzungskennung ins Layout schreiben bzw. einen Anteil auf `null` setzen → das restliche Layout und alle Projekte bleiben erhalten.
- [ ] **Mindestgröße:** viele Teilungen bzw. Trennlinie ganz an den Rand ziehen → kein Bereich wird kleiner als etwa 120 × 70 px.
- [ ] **Fenster schließen:** weiteres Fenster mit minimierter Sitzung schließen → die Frage nennt sie unter „Auch diese Sitzungen werden beendet, obwohl sie gerade nicht im Layout stehen …“.
- [ ] **Beide Sprachen:** Knöpfe, Tooltips, Hub-Text und Palette auf Deutsch und Englisch.

## 61 · Gesamtprüfung vor dem Release 2.2.0
- [ ] **Abschnitte 54–60** komplett durchlaufen, einmal auf Deutsch und einmal auf Englisch.
- [ ] **Update von 2.1:** Installer über 2.1 (`tools\update-test.ps1 -Exe <exe> -TempRoot E:\paneldeck-test`): Daten, Einstellungen, Sitzungen bleiben; `settings.json` und `state.json` von 2.1 laden ohne Verlust (neue Felder fehlen dort).
- [ ] **Zurück auf 2.1** (optional, Kopie des Datenordners): 2.1 öffnet die Daten von 2.2 → Chats erscheinen als Terminals, `chats/*.json` bleibt unangetastet, kein Absturz.
- [ ] **Datenschutz:** ohne eingerichteten Server keine Netzwerkzugriffe durch Chats; mit Server nur Verbindungen zu dessen Adresse (z. B. mit `netstat -ano` prüfen).
- [ ] **Smoke-Test:** `tools\smoke-mcp.ps1 -Exe <exe> -WorkRoot E:\paneldeck-test` → alle Prüfungen bestanden (Zahl siehe Skript-Ausgabe „Ergebnis: N bestanden“).
- [ ] **Release-Dateien und Defender:** wie in Abschnitt 53 (`tools/release.sh`, `SHA256SUMS.txt`, `tools\defender-scan.ps1` auf `paneldeck-2.2.0.exe` und `Paneldeck-Setup-2.2.0.exe`).
- [ ] **Dokumentation:** README (Englisch/Deutsch), `docs/en/chat.md`/`docs/de/chat.md` und die geänderten Anleitungen stimmen mit dem Programm überein, alle Links funktionieren; Changelog (beide Sprachen) und `docs/release-notes/2.2.0.en.md` sind vollständig.

## 62 · Klickbares sieht klickbar aus (#187, 2.3.0)
Vorbereitung: Testinstanz wie 54 (Claude-Modus an, eine Claude-Sitzung mit Verbrauch, MCP-Stufe „Nur lesen“), dunkles und helles Thema. Automatisch: Unit-Tests `afford` (Hand über Knöpfen/Einträgen/Klickflächen, nicht über Textfeldern, Zieh-Flächen, deaktivierten Knöpfen, Fensterhintergründen; Knopf über dem Terminal; Auswahlknöpfe; Kostenknopf), `mcp::level_selector`, `sidebar::cost_summary`.
- [ ] **Hand überall:** Hauptreiter (Terminal Hub / Todo-Liste), Fensterknöpfe Minimieren/Schließen (Maximieren: Zeiger des Systems), Icon-Knöpfe der Werkzeugleiste, Sitzungs- und Ordnerzeilen, Projekt-Pillen, Einstellungsreiter, Knöpfe, Kontrollkästchen, Auswahllisten und ihre Einträge, aufklappbare Überschriften, Menü- und Paletteneinträge, Toasts → Hand-Zeiger.
- [ ] **Kein Hand-Zeiger:** Terminalfläche (Pfeil, auch beim Markieren; Strg über einem Verweis → Hand), Textfelder (I-Balken), Hintergrund eines Chats, Text eines Absatzes (Verweis darin → Hand), Kreisdiagramm außerhalb der Stücke, Kopfzeile eines Bereichs, deaktivierte Knöpfe; Trennlinien und Panelränder behalten die Größenpfeile.
- [ ] **Knöpfe über dem Terminal:** Suche (Strg+F) „<“/„>“ und „Neu starten“ in der Leiste einer beendeten Sitzung → Hand.
- [ ] **Agenten-Schnittstelle:** *Einstellungen → Agenten* → Aus / Nur lesen / Steuern sehen auch nicht gewählt wie Knöpfe aus (Rahmen, Fläche, gleich breit), gewählt in der Auswahlfarbe mit gut lesbarem Text (auch im hellen Thema); Klick wechselt die Stufe.
- [ ] **Weitere Auswahlknöpfe:** Sprache, Prompt-Sprache (Systemprompt), Profil-Reiter, Modell-Stufe und Werkzeugaufrufe (*Modelle*), Windows-/WSL-Pfad (Ordnerauswahl), Nicht vorgemerkt/Vorgemerkt (Git-Panel) → wie oben.
- [ ] **Kostenzeile:** unten in der Seitenleiste eigene Zeile „Σ … Token · ≈ $…“ als kleiner Knopf mit Pfeil „›“ → Überfahren: Fläche, Hand, Tooltip mit vollem Text; Klick öffnet die Kosten-Übersicht. Seitenleiste ganz schmal ziehen → Text wird kürzer („Σ … · ≈ $…“, dann „≈ $…“) bzw. verschwindet, ragt nie über den Rand. Ohne Verbrauch steht rechts neben dem Zahnrad die Version.
- [ ] **MCP-Chip:** als kleiner Knopf, Hand und Tooltip, Klick öffnet *Einstellungen → Agenten*.
- [ ] **Farbfelder:** Farbe von Projekt, Ordner, Sitzung, Profil und im Dialog *Sitzungseinstellungen* → Ring beim Überfahren, Hand; gewählte Farbe umrandet.
- [ ] **Hover-Zustände:** Toast (Rahmen), Tag-Farbe in *Tags verwalten* (Ring), Eltern-Ticket-Kürzel in der Ticketansicht (Hand, Tooltip „Eltern-Ticket öffnen“).
- [ ] **Weiteres Fenster:** dasselbe in einem weiteren Fenster (Strg+Klick auf einen Reiter).
- [ ] **Beide Sprachen:** Tooltips auf Deutsch und Englisch.

## 63 · Hintergrundprozesse und Job Objects (#152, #168, 2.3.0)
Jede Shell startet in einem eigenen Windows-Job-Object; Programme, die aus der Sitzung gestartet werden, bleiben darin (auch losgelöst). Sie können nicht ausbrechen (`CREATE_BREAKAWAY_FROM_JOB` schlägt fehl). Lang laufende Programme (`code .`, Browser, `wt`, `Start-Process notepad`) gehören damit zur Sitzung, die sie gestartet hat – „Alle beenden“ beendet sie hart, ohne Speichern.
- [ ] **Eine Sitzung, ruhend:** in PowerShell `Start-Process notepad` → X der Sitzung: Rückfrage ohne Wartezeit nennt `notepad.exe (PID …) · window open – <Titel>` und warnt „End all terminates them immediately – without saving“; Enter = „Keep running“ → Sitzung zu, Editor bleibt offen.
- [ ] **Alle beenden:** dasselbe mit „End all“ → Sitzung zu, Editor beendet (ungespeicherter Text weg). Esc/Abbrechen: nichts passiert, kein Enter/Esc im Terminal.
- [ ] **Laufende Sitzung:** `Start-Process notepad`, dann `ping -t 127.0.0.1` laufen lassen → X: Rückfrage mit 3 s Wartezeit; Enter erst danach (= weiterlaufen lassen, schließt die Sitzung).
- [ ] **Kein Fehlalarm:** Sitzung nur mit `claude` bzw. `ping -t` im Vordergrund → keine Hintergrund-Rückfrage (nur die übliche).
- [ ] **Viele:** zwei Sitzungen eines Ordners mit je einem Editor → Papierkorb am Ordner: eine Rückfrage, Prozesse je Sitzung, beide Knöpfe nach 3 s.
- [ ] **Beenden:** Hauptfenster schließen bzw. Tray → Beenden mit Editor im Hintergrund → Rückfrage (3 s); Abbrechen hält Paneldeck offen; mit „In den Tray schließen“ keine Rückfrage beim X.
- [ ] **Weiteres Fenster:** Sitzung mit Editor in ein zweites Fenster, Fenster schließen → Liste, „Schließen, weiterlaufen lassen“/„Schließen, alle beenden“ erst nach 3 s; Esc bricht ab, Enter erreicht kein Terminal.
- [ ] **Neu laden:** Kontextmenü → Sitzung neu laden mit Editor im Hintergrund → „Neu laden, weiterlaufen lassen“ / „Neu laden, alle beenden“, auch wenn die Shell schon beendet ist.
- [ ] **Programme mit Fenster:** `code .` bzw. ein Browser aus der Sitzung → in der Liste als „window open“ markiert; vor „Alle beenden“ selbst speichern.
- [ ] **MCP:** Agent schließt eine selbst angelegte Sitzung mit Hintergrundprozess → keine Rückfrage, Antwort mit `remaining_processes` (pid, name, window, title) und Hinweis „untrusted“; der Prozess läuft weiter.
- [ ] **Zuordnung:** ein per `Start-Process` aus der Sitzung gestarteter Prozess, dessen Shell schon beendet ist, wird bei `whoami` der Sitzung zugeordnet (Smoke-Test prüft das).
- [ ] **Beide Sprachen:** Rückfragen, Knöpfe und Warnung auf Deutsch und Englisch.

## 64 · Kontextlänge als Schieberegler (#305, 2.5.0)
*Einstellungen → Modelle → (Server) → (Modell) → Kontextlänge.* Regler in Zweierpotenzen 4096 … 1M, Zahlenfeld daneben für jeden Wert von 256 bis 16M.
- [ ] **Standard:** ohne Häkchen *eigene* ist der Regler sichtbar, aber gesperrt (Ziehen/Klicken ändert nichts); daneben „vom Server: 128K · 131.072 Tokens“ bzw. „Standard 8K · 8.192 Tokens“.
- [ ] **Eigene:** Häkchen setzen → Regler wird bedienbar und startet bei der erkannten Länge (bzw. 8192), auf die nächste Stufe gerundet; daneben Kurzform (z. B. `128K`) und Zahlenfeld „131.072 Tokens“.
- [ ] **Ziehen:** Regler von ganz links nach ganz rechts → nur die Stufen 4K, 8K, 16K, 32K, 64K, 128K, 256K, 512K, 1M (jeweils genau das Doppelte), Zahlenfeld folgt.
- [ ] **Pfeiltasten:** Regler anklicken bzw. per Tab fokussieren, → / ← verdoppeln bzw. halbieren den Wert um genau eine Stufe; an den Enden (4K, 1M) bleibt er stehen.
- [ ] **Freier Wert:** im Zahlenfeld 6000 bzw. 3.000.000 eintippen + Enter → Wert bleibt exakt (auch nach Neustart in `settings.json`), der Regler steht auf der nächsten Stufe (8K bzw. 1M), die Kurzform fehlt bei 6000; 100 → wird zu 256, 20.000.000 → 16.777.216.
- [ ] **Alte Werte:** `settings.json` mit `"context_length": 6000` bzw. `2000` bzw. `2097152` → nach dem Start unverändert angezeigt und gespeichert; erst wenn der Regler bewegt wird, springt der Wert auf eine Stufe.
- [ ] **Beide Sprachen:** Tooltip des Reglers, „Tokens“/„tokens“ und Tausendertrenner (Deutsch `1.048.576`, Englisch `1,048,576`).

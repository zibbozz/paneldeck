[← README](../../README.de.md) · [English](../en/processes.md) · **Deutsch**

# Prozessliste

Die **Prozessliste** führt lang laufende Programme wie Entwicklungsserver und Watcher aus – `php -S localhost:8000`, `ng serve`, `npm run dev`, `cargo watch` – getrennt von deinen interaktiven Terminal-Sitzungen. Getippt wird dort nicht: Du startest einen Prozess, siehst seine Ausgabe in einem eigenen Bereich und stoppst ihn wieder. *(Seit 2.6.0.)*

<!-- screenshot: Abschnitt Prozesse mit einem laufenden Eintrag (grüner Punkt, ↗, Fehlerzähler) und einem gestoppten, Terminal Hub → Seitenleiste → PROZESSE -->

## So legst du einen Entwicklungsserver an
1. Öffne den **Terminal Hub** (`Strg+1`).
2. Klicke in der Seitenleiste auf **+** neben **PROZESSE** (Tooltip *Neuer Prozess …*).
3. Gib einen **Namen** (z. B. `Web`) und einen **Befehl** ein (z. B. `npm run dev`).
4. Wähle den **Ordner** (Ordner-Knopf *Ordner wählen*; leer = Standardordner der Shell). Wähle bei Bedarf eine **Shell** und ein **Projekt**.
5. Klicke auf **Speichern**. Der Eintrag steht im Abschnitt *Prozesse* und ist gestoppt.
6. Klicke in seiner Zeile auf **▷** (*Starten*) oder doppelklicke die Zeile. Ein Klick auf die Zeile zeigt die Ausgabe.

<!-- screenshot: Dialog Neuer Prozess mit allen Feldern ausgefüllt (Name, Befehl, Ordner, Shell, Projekt, Gruppe, Reihenfolge, Automatisch starten, Protokolldatei, eine Umgebungsvariable), Terminal Hub → PROZESSE → + -->

## So startest du mehrere Programme gemeinsam
1. Rechtsklick auf den ersten Eintrag → *Bearbeiten …*.
2. Gib einen Namen für die **Gruppe** ein (z. B. `Dev`) und setze **Reihenfolge** auf `1`. Klicke auf **Speichern**.
3. Mach dasselbe beim zweiten Eintrag mit derselben Gruppe (Auswahl über **▾**) und **Reihenfolge** `2`.
4. Die Einträge stehen jetzt unter dem Namen der Gruppe. Der Start-Knopf der Gruppe (*Gruppe starten (in ihrer Reihenfolge)*) startet sie aufsteigend; ihr Stopp-Knopf (*Gruppe stoppen (in umgekehrter Reihenfolge)*) stoppt sie andersherum.

## So startet ein Prozess mit Paneldeck oder mit einem Projekt
1. Rechtsklick auf den Eintrag → *Bearbeiten …*.
2. Setze **Automatisch starten** auf *Mit Paneldeck* oder *Wenn das Projekt geöffnet wird*.
3. Klicke auf **Speichern**. Die Einstellung gilt ab dem nächsten Start von Paneldeck bzw. ab dem nächsten Öffnen des Projekts.

## So findest du heraus, warum ein Prozess gestoppt hat
1. Ein abgestürzter Eintrag wird rot, und du bekommst eine Benachrichtigung. Ein Klick auf seine Zeile öffnet die Ausgabe.
2. Der Kopf zeigt den Zustand, z. B. `abgestürzt (Code 1)`. `! n` in der Zeile zählt die Zeilen, die nach Fehlern aussehen.
3. Mit *In der Ausgabe suchen* (Strg+F) findest du den ersten Fehler.
4. Behebe die Ursache und klicke dann im roten Balken unten in der Ausgabe oder in ihrem Kopf auf *Neu starten* (oder Rechtsklick auf die Zeile → *Neu starten*).

<!-- screenshot: Ausgabe eines abgestürzten Prozesses mit Kopfzeile (Zustand, Neu starten, Suchen, Kopieren, Leeren, Bearbeiten, Schließen), Terminal Hub → PROZESSE → Klick auf einen roten Eintrag -->

## Der Abschnitt *Prozesse*
Der Abschnitt *Prozesse* liegt in der Seitenleiste des Terminal Hubs über den Vorlagen (nur im Hauptfenster) und lässt sich einklappen. Sein Titel zeigt, wie viele Einträge laufen (z. B. `PROZESSE 2/5`).

- **+** legt einen neuen Eintrag an.
- Jede Zeile zeigt einen Statuspunkt, den Namen und rechts **Starten** (▷) bzw. **Stoppen** (□). Ein laufender Eintrag mit erkannter lokaler Adresse bekommt **↗** (im Browser öffnen), und `! n` zählt die Zeilen seiner Ausgabe, die nach Fehlern aussehen.
- **Klick** auf eine Zeile zeigt ihre Ausgabe im Hauptbereich statt der Sitzungen; **Doppelklick** startet einen gestoppten Eintrag. Ein Klick auf eine Sitzung (oder **×** im Kopf der Ausgabe) holt die Sitzungen zurück.
- **Rechtsklick** auf eine Zeile: *Starten*, *Stoppen*, *Neu starten*, *Ausgabe zeigen*, *Bearbeiten …*, *Duplizieren* und – mit Protokolldatei – *Ordner der Protokolle öffnen*.
- Einträge eines anderen Projekts sind ausgeblendet, solange sie nicht laufen; Einträge ohne Projekt erscheinen in jedem Projekt.
- Einträge mit **Gruppe** stehen unter dem Namen der Gruppe, mit Knöpfen zum Starten bzw. Stoppen der ganzen Gruppe.

## Felder
| Feld | Bedeutung |
|---|---|
| Name | Erscheint in der Liste, in Benachrichtigungen und für Agenten. |
| Befehl | Eine Befehlszeile, ausgeführt über die gewählte Shell – `npm`, `php`, `ng` und andere Programme werden also wie im Terminal über `PATH` gefunden. |
| Ordner | Arbeitsordner (leer = Standardordner der Shell). Ein Ordner, den es nicht gibt, lässt den Start fehlschlagen – ebenso ein Netzwerkordner, der nicht rasch antwortet (dann erneut versuchen). |
| Shell | Standard ist PowerShell (PowerShell 7, falls installiert, sonst Windows PowerShell); oder jede erkannte Shell – cmd, Git Bash, WSL. PowerShell startet ohne dein Profil (`-NoProfile`). Bei cmd wird `%NAME%` im Befehl wie bei einer getippten Befehlszeile ersetzt (z. B. `cd %USERPROFILE%\app && npm start`); unbekannte Variablen bleiben stehen, `%X:~0,3%` und `%X:a=b%` werden nicht ausgewertet. |
| Projekt | Das Projekt, zu dem der Eintrag gehört (oder keins). |
| Gruppe, Reihenfolge | Einträge derselben Gruppe starten und stoppen gemeinsam: beim Starten aufsteigend nach *Reihenfolge* (z. B. Backend 1, Frontend 2), beim Stoppen umgekehrt. *Reihenfolge* ist nur die Reihenfolge des Startens: Der nächste Eintrag startet, sobald der vorige gestartet ist – Paneldeck wartet nicht, bis er bereit ist (z. B. an seinem Port lauscht). |
| Automatisch starten | *Nie (nur von Hand)* (Standard), *Mit Paneldeck* oder *Wenn das Projekt geöffnet wird* (auch beim Start, wenn es das geöffnete Projekt ist; einmal je Lauf). |
| Protokolldatei | Die Ausgabe zusätzlich nach `process-logs/<id>-<name>.log` im Datenordner schreiben (standardmäßig aus). Jede Zeile bekommt einen Zeitstempel; ab 5 MB wird rotiert (`.1` … `.3`). |
| Umgebungsvariablen | Name und Wert. Werte werden **verschlüsselt** gespeichert (Windows-DPAPI, nur für denselben Windows-Benutzer auf demselben Rechner lesbar), nie Agenten gezeigt und nie protokolliert. Beim Bearbeiten behält ein leerer Wert den gespeicherten. |

Änderungen an einem laufenden Eintrag gelten ab seinem nächsten Start. Löschen lässt sich ein Eintrag nur, wenn er nicht läuft.

## Ausführen, Stoppen, Zustände
Ein Prozess läuft in einer Konsole ohne Eingabe (Farben und Fortschrittsbalken funktionieren, Tippen ist abgeschaltet). Die Ausgabe behält die letzten **10.000 Zeilen**. Im Kopf des Bereichs stehen *Starten* / *Stoppen*, *Neu starten*, *In der Ausgabe suchen* (Strg+F), *Ganze Ausgabe kopieren*, *Ausgabe leeren*, *Bearbeiten …* (Zahnrad) und **×** (zurück zu den Sitzungen), solange der Prozess läuft außerdem bis zu zwei Knöpfe mit erkannten lokalen Adressen. Markierter Text wird mit Strg+C oder Rechtsklick kopiert.

**Stoppen** (Knopf oder Strg+C in der Ausgabe ohne Markierung) sendet Strg+C; ist der Prozess **nach 5 Sekunden** nicht beendet, beendet Paneldeck den **ganzen Prozessbaum** (Windows: das Job Object, in dem der Prozess läuft; Linux/macOS: seine Prozessgruppe). *Neu starten* stoppt einen laufenden Prozess so und startet ihn wieder.

Endet die Shell, laufen aber von ihr gestartete Programme weiter (z. B. ein im Hintergrund gestarteter Server), behält der Eintrag einen gelben Punkt und einen Knopf *Stoppen* (*Restprozesse beenden*), der sie beendet; beim Beenden von Paneldeck zählen sie mit.

| Zustand | Bedeutung |
|---|---|
| läuft | Der Prozess läuft. |
| wird gestoppt | Strg+C ist gesendet; Paneldeck wartet auf das Ende (höchstens 5 s, dann wird der Baum beendet). |
| gestoppt | Von dir gestoppt – oder noch nicht gestartet. |
| beendet | Von selbst mit Exit-Code 0 beendet. |
| abgestürzt (Code X) | Von selbst mit einem anderen Exit-Code beendet. Du bekommst eine Benachrichtigung (wie bei Claude), der Eintrag wird rot und die Ausgabe zeigt *Neu starten*. |
| Start fehlgeschlagen | Befehl oder Shell nicht gefunden oder der Ordner fehlt; die Meldung steht in der Ausgabe, ohne Exit-Code. |

**Nach einem Neustart von Paneldeck** ist die Liste noch da und alle Prozesse sind gestoppt – von selbst starten nur Einträge mit *Mit Paneldeck* (und *Wenn das Projekt geöffnet wird* für das geöffnete Projekt).

**Beim Beenden von Paneldeck** mit laufenden Prozessen kommt erst eine Rückfrage: *Alle stoppen* oder *Abbrechen* (ein *Weiterlaufen lassen* gibt es nicht – die Prozesse enden ohnehin mit Paneldeck). Das Verstecken im Tray fragt nicht und stoppt nichts.

## Im Browser öffnen
Paneldeck sucht in der Ausgabe nach Adressen (z. B. `http://localhost:4200/`). Angeboten werden nur **lokale** Adressen – `localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]` und die eigene Netzwerkadresse dieses Rechners – und sie öffnen sich **nur per Klick** (↗ in der Liste oder die Adressknöpfe im Kopf der Ausgabe). `0.0.0.0` wird als `localhost` geöffnet.

## Agenten (MCP)
Agenten lesen die Prozessliste ab der Stufe *Lesen* (`list_processes`, `read_process_output`; Ausgabe und Befehle sind als nicht vertrauenswürdig gekennzeichnet) und starten, stoppen und starten Einträge oder Gruppen neu ab der Stufe *Steuern* (`start_process`, `stop_process`, `restart_process`). Einträge anlegen, ändern oder löschen kannst nur du. Siehe [Agenten-Schnittstelle](agent-interface.md#prozessliste).

Ein Programm der Prozessliste, das selbst die Agenten-Schnittstelle von Paneldeck aufruft, zählt als keine Sitzung.

## Speicherung
Die Einträge stehen in `processes.json` im Datenordner – siehe [Datenordner](data-and-backups.md#prozessliste-processesjson).

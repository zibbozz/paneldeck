[← README](../../README.de.md) · [English](../en/windows.md) · **Deutsch**

# Mehrere Fenster und der leere Terminal Hub

## Einen Reiter in einem neuen Fenster öffnen
**Strg+Klick** auf einen Reiter der Fensterleiste (*Terminal Hub* oder *Todo-Liste*) wechselt nicht den Reiter, sondern öffnet ein **neues Fenster** mit diesem Reiter. Die Tooltips der Reiter weisen darauf hin („Strg+Klick: in neuem Fenster öffnen“).

- Ein neues Fenster öffnet sich leicht versetzt, gleich groß, mit eigener Fensterleiste und seiner Nummer im Titel („Paneldeck (2)“, „Paneldeck (3)“ …; mit fokussierter Sitzung „<Sitzung> – Paneldeck (2)“). Es lässt sich wie das Hauptfenster verschieben, maximieren, in der Größe ändern und minimieren (das Snap-Layouts-Menü von Windows 11 gibt es nur am Hauptfenster).
- Alle Fenster laufen im **selben Prozess**: Sie nutzen denselben Datenordner und dasselbe Profil und teilen sich Einstellungen, Datenbank, Tray, Agenten-Schnittstelle und Sicherungen. Ein weiteres Fenster ist keine neue Instanz (anders als ein [Profil](command-line.md#profile-und-mehrere-instanzen)).
- Ein **Terminal-Hub-Fenster hat eigene Sitzungen**: eigene Seitenleiste, eigenes Layout, aktives Projekt und eigener Reiter. Ein neues Terminal-Hub-Fenster startet **leer**; Sitzungen, die du dort anlegst oder teilst, erscheinen nur in diesem Fenster. Sitzungen zwischen Fenstern verschieben geht in 2.1.0 nicht.
- Ein **Todo-Listen-Fenster** zeigt dieselben Daten wie das Hauptfenster: Eine Karte oder ein Ticket, das in einem Fenster angelegt oder geändert wird, erscheint sofort in den anderen.
- Jedes Fenster behält seine eigene Ansicht: Reiter, Scroll-Positionen, Textfelder, offene Dialoge und Akzentfarbe gehen nicht auf andere Fenster über; ein *Rückgängig*-Hinweis wirkt in seinem eigenen Fenster.

## Über Fenster hinweg arbeiten
- Befehls-Palette und Verweise finden Sitzungen aller Fenster: Wählst du im Hauptfenster eine Sitzung von Fenster 2, kommt Fenster 2 mit dieser Sitzung nach vorn. Dasselbe gilt für den Terminal-Chip eines Tickets.
- Eine aus Fenster 2 abgedockte Sitzung kehrt nach Fenster 2 zurück, wenn ihr abgedocktes Fenster geschlossen wird.
- Hinweise einer Sitzung (fertig, Freigabe nötig) erscheinen in ihrem eigenen Fenster, und der Taskleistenknopf dieses Fensters blinkt.
- Mit *Schließen → Tray* kann das Hauptfenster im Tray verschwinden, während die anderen Fenster offen bleiben und sich weiter aktualisieren.

## Schließen und Wiederherstellen
- Beim Schließen eines weiteren Fensters **fragt Paneldeck immer nach** („Fenster schließen?“), wenn darin noch Sitzungen laufen, eine seiner Sitzungen abgedockt ist oder eine Sitzung einen Git-Worktree hat – auch bei ausgeschaltetem *Schließen bestätigen*, weil ein ganzes Fenster viele Sitzungen auf einmal beendet:
  - die Rückfrage nennt die Anzahl der noch laufenden Sitzungen; **Fenster schließen** beendet sie;
  - **abgedockte Sitzungen** bleiben in ihren eigenen Fenstern offen und gehören danach zum Hauptfenster;
  - **Sitzungen mit Git-Worktree** werden nie still geschlossen: Jede hat einen Knopf **Schließen …**, der die gewohnte Worktree-Rückfrage öffnet (Worktree behalten, entfernen oder mergen). *Fenster schließen* bleibt gesperrt, bis sie geschlossen sind.

  *Abbrechen* (oder `Esc`) lässt das Fenster offen. Ein Fenster ohne Sitzungen (oder nur mit beendeten Sitzungen) schließt ohne Nachfrage.
- **Das Hauptfenster zu schließen, schließt alle Fenster** (mit *Schließen → Tray* geht es stattdessen in den Tray).
- Beendest du Paneldeck mit offenen weiteren Fenstern, stellt der nächste Start sie an Position und in Größe wieder her, mit ihren Sitzungen. Liegt die gespeicherte Lage auf keinem Monitor mehr, wird das Fenster auf einen sichtbaren Monitor gesetzt. Wiederhergestellte Fenster nehmen dem Hauptfenster nicht den Fokus, und startet Paneldeck im Tray (`--tray`, *Mit Windows starten*), bleiben sie verborgen, bis das Hauptfenster angezeigt wird.
- Die Fenster stehen in `state.json` (Feld `extra_windows` und `window` je Sitzung). Ein älteres Paneldeck (2.0.0) liest diese Datei weiterhin und zeigt alle Sitzungen im Hauptfenster.

## Agenten
`list_sessions` meldet das Fenster `window` jeder Sitzung (0 = Hauptfenster), `focus_session` holt das Fenster der Sitzung nach vorn, und eine von einem Agenten angelegte Kinder-Sitzung öffnet sich im Fenster des Agenten. Ein `screenshot` einer Sitzung in einem anderen Fenster liefert `not_visible` mit einer Erklärung. Siehe [Agenten-Schnittstelle](agent-interface.md).

## Der leere Terminal Hub
Paneldeck **startet immer ohne offenes Terminal**, wenn nichts wiederherzustellen ist (erster Start oder alle Sitzungen geschlossen). Stattdessen zeigt der leere Terminal Hub mittig einen Hinweis mit **Neues Terminal** (im Claude-Modus auch **Neue Claude-Sitzung …**) und den Kürzeln für Palette, neue Sitzung und Teilen – in deiner Sprache und mit deinen eigenen Tastenbelegungen.

- Sitzungen des letzten Laufs werden wie bisher wiederhergestellt; es öffnet sich kein zusätzliches Terminal.
- Schließt du das letzte Terminal, kommt der leere Hub zurück.
- Auch ein neues Terminal-Hub-Fenster (Strg+Klick) startet leer.
- Hat das aktive Projekt eines Fensters keine Sitzungen, sagt der Hinweis das.

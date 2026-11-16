[← README](../../README.de.md) · [English](../en/chat.md) · **Deutsch**

# Chats mit lokalen Modellen

Ein **Chat** ist eine Sitzungsart neben Terminals: Du unterhältst dich mit einem Sprachmodell, das **auf diesem Rechner oder in deinem privaten Netz** läuft – mit [Ollama](https://ollama.com), LM Studio, dem llama.cpp-Server oder vLLM. Chats liegen wie Terminals in Projekten und Teilungen, und ein Modell mit Werkzeug-Fähigkeit kann Paneldecks [Agenten-Werkzeuge](agent-interface.md) nutzen – Sitzungen auflisten, Terminals lesen, Befehle ausführen – in den Grenzen, die du je Modell festlegst.

Nichts geht an einen Cloud-Dienst: Paneldeck verbindet sich nur mit Servern, die du einrichtest, und nur mit Adressen auf diesem Rechner oder in privaten Netzen (siehe [Erlaubte Adressen und Sicherheit](#erlaubte-adressen-und-sicherheit)).

## Server einrichten

### Ollama (für den Anfang empfohlen)
1. Ollama von https://ollama.com installieren (oder `winget install Ollama.Ollama`). Es läuft im Hintergrund und lauscht auf `http://localhost:11434`.
2. Ein Modell laden, zum Beispiel ein kleines Modell mit Werkzeug-Fähigkeit:
   ```
   ollama pull qwen2.5:7b
   ```
   (`llama3.1:8b` geht auch; größere Modelle antworten besser, brauchen aber mehr Speicher.)
3. In Paneldeck: *Einstellungen → Modelle*. Ist noch kein Server eingerichtet, schlägt Paneldeck **Ollama auf diesem Rechner** (`http://localhost:11434`) vor – **Hinzufügen** klicken, dann **Verbinden**.

Um Ollama auf einem anderen Rechner im Netz zu nutzen, es dort mit `OLLAMA_HOST=0.0.0.0` starten und seine Adresse eintragen, z. B. `http://192.168.1.20:11434`.

### LM Studio, llama.cpp-Server, vLLM (OpenAI-kompatibel)
Diese Server sprechen die OpenAI-API (`/v1/models`, `/v1/chat/completions`). Hinzufügen mit **+ Server hinzufügen** und dem Typ *OpenAI-kompatibel (LM Studio, llama.cpp, vLLM)*. Die Adresse darf mit oder ohne `/v1` angegeben werden:

| Server | Typische Adresse |
|---|---|
| LM Studio (Server im Reiter *Developer* starten) | `http://localhost:1234/v1` |
| llama.cpp `llama-server` (Werkzeugaufrufe brauchen `--jinja`) | `http://localhost:8080` |
| vLLM auf einem anderen Rechner | `http://192.168.1.30:8000/v1` |

OpenAI-kompatible Server melden nicht, ob ein Modell Werkzeuge kann und wie groß sein Kontext ist – beides bei Bedarf je Modell einstellen (siehe unten).

## Einstellungen → Modelle
Der Reiter listet deine Server mit ihren Modellen.

- **+ Server hinzufügen:** Typ (*Ollama* oder *OpenAI-kompatibel*), Name, Adresse, optional **API-Schlüssel** und **Wartezeit** (auf Daten, auch auf das erste Token; 0 = Standard 300 s). Eine Adresse außerhalb der [erlaubten Netze](#erlaubte-adressen-und-sicherheit) erscheint rot („Adresse nicht erlaubt“) und lässt sich nicht speichern.
- **Verbinden** ruft die Modellliste ab. Jedes Modell zeigt, ob es Werkzeuge nutzen kann (*Werkzeuge ✓*, *keine Werkzeuge* oder *Werkzeuge ?*, wenn der Server es nicht verrät), und seine Kontextlänge. Läuft der Server nicht, kommt „Keine Verbindung zum Server … (Ollama: Port 11434)“.
- **Bearbeiten** / **Entfernen …** eines Servers. Chats eines entfernten Servers antworten nicht mehr (sie zeigen einen Hinweis); den Server wieder hinzufügen, um sie fortzusetzen.

### API-Schlüssel
Manche Server (z. B. ein vLLM- oder LM-Studio-Server mit Schlüssel) verlangen einen API-Schlüssel; er wird als `Authorization: Bearer <Schlüssel>` gesendet.
- Der Schlüssel wird **verschlüsselt für deinen Windows-Benutzer** gespeichert (Windows-DPAPI) – in `settings.json` steht nur der verschlüsselte Blob, nie der Schlüssel selbst. *Zeigen*/*Verbergen* im Dialog, ein 🔑 neben dem Server, sobald ein Schlüssel gespeichert ist; beim Bearbeiten das Feld leer lassen, um den Schlüssel zu behalten; *Schlüssel entfernen* löscht ihn.
- **Datenordner umziehen:** Den verschlüsselten Schlüssel kann nur derselbe Windows-Benutzer auf demselben Rechner entschlüsseln. Kopierst du den Datenordner zu einem anderen Benutzer oder Rechner, bleibt der Server-Eintrag erhalten, der Chat meldet aber „Der gespeicherte API-Schlüssel lässt sich nicht entschlüsseln (anderer Benutzer oder Rechner?) – bitte neu eingeben.“ Den Schlüssel unter *Bearbeiten* neu eingeben.
- Über einfaches `http://` geht der Schlüssel (wie die ganze Unterhaltung) unverschlüsselt durch dein Netz – für Server auf anderen Rechnern HTTPS nutzen, wenn das wichtig ist.

### Einstellungen je Modell
Ein Klick auf ein Modell öffnet seine Einstellungen. Sie gelten für jeden Chat mit diesem Modell:

| Einstellung | Bedeutung |
|---|---|
| **Stufe** | **Aus** – nur Chat, keine Paneldeck-Werkzeuge · **Lesen** – Sitzungen, Terminalinhalt, Verlauf und Todo-Liste bzw. Tickets lesen · **Steuern** – zusätzlich Eingaben senden, Befehle ausführen, Sitzungen anlegen und schließen, Todo-Liste bzw. Tickets ändern. Ein neues Modell beginnt mit **Aus**; siehe [Die Frage nach der Stufe](#die-frage-nach-der-stufe). Diese Stufe ist **unabhängig von der MCP-Stufe** unter *Einstellungen → Agenten*, die nur für externe Agenten gilt. |
| **Werkzeuge** | die Werkzeuge, die die Stufe erlaubt; abgewählte werden dem Modell nie angeboten. |
| **Vor Schreib-Aktionen nachfragen** | standardmäßig an: Jedes schreibende Werkzeug wartet im Chat auf deine Freigabe. Schreibend sind alle Steuern-Werkzeuge außer `wait_for_status` und `focus_session` (sie warten nur bzw. zeigen nur eine Sitzung). |
| **Max. Schritte** | höchstens so viele Werkzeugaufrufe je Antwort (1–100, Standard 10); jeder Aufruf zählt, auch abgewiesene. Danach hält die Antwort mit einem Knopf *Weiter* an. |
| **Werkzeugaufrufe** | *Automatisch* (vom Server erkannt), *An* oder *Aus*. *Aus* für ein Modell, das scheitert, sobald Werkzeuge mitgeschickt werden. |
| **Kontextlänge** | *Standard* (Ollama: automatisch passend zur Stufe des Modells, siehe [Grenzen](#grenzen); OpenAI-kompatibel: vom Server, sonst 8192 Tokens) oder eine *eigene* Zahl an Tokens. Eine eigene Angabe bleibt immer erhalten; ist sie für die Werkzeug-Stufe des Modells zu klein, zeigt der Chat einmal einen Hinweis. |

Wie bei externen Agenten hängen die Werkzeuge auch vom Todo-Modus ab: Bei ausgeschaltetem Expertenmodus bekommt ein Modell die Werkzeuge der Todo-Liste, bei eingeschaltetem die Ticket-Werkzeuge.

### Die Frage nach der Stufe
Ein Modell, für das du nie eine Stufe gewählt hast, gilt überall als **Aus** – beim Senden, Neu erzeugen, Wiederholen, Weiter und bei Nachrichten externer Agenten. Sendest du in einem Chat mit einem solchen Modell die erste Nachricht, bleibt sie im Eingabefeld, und der Chat fragt einmalig: „Darf dieses Modell Paneldeck nutzen?“ mit **Aus**, **Lesen** und **Steuern**. Die Antwort wird für das Modell gespeichert (`level_chosen: true` in `settings.json`), und die wartende Nachricht wird gesendet. Eine Stufe unter *Einstellungen → Modelle* zu wählen, beantwortet die Frage ebenfalls. Solange die Frage offen ist, ist der Chat in der Seitenleiste als *braucht dich* markiert.

## Einen Chat beginnen
- Der Sprechblasen-Knopf in der Seitenleiste (*Neuer Chat (lokales Modell)*), *Neuer Chat* in der Befehls-Palette, *Neuer Chat* im leeren Terminal Hub oder *Neuer Chat hier* im Kontextmenü eines Ordners.
- Ein Dialog fragt nach **Server** und **Modell** (er merkt sich die letzte Wahl; *Verbinden* aktualisiert die Modellliste). **Enter** oder *Chat beginnen* öffnet den Chat. Ohne Server erklärt der Dialog, was zu tun ist, und bietet *Einstellungen öffnen* an.
- Ein Chat bleibt sein ganzes Leben an dieses eine Modell gebunden. *Duplizieren* im Kontextmenü des Chats beginnt einen leeren Chat mit demselben Modell.

## Einen Chat benutzen
- Nachricht tippen; **Enter** sendet, **Umschalt+Enter** macht eine neue Zeile. Die Antwort wird gestreamt und als Markdown angezeigt; Codeblöcke haben einen Knopf **Kopieren**.
- **Stopp** beendet eine laufende Antwort (der Text bis dahin bleibt). **↻** unter der letzten Antwort erzeugt sie neu (sie ersetzt die alte). **✎** neben deiner letzten Nachricht holt sie zurück ins Eingabefeld („Letzte Nachricht wird bearbeitet – Senden ersetzt sie.“); **Esc** oder *Abbrechen* bricht das Bearbeiten ab.
- Ein Fehler (Server nicht erreichbar, Modell nicht gefunden …) erscheint als roter Kasten mit **Wiederholen**.
- Unter einer Antwort: der **Token-Verbrauch** („Tokens: 120 Eingabe · 45 Antwort“, soweit der Server ihn meldet) und, wenn das Modell an seiner Token-Grenze aufgehört hat, „Antwort an der Token-Grenze des Modells abgeschnitten.“
- Ein Modell ohne Werkzeug-Fähigkeit zeigt den Hinweis „Dieses Modell kann keine Werkzeuge nutzen – nur Chat.“
- **Kontext:** Das Modell bekommt den [Systemprompt](agent-interface.md#systemprompt) (für die Stufe des Modells gerendert), den Projektnamen und den Ordner des Chats – aber keinen direkten Zugriff auf das Dateisystem. Passt der Verlauf nicht mehr in die Kontextlänge, fallen die ältesten Runden weg (der Systemprompt bleibt immer). Eine Markierung „Ältere Nachrichten sieht das Modell nicht mehr“ zeigt, ab wo das Modell die Unterhaltung noch sieht.
- Text aus Prompt-Snippets, dem Prompt-Composer, einem Doppelklick im Dateibaum oder einer abgelegten Datei landet im Eingabefeld des Chats – er wird **nie automatisch gesendet**.
- Das Kontextmenü eines Chats hat *Verlauf als Datei speichern …* (Markdown), *Duplizieren* und die üblichen Einträge; *Neu starten* und die Warteschlange gibt es nur bei Terminals.

### Werkzeugaufrufe und Freigaben
Ruft das Modell ein Paneldeck-Werkzeug auf, zeigt der Chat **eine eingeklappte Zeile je Aufruf** mit dem Werkzeugnamen und einem Status: ein Kreisel, solange es läuft, ✓ fertig, ✗ Fehler, ⊘ abgelehnt. Aufgeklappt zeigt sie Parameter und Ergebnis.

- Mit *Vor Schreib-Aktionen nachfragen* wartet ein schreibender Aufruf: „Das Modell möchte ein Werkzeug mit Schreibzugriff nutzen:“ mit dem Werkzeugnamen, der **Ziel-Sitzung** (Nummer und Name) und den **vollständigen Argumenten** (scrollbar; ein Hinweis sagt, wenn unten noch mehr steht), dazu **Erlauben**, **Immer für diesen Chat** und **Ablehnen**. Solange ist der Chat in der Seitenleiste als *braucht dich* markiert, wie eine Freigabe-Frage von Claude. **Enter** erlaubt nur, wenn die Freigabe-Zeile den Fokus hat – Enter im Eingabefeld gibt nie etwas frei. *Ablehnen* teilt dem Modell mit, dass der Aufruf abgelehnt wurde.
- **Immer für diesen Chat** gibt es nur, wenn der Aufruf ein Ziel `session` als Zahl hat, und nie für `create_session`. Es gilt nur für **dieses Werkzeug auf dieser Ziel-Sitzung** in diesem Chat – dasselbe Werkzeug auf einer anderen Sitzung fragt wieder. Die Regeln stehen in der Verlaufsdatei des Chats (`always_allow`, siehe [Datenordner](data-and-backups.md#chatverläufe)) und lassen sich oben im Chat unter **Dauerhafte Freigaben (N)** → *Widerrufen* zurücknehmen.
- Jeder Werkzeugaufruf zählt gegen die **Max. Schritte** des Modells, auch abgewiesene. Ist die Grenze erreicht, hält die Antwort an: „Nach N Werkzeugaufrufen angehalten.“ mit **Weiter**, das noch einmal so viele Aufrufe erlaubt. Ruft das Modell immer wieder Werkzeuge auf, die nicht laufen können (unbekannt, nicht erlaubt, kaputte Argumente), hält der Chat mit einem eigenen Hinweis an: „Das Modell rief immer wieder Werkzeuge auf, die nicht laufen konnten – angehalten.“
- **Kontext voll:** Werkzeug-Ergebnisse werden so gekürzt, dass die Runde in den Kontext des Modells passt. Reicht das nicht mehr, werden ältere Ergebnisse der Runde, die das Modell schon gesehen hat, durch einen kurzen Platzhalter ersetzt (ausgelassen); hilft auch das nicht, hält der Chat mit „Die Werkzeug-Ergebnisse passen nicht mehr in den Kontext des Modells – Kontextlänge in Einstellungen → Modelle erhöhen.“ an. *Weiter* überspringt dann die noch wartenden Aufrufe (das Modell erfährt, dass sie nicht ausgeführt wurden) und fragt das Modell erneut. Kein Aufruf läuft ohne Platz für sein Ergebnis, damit der Systemprompt nie abgeschnitten wird.
- **Ergebnisse:** Ein Werkzeug-Ergebnis ist höchstens 16 KB groß (weniger, wenn der Kontext knapp ist). Bei Terminalausgabe (`read_terminal`, `run_command`, `read_chat`) bleibt das **Ende** erhalten und der Anfang fällt weg, mit dem Hinweis, dass der Inhalt nicht vertrauenswürdig ist. Screenshots werden nicht an Chat-Modelle weitergegeben – das Modell erfährt nur, dass das Bild weggelassen wurde.
- **Stopp** bricht auch Werkzeugaufrufe ab, die noch warten oder laufen.
- Die Werkzeuge laufen in Paneldeck mit **dem Chat als Aufrufer**, nach denselben Regeln wie für externe Agenten: Ein Chat kann nicht in seine eigene Sitzung schreiben, nutzt nie Administrator-Profile oder eigene Programme, schließt nur Sitzungen, die er selbst angelegt hat, tut nichts in einer Sitzung mit offener Freigabe-Frage, und Terminalinhalt ist als nicht vertrauenswürdig gekennzeichnet. Sitzungen, die der Chat anlegt, sind seine Kinder-Sitzungen. Jeder Aufruf steht im Aufrufprotokoll (`mcp-log.jsonl`, gekennzeichnet mit `how: "chat"` und dem `model`).
- Nicht erlaubte Aufrufe (Stufe, abgewähltes Werkzeug) oder solche mit kaputten Argumenten werden nicht ausgeführt; das Modell bekommt stattdessen einen Fehler zurück. Beendet sich Paneldeck, während ein Werkzeug läuft, wird der Aufruf beim nächsten Start als unterbrochen markiert.

### Chats in Projekten, Teilungen und Fenstern
- Ein Chat ist eine Zeile in der Seitenleiste (Chat-Symbol, Status und Modell) und lässt sich in Projekte und Ordner verschieben, neben Terminals teilen, in ein eigenes Fenster abdocken und in weiteren Fenstern öffnen (Strg+Klick auf den Reiter *Terminal Hub*, siehe [Mehrere Fenster](windows.md)). Antworten streamen weiter, auch wenn das Fenster im Hintergrund ist.
- Chats werden nach einem Neustart mit ihrem Verlauf wiederhergestellt.
- **Schließen löscht den Verlauf eines Chats.** Ein Chat mit Nachrichten fragt vorher nach (wenn *Schließen bestätigen* an ist); beim Schließen eines Fensters oder Ordners mit Chats warnt Paneldeck vor den Verläufen, die gelöscht werden. Mit *Verlauf als Datei speichern …* behältst du eine Kopie.
- Der Verlauf steht in `chats/<id>.json` im Datenordner (siehe [Datenordner](data-and-backups.md#chatverläufe)).

Auch externe Agenten sehen Chats und können ihnen Nachrichten schicken – siehe [Agenten-Schnittstelle](agent-interface.md#chats).

## Erlaubte Adressen und Sicherheit
Paneldeck verbindet sich nur mit **diesem Rechner und privaten Netzen**:

| Erlaubt | |
|---|---|
| dieser Rechner | `localhost`, `127.0.0.0/8`, `::1` |
| private Netze | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `fc00::/7` |
| Link-Local | `169.254.0.0/16`, `fe80::/10` (IPv6-Link-Local-Adressen brauchen eine Zone, die Adressen hier nicht tragen können – besser IPv4 oder eine Adresse aus `fc00::/7`) |

Alles andere wird abgelehnt, zum Beispiel **öffentliche Adressen** (`8.8.8.8`, `2001:…`), der Carrier-Grade-NAT-Bereich **`100.64.0.0/10`**, `0.0.0.0`, Multicast und Broadcast sowie IPv4-Adressen in IPv6-Schreibweise (`::ffff:8.8.8.8`, `64:ff9b::…`).

- **Abgekürzte Zahlenformen** wie `127.1`, `0x7f.0.0.1`, `0177.0.0.1` oder `2130706433` werden abgelehnt statt geraten – Adressen als `192.168.1.10` schreiben. Benutzername/Passwort in der Adresse, Abfragen (`?`) und Fragmente (`#`) sind ebenfalls nicht erlaubt.
- **Hostnamen** außer `localhost` (z. B. `gpu-box.lan`) werden aufgelöst, und **jede** aufgelöste Adresse muss erlaubt sein – eine öffentliche Adresse darunter genügt, um den Namen abzulehnen. Die Prüfung läuft **vor jeder Anfrage** erneut, Paneldeck verbindet sich genau mit der geprüften Adresse (festgelegt, damit eine DNS-Änderung dazwischen die Verbindung nicht umlenken kann) und prüft nach dem Verbinden die tatsächliche Gegenstelle noch einmal.
- **Kein Proxy** (die Verbindung geht nie über einen System-Proxy), **keine Umleitungen** (ein Server, der umleitet, bekommt den Fehler „Der Server leitet weiter …“ – die Zieladresse eintragen), keine Cookies, keine automatische Windows-Anmeldung.
- **HTTPS:** TLS 1.2/1.3, das Zertifikat wird immer geprüft (Name, Gültigkeit, vertrauenswürdige Stammzertifizierungsstelle). Eine Sperrprüfung wird nicht erzwungen, weil interne Zertifizierungsstellen im LAN oft keine Sperrliste haben. Ein **selbst signiertes** Zertifikat wird mit einem Hinweis abgelehnt: das Zertifikat bzw. sein Stammzertifikat in Windows unter *Vertrauenswürdige Stammzertifizierungsstellen* (aktueller Benutzer) importieren oder im lokalen Netz `http://` verwenden.

## Datenschutz
- Anfragen gehen nur an die Server, die du eingerichtet hast – nichts verlässt deinen Rechner bzw. dein privates Netz, und Paneldeck sendet keine Telemetrie.
- Das Modell bekommt, was im Chat steht: deine Nachrichten, den Systemprompt, Projektname und Ordner sowie die Ergebnisse der Werkzeuge, die es aufruft (z. B. Terminalinhalt). Mit einem Server auf einem anderen Rechner gehen diese Daten durch dein Netz zu diesem Rechner.
- Verläufe bleiben im Datenordner (`chats/`), bis du den Chat schließt.

## Grenzen
- **Wartezeiten:** 10 s zum Verbinden, 30 s zum Senden einer Anfrage, standardmäßig **300 s** Warten auf Daten – auch auf das erste Token, das auf einer CPU Minuten dauern kann. Die Wartezeit lässt sich je Server einstellen (*Wartezeit*, 5–3600 s).
- **Größen:** Eine Antwort darf bis zu 16 MB Text und 128 Werkzeugaufrufe (je bis zu 1 MB Argumente) haben; größere enden mit „Die Antwort des Servers ist größer als erlaubt.“ Eine Verlaufsdatei über 8 MB verliert beim Speichern ihre ältesten Nachrichten.
- **Kontext:** Eine eigene Angabe gilt immer. Ohne sie bekommt **Ollama** einen automatisch bemessenen Kontext: Systemprompt + Werkzeug-Beschreibungen + 4096 Tokens Platz, geteilt durch 0,75 (die Antwort-Reserve), aufgerundet auf eine Zweierpotenz, dann mindestens 8192, höchstens 32768 und höchstens die eigene Länge des Modells – derzeit sind das bei jeder Stufe 16384. Gesendet wird er als `num_ctx`; mehr als nötig kostet nur Grafikspeicher. **OpenAI-kompatible** Server nutzen die Länge, die sie melden, sonst 8192. Ein Viertel des Kontexts bleibt für die Antwort frei (bei Ollama gesendet als `num_predict`). Ist das Modell zu langsam oder reicht der Grafikspeicher nicht, eine kleinere *eigene* Kontextlänge einstellen (und die Stufe *Lesen* nutzen); eine eigene Angabe, die für die Stufe zu klein ist, zeigt einmal je Chat einen Hinweis.
- **Werkzeug-Beschreibungen:** Chat-Modelle bekommen kompakte Werkzeug-Beschreibungen, um Kontext zu sparen; externe MCP-Clients bekommen die vollständigen.
- **Token-Schätzung:** Paneldeck schätzt Tokens vorsichtig – lieber zu viele als zu wenige, damit der Server nie still den Anfang der Unterhaltung abschneidet – und gleicht die Schätzung mit den Token-Zahlen ab, die der Server meldet.
- **Werkzeug-Ergebnisse:** höchstens 16 KB je Ergebnis; Screenshots werden nicht an das Modell weitergegeben.
- **Nicht angezeigt:** Die „Denk“-Ausgabe von Reasoning-Modellen wird nicht gesondert dargestellt.

## Fehlersuche
- **„Keine Verbindung zum Server … (Ollama: Port 11434)“:** Ollama (bzw. der andere Server) läuft nicht, oder Adresse/Port stimmen nicht. Ollama starten (Tray-Symbol) und erneut *Verbinden* klicken.
- **„Nicht gefunden – Adresse, Server-Typ und Modellname prüfen (HTTP 404)“:** Das Modell wurde auf dem Server gelöscht oder umbenannt, oder der Server-Typ stimmt nicht. *Einstellungen → Modelle → Verbinden* zeigt die aktuellen Modelle; bei Ollama lädt `ollama pull <modell>` es neu.
- **„Adresse nicht erlaubt“:** Die Adresse liegt weder auf diesem Rechner noch in einem privaten Netz (siehe oben).
- **„Unerwartete Antwort des Servers – stimmt der Server-Typ?“:** Ein OpenAI-kompatibler Server wurde als Ollama eingetragen oder umgekehrt.
- **„Zugriff verweigert – API-Schlüssel prüfen (HTTP 401/403)“:** Der Schlüssel fehlt oder ist falsch. **„Der gespeicherte API-Schlüssel lässt sich nicht entschlüsseln …“:** Der Datenordner stammt von einem anderen Windows-Benutzer oder Rechner – den Schlüssel neu eingeben.
- **Das Modell vergisst den Anfang der Unterhaltung / „Der Systemprompt ist größer als der Kontext des Modells“:** Die Kontextlänge ist zu klein. Eine zu kleine eigene *Kontextlänge* entfernen (Ollama bemisst sie dann automatisch) oder erhöhen, oder die eigene `system-prompt.md` kürzen.
- **„Die Werkzeug-Ergebnisse passen nicht mehr in den Kontext des Modells …“ / „Systemprompt und Werkzeug-Beschreibungen sind größer als der Kontext des Modells …“:** eine eigene *Kontextlänge* erhöhen oder entfernen, Werkzeuge abwählen, die das Modell nicht braucht, oder kleinere Ergebnisse verlangen. **„Der Kontext dieses Modells (N Tokens) ist zu klein für seine Werkzeug-Stufe …“:** Die eigene Kontextlänge ist kleiner, als die Stufe braucht – in *Einstellungen → Modelle* erhöhen oder die Stufe *Lesen* nutzen.
- **Das Modell nutzt keine Werkzeuge:** auf die Hinweise „Dieses Modell kann keine Werkzeuge nutzen – nur Chat.“ und „Werkzeuge sind für dieses Modell aus (Einstellungen → Modelle).“, die *Stufe* des Modells (Aus = nur Chat; ein neues Modell beginnt mit Aus) und abgewählte Werkzeuge achten. Kleine Modelle rufen Werkzeuge oft schlecht auf – ein größeres Modell mit Werkzeug-Fähigkeit versuchen. Meldet der Server einen Fehler, sobald Werkzeuge mitgeschickt werden (z. B. „does not support tools“), *Werkzeugaufrufe* auf *Aus* stellen.
- **Fehler zur Chat-Vorlage oder zu Rollen** (z. B. „roles must alternate“) bei llama.cpp oder LM Studio: Manche Modelle haben strenge Chat-Vorlagen. Paneldeck fasst zwei aufeinanderfolgende Nutzer-Nachrichten (nach einer fehlgeschlagenen Antwort) schon zu einer zusammen; lehnt der Server trotzdem ab, llama.cpp mit `--jinja` starten, ein anderes Modell oder eine neuere Fassung davon wählen.
- **Zeitüberschreitung bei Antworten:** Große Modelle auf einer CPU brauchen lange für das erste Token – die *Wartezeit* des Servers erhöhen.
- **„Das Zertifikat des Servers ist nicht vertrauenswürdig …“:** siehe HTTPS oben.

# Architektur: lokale Replik als Arbeitsgrundlage

Stand: 17. November 2026. Die folgenden Abschnitte beschreiben den aktuellen
Implementierungsstand. Frühere Ausbaustufen stehen am Ende als Entwicklungshistorie.

## Aktueller Überblick

- Die originale Bulwark-Oberfläche bleibt im unveränderten, fixierten Submodul.
  Desktopadapter und geprüfte Build-Transforms liegen außerhalb davon.
- UI-Lesezugriffe laufen über native IPC und redb. Zugangsdaten bleiben in Rust
  und im Systemschlüsselbund; nur die native Synchronisierung spricht mit Stalwart.
- Mail unterstützt lokale Lesezugriffe, Gelesen-/Sternänderungen, Verschieben,
  Papierkorb und lokale Entwürfe. Schreibabsichten werden dauerhaft gespeichert.
- Strukturierter Versand wird vor dem Netzwerkaufruf in redb journalisiert.
  Online kann der native Versandpfad unmittelbar die echte Serverantwort liefern;
  offline oder zur Prüfung gehaltene Aufträge liefern ein ausdrückliches
  queued-Ergebnis und warten auf Freigabe. Unklare Sendungen werden nicht
  automatisch wiederholt. Entwürfe werden noch nicht mit dem Server synchronisiert.
- Kalenderereignisse und Dateimetadaten nutzen lokale Überlagerungen und das
  Schreibjournal. Neue Datei-Uploads und Composer-Anhänge werden ebenfalls
  lokal gehalten und als Abhängigkeiten freigegebener Aufträge hochgeladen.
  Datei-Inhaltsersetzungen, Kontaktänderungen/-fotos und Kalenderverwaltung fehlen noch.
- Kontakte und Adressbücher werden lesend über eine kontogetrennte native redb-Replik
  bereitgestellt. Suche, Details und freigegebene Adressbücher funktionieren offline.
- Blob-Downloads werden kontogetrennt in redb gespeichert und funktionieren danach
  offline. Das Downloadlimit ist pro Konto einstellbar; FIFO-Bereinigung entfernt alte
  Downloads. Die Bereinigung lokaler Uploads bleibt offen.
- Email/get projiziert Header, MIME-Teile und Textwerte lokal. Einmaliger Online-
  Vollabgleich erneuert alte Repliken; abgeschnittene Textteile werden bis 8 MB
  nachgeladen und groessere Teile bleiben als abgeschnitten markiert.
- Offline-Aufträge benötigen beim Wiederverbinden eine Auswahl zur Übertragung.
  Reine Entwürfe zählen nicht als ausstehende Aktionen. Blockierte und unklare
  Aufträge werden durch eine pauschale Freigabe nicht erneut versendet.
- JMAP_READ_ONLY ist standardmäßig aktiv. Die begrenzten Kalender-Live-Tests
  sind abgeschlossen; ihre dauerhaften Quittungen sperren eine Wiederholung.
  Der zusätzliche TEST-Termin wurde nach fünf erfolgreichen Offline-/Reconnect-
  Phasen gelöscht. Externe Einladungs-/Absagezustellung bleibt unbestätigt.
- HERAKLES_FORCE_READ_ONLY erzwingt bei gesetzter Prozessvariable (unabhängig vom
  Wert, auch bei leerem Wert oder false) lesenden Betrieb für geladene Konten.
  Die Sperre verändert keine gespeicherten Zugangsdaten oder Nur-Lese-Einstellungen.
  Zum Aufheben die Variable vor dem nächsten Appstart vollständig entfernen.

## Aktuelle Verifikation

Am 17. November 2026 am aktuellen Arbeitsstand ausgeführt:

- npm run build: erfolgreich, einschließlich TypeScript-Prüfung; Vite meldet
  große Bundles und teilweise zugleich statische und dynamische Imports.
- npm run check:native und cargo build --manifest-path src-tauri/Cargo.toml:
  erfolgreich; der abschließende Native-Check umfasst auch die Prozesssperre.
- npm run test:native: 89 bestanden, 13 Live-Tests regulär ignoriert.
- cargo fmt --manifest-path src-tauri/Cargo.toml -- --check: erfolgreich.
- npm exec playwright -- test gegen Vite-Preview auf Port 1430:
  acht Tests bestanden, mit synthetischen Daten und nachgebildeter IPC-Grenze.
- npm run test:jmap-audit: sieben Tests bestanden; npm run check:jmap erfolgreich.
- node --test scripts/stage-release.test.mjs: drei Tests bestanden.

Diese erneute Prüfung hat keine schreibenden Live-Tests ausgeführt. Die zuvor
separat genehmigten Kalenderprüfungen und ihre Grenzen sind in
[calendar-live-validation.md](calendar-live-validation.md) dokumentiert.
Nach Freigabe von Computer Use wurde am 17. November der aktuelle native
Entwicklungsbuild von b69f3f1 mit HERAKLES_FORCE_READ_ONLY=1 im echten
Windows-Fenster geprüft: Kalender öffnen, Offline-Modus aktivieren, vorhandenen
lokalen Termin anzeigen, App vollständig schließen und neu starten. Offline-Modus
und Kalenderdaten blieben erhalten; die Oberfläche zeigte Nur Lesen und keine
ausstehenden Aktionen. Danach wurde der Reconnect erfolgreich abgeschlossen (Online / Verbunden,
Abgleich abgeschlossen); die Kalenderdaten blieben sichtbar.
Es wurden keine Termine verändert oder neuen Testobjekte angelegt. Eine gezielte
Freigabe eines gehaltenen Schreibauftrags im Fenster ist damit weiterhin nicht
nachgewiesen; der frühere Live-Test deckt diesen Ablauf nur am Produktionsworker ab.
Linux/macOS sind ebenfalls nicht vollständig nativ validiert. Das Upstream-Submodul
steht unverändert auf 03e7cd86f102bc66a589b7b02d49523b00ae633a.

## Nächste Arbeit

1. Gezielte Reconnect-Freigabe eines gehaltenen Kalender-Schreibauftrags im echten
   Desktopfenster prüfen; dafür eine neue begrenzte Live-Freigabe einholen. Der
   lesende Offline-/App-Neustart ist nachgewiesen. Keine verbrauchten Quittungen
   zurücksetzen oder bestehende Termine ändern.
2. Versand mit Anhängen und FileNode-Erstellung separat freigeben und live prüfen;
   Datei-Inhaltsersetzungen und Verwaltung verwaister Uploads ergänzen.
3. Den Konfliktvergleich um freie Bearbeitung einzelner Felder und eine
   durchsuchbare Übersicht abgeschlossener Aufträge erweitern.
4. Kontaktänderungen über ein dauerhaftes Journal anbinden; weitere lokale
   JMAP-Methoden und Kontaktfotos portieren.
5. Große Postfächer mit paginierter Staging-Ablage und lokalen Seitenabfragen
   unterstützen; Verwaltung lokaler Uploads ergänzen. Downloadlimits und
   kontogetrennte FIFO-Bereinigung sind bereits implementiert.
6. Lokale Daten verschlüsseln und Linux/macOS vollständig prüfen. Die vorhandene
   Release-Staging-/CI-Grundlage um verifizierte Installer, Signierung und Updates
   vervollständigen.

## Entwicklungshistorie

Die nachfolgenden Ausbaustufen dokumentieren die Entwicklung in zeitlicher
Reihenfolge. Aussagen wie „fehlt noch“, alte Testzahlen und frühere
Transportgrenzen beziehen sich auf die jeweilige Stufe. Für den heutigen
Funktionsumfang gelten der aktuelle Überblick und die neuesten Abschnitte zu
Schreibjournal, Uploads und lokaler Konfliktauflösung.



Herakles Mail passt die Oberfläche und das Add-on-Modell von [Bulwark Webmail](https://github.com/bulwarkmail/webmail) für einen Offline-Desktopclient an. [Stalwart](https://www.stalwart.email/) ist das primäre JMAP-Serverziel und läuft unabhängig vom Client. Projektdanksagungen und Lizenzhinweise stehen im [README](../README.md#acknowledgements) und in der [NOTICE](../NOTICE).



## Verbindlicher Datenfluss



```mermaid

flowchart LR

    UI[Bulwark React-Oberfläche] <-->|lokale Abfragen und Änderungsabsichten| IPC[Tauri IPC / lokales Repository]

    IPC <--> DB[(redb)]

    DB <--> SYNC[Rust-Synchronisierung]

    SYNC <-->|JMAP über HTTPS| SERVER[Stalwart]

```



Die Oberfläche arbeitet immer mit redb, auch bei bestehender Internetverbindung.

Änderungen werden zuerst dauerhaft lokal gespeichert. Nur die Rust-Synchronisierung

kommuniziert mit Stalwart. Netzwerkfehler dürfen weder lokale Lesezugriffe verhindern

noch ausstehende Änderungen löschen. redb stellt Persistenz bereit; die

Synchronisierung ist eigener Anwendungscode.



## Upstream und Integrationsgrenze



Bulwark 1.20.0 liegt als unverändertes Git-Submodul unter `upstream/bulwark`, fixiert

auf `03e7cd86f102bc66a589b7b02d49523b00ae633a`. Quelle und Lizenz stehen zusätzlich

in `upstream/bulwark.lock.json`. LICENSE und NOTICE bleiben im Submodul erhalten.

Nach einem frischen Checkout: `git submodule update --init`.



Bulwarks bestehender Client (`lib/jmap/client.ts`) verwendet direkte Serverzugriffe

und registrierbare Hooks aus `lib/jmap/offline-hooks.ts`. Der Browser-Offline-Modus

in `lib/offline/` nutzt IndexedDB und ist ein Fallback. Das entspricht noch nicht

dem verbindlichen Desktop-Datenfluss.



Für die Übernahme werden ein lokaler JMAP-kompatibler Adapter und der gespeicherte

Sitzungszustand benötigt. `JMAPClient.fromOfflineSession` ist ein vorhandener

Einstiegspunkt für einen Client ohne Netzwerkzugriff. Der Desktop-Adapter muss

lokale Reads und Writes beantworten, Änderungen an die Stores melden und weitere

Netzwerkpfade (Sitzung, Blob-Download, Upload, Events) ebenfalls auf Rust umstellen.

Ein bloßer Austausch von IndexedDB durch redb reicht nicht. Die bestehende

IndexedDB-Synchronisierung darf nicht parallel mit dem Rust-Worker schreiben.

Die ursprünglichen Bulwark-Oberflächen werden jetzt über src/desktop/BulwarkApp.tsx eingebunden. Routing, Plugin-Binärspeicher und Offline-Hooks sind durch Desktop-Adapter ersetzt. Der lokale JMAP-Responder implementiert Mailbox/query/get, Email/query/get und Thread/get einschließlich Ergebnisreferenzen; andere Methoden bleiben unvollständig oder werden abgelehnt.



## Implementierter erster Datenpfad



- `src/mail-repository.ts`: lokale IPC-Abfragen, persistente Flag-Änderungen und separater Synchronisierungsaufruf.

- `src/desktop/`: originale Bulwark-Komponenten und Hostadapter. Lokale Sitzung zuerst laden, dann Hintergrundabgleich. Die ältere LocalMailbox-Entwicklungsansicht ist kein Anwendungseinstieg mehr.

- `src-tauri/src/replica.rs`: kontogetrennte Objekte, Serverzustände und geordnete Änderungswarteschlange in redb.

- `src-tauri/src/sync.rs`: HTTPS-Sitzungsermittlung, Mailbox-Vollabgleich, Email-Erstabgleich und paginierte `Email/changes`-Abgleiche.



Objekte und ihr Serverzustand werden transaktional geschrieben. Ein abgebrochener

Abgleich veröffentlicht keinen unvollständigen Checkpoint. `cannotCalculateChanges`

löst einen neuen Vollabgleich aus. Beim Erstabgleich wird geprüft, dass sich der

Email-Zustand während der Abfrage nicht geändert hat; sonst wird ohne Veröffentlichung

abgebrochen und beim nächsten Durchlauf erneut versucht.



Lokale Änderungen liegen als separate Absichten über dem Serverdatenbestand.

Dadurch überschreibt ein Download sie nicht. Die Warteschlange überlebt Neustarts

und Vollabgleiche. Erfolgreiche Änderungen werden einzeln bestätigt; später erzeugte

Änderungen bleiben erhalten. `ifInState` schützt Serverupdates; ein `stateMismatch`

bleibt für den nächsten Abgleich vorgemerkt. Für unterstützte Flags gilt: Die jüngste

lokale Absicht wird nach Aktualisierung der Serverbasis erneut versucht.



Permanente Fehler wie fehlende Nachrichten oder Schreibrechte bleiben sichtbar als

blockierte Einträge gespeichert und verhindern nicht den Abgleich anderer Änderungen.

Eine Oberfläche zum Verwerfen oder erneuten Freigeben dieser Einträge fehlt noch.

Nur idempotente Gelesen-/Markiert-Updates werden automatisch wiederholt; Versand

braucht eine separate Strategie gegen doppelte Zustellung.



## Noch offene Arbeit



- Die eingebundene Bulwark-Oberfläche funktional vervollständigen; alle JMAP-Lese-/Schreibmethoden, Ereignisse und Nebenpfade abdecken.

- Verschieben, Löschen, Entwürfe, Versand, Suchindex und weitere Mailfunktionen.

- Vollständige HTML-Textteile und Anhänge offline; aktuell Text-/HTML-Teile mit maximal 256 KiB pro Teil, keine Blob-Synchronisierung.

- Kalender, Kontakte, Dateien und die Funktionen der vollständigen Bulwark-Edition einschließlich OAuth, 2FA, Admin, Plugins und Einstellungssynchronisierung.

- Erstabgleich mit paginierter Staging-Ablage statt derzeitigem Aufbau im Arbeitsspeicher; lokale Seitenabfragen statt vollständiger Nachrichtenlisten.

- Der erste Zugang wird vor dem Speichern per JMAP-Sitzung geprüft und im nativen Systemschlüsselbund gesichert. Eine vorhandene `.env.local`/`.env.local.old` dient nur zur einmaligen Debug-Migration. Die Profilregistrierung liegt in redb; jedes Profil enthält mehrere Konten, deren Zugangsdaten getrennt im Systemschlüsselbund liegen. Profil- und Kontowechsel laden den ausgewählten JMAP-Zugang neu und trennen Profilcaches.

- Verschlüsselung lokaler Kontodaten, Speicherlimits und Cache-Verwaltung.

- Native Linux-/macOS-Validierung, Installationspakete, Signierung und Updates.



## Verifikation



24 Rust-Tests prüfen Persistenz nach Neustart, Kontotrennung, atomare Checkpoints,

Vollabgleich-Rennen, paginierte Deltas, Netzwerkabbrüche, abgelaufene Zustände,

Bestätigungen, lokale Folgeänderungen und permanente Ablehnungen. Der Frontend-Build

mit TypeScript 7.0.2 und der Windows-Tauri-Debug-Build sind erfolgreich. Linux und

macOS und ein vollständiger Live-Erstabgleich sind noch nicht getestet. Ein separat ausgeführter Nur-Lese-Live-Test hat Sitzung, Mailboxen, bis zu zehn Nachrichten und das erneute Lesen aus redb erfolgreich geprüft. Die automatisierten

Sync-Tests verwenden einen skriptgesteuerten Transport statt eines echten Servers.



## Nur-Lese-Betrieb



JMAP_READ_ONLY ist standardmäßig true. Die native IPC-Schicht sperrt neue Änderungen, der Synchronisierer überspringt die Warteschlange und der HTTP-Transport erlaubt nur Mailbox/get, Email/get, Email/query und Email/changes. Unbekannte Methoden sind gesperrt. Der Live-Test erzwingt diesen Modus unabhängig von der Konfiguration und verwendet eine temporäre Datenbank. Sitzungsumleitungen sind nur innerhalb derselben HTTPS-Herkunft erlaubt.





## Originale Oberfläche und Add-ons



Der Einstieg nutzt Bulwarks originale Mailansicht sowie Einstellungen, Kalender-, Kontakt- und Datei-Komponenten. Mail und Kalender sind lesend an die lokale Replik angebunden; Kontakte und Dateien sind noch offen. Plugin-Installation und Sandbox-Ausführung nutzen das originale Paketformat und die originalen Host-APIs. Pakete und UI-/Plugin-Daten liegen in redb. Details und ausdrücklich noch nicht unterstützte APIs: docs/addons.md.



Der Browser-Integrationstest prüft mit synthetischen Daten Mailanzeige, ZIP-Installation, Aktivierung, einen Plugin-Speicherzugriff und Wiederladen nach Neustart. Die native Testsuite enthält zusätzlich Tests für lokale Abfragen und JMAP-Ergebnisreferenzen. Keine vollständige Funktionsgleichheit oder plattformübergreifende Plugin-Kompatibilität ist damit nachgewiesen.







## Kalender-Replik



Der Desktop-Bootstrap initialisiert Bulwarks Kalenderfähigkeit ausdrücklich; dies fehlte bisher und versteckte den Navigationseintrag. src-tauri/src/calendar.rs speichert kontogetrennte Kalender-Snapshots inklusive Teilnehmeridentitäten, Terminen, Wiederholungsregeln und Overrides. Vollabgleich und CalendarEvent/changes veröffentlichen einen neuen Snapshot erst nach erfolgreichem Abschluss. Ein Abbruch erhält den alten Snapshot. Stalwarts getrennte Abrufe von utcStart/utcEnd und recurrenceOverrides werden berücksichtigt.



src/desktop/calendar.ts beantwortet lokale Kalender-JMAP-Aufrufe. Kalenderrechte werden im Nur-Lese-Modus eingeschränkt; Schreibmethoden werden abgelehnt. Datumsbereiche und Wiederholungen verwenden Bulwarks vorhandene Zeit- und Wiederholungshelfer. Die Kalenderansicht wird nach erfolgreichem Hintergrundabgleich aktualisiert. Gemeinsame Konten und alle Suchfilter sind noch nicht vollständig abgedeckt.



Der Live-Nur-Lese-Test hat einen Kalender und zwei Termine inklusive Folgeabgleich und erneutem redb-Öffnen geprüft. Der synthetische UI-Test prüft die Kalendernavigation, Terminanzeige und erneute Anzeige bei nicht erreichbarem Server. Der Schließpfad wird zusätzlich durch zwei Tests und einen echten Windows-Prozess-Schließtest geprüft.





## Freigaben und Sitzungsaktualisierung



`refresh_desktop_session` aktualisiert den lokal gespeicherten JMAP-Kontobestand.

Die App startet weiterhin sofort aus der gespeicherten Sitzung. Im Hintergrund

werden Änderungen an Session-State, Accounts und Fähigkeiten erkannt und der

ursprüngliche Bulwark-Client mit der neuen lokalen Sitzung ersetzt. Ohne Netzwerk

bleibt die zuletzt bekannte Sitzung verfügbar.



`src/desktop/accounts.ts` wählt alle persönlichen und freigegebenen Konten pro

Fähigkeit aus. Wie im Upstream gilt der serverweite Capability-Fallback nur für

explizit freigegebene Accounts mit leerer Capability-Liste, niemals gegen eine

explizite Einschränkung. Rust prüft dieselbe Regel beim Verbindungsaufbau.



`local_jmap` beantwortet auch gemischte Batches mit verschiedenen accountIds aus

separaten lokalen Repliken. Ergebnisreferenzen bleiben an ihre benannten früheren

Antworten gebunden. Vor dem Lesen werden aktuelle Sitzungszugehörigkeit und

Capability geprüft. Entfernte Konten werden nach erfolgreicher Sitzungsaktualisierung

nicht mehr über den nativen Adapter ausgeliefert. Offline kann eine noch unbekannte

serverseitige Rechteänderung naturgemäß erst beim nächsten erfolgreichen Kontakt

berücksichtigt werden. Alte Cache-Dateien werden nicht automatisch gelöscht.



Jeder Abgleich lädt zunächst alle erlaubten Mailbox-Bäume. Danach werden Kalender

und Nachrichten getrennt je Account synchronisiert. Ein Fehler in einer Freigabe

verhindert nicht den Versuch für andere Konten. Abgelehnte Kalender-Probes für reine

Mailfreigaben werden lokal markiert und an Bulwark als accountNotSupportedByMethod

beantwortet. Dadurch verschwinden berechtigte persönliche Kalender nicht wegen

einer Mailfreigabe ohne Kalenderrecht.



Verifikation: 24 native Tests und vier Browser-Tests; darunter Account-Isolation bei

gleichen Objekt-IDs, gemischte JMAP-Batches, entfernte Freigaben, Capability-Fallbacks,

freigegebene Ordner/Nachrichten und Kalender nach Offline-Neustart. Der Live-Test hat

sieben freigegebene Mailkonten mit Mailbox/get und einer begrenzten Email/query sowie Email/get für höchstens eine Nachricht je nicht leerem Konto geprüft.

Für diese sieben Freigaben meldet Stalwart keine Kalenderunterstützung. Der Test

hat keine Serverdaten verändert und eine temporäre Datenbank verwendet.





## Erster Schreibumfang



Der Benutzer hat den Schreibtest ausdrücklich freigegeben. Die native lokale

JMAP-Schicht akzeptiert Email/set ausschließlich für Änderungen an $seen/$flagged.

Jede Änderung an einer Nachricht wird atomar mit ihren Warteschlangeneinträgen in

redb gespeichert. Ein ungültiger gemischter Patch erzeugt keine halbe Änderung.

Nachfolgende Reads im selben JMAP-Batch sehen den lokal vorgemerkten Zustand.

Serverseitig schreibgeschützte Accounts und der konfigurierbare Nur-Lese-Modus

werden weiterhin geprüft. Ungelesen-/Thread-Zähler werden aus der lokalen Replik

berechnet, damit sie auch vor dem Serverabgleich stimmen.



Der native Synchronisierer verwendet idempotente Keyword-Patches mit ifInState,

bestätigt einzelne Queue-Einträge und liest danach den Serverzustand nach.

Schreibereignisse und Reconnects starten einen Abgleich. Ereignisse während eines

laufenden Abgleichs fordern einen weiteren Durchlauf an; es laufen keine zwei

Worker parallel. Die Statusleiste zeigt die Zahl ausstehender und blockierter

Änderungen. Andere Schreibmethoden bleiben auf der lokalen bzw. Transportgrenze

gesperrt, insbesondere Kalenderänderungen und Versand.



Der explizit gegatete Live-Test hat eine Stern-Markierung über dieselbe persistente

Warteschlange geändert, serverseitig gelesen und mit einem frischen Serverzustand

zurückgesetzt. Er verwendet eine temporäre Datenbank und schreibt vor der Mutation

einen lokalen Recovery-Datensatz ohne Zugangsdaten. Auch nach einem Timeout wird

die Wiederherstellung versucht. Der UI-Test prüft Stern-Änderung bei simuliertem

Serverausfall, den sichtbaren Pending-Status und die spätere Synchronisierung bei

Reconnect. Dies belegt keine Funktionsgleichheit für weitere Schreiboperationen.



## Upstream-Aktualisierungen



`upstream/bulwark` bleibt ein unverändertes Git-Submodul. Desktop-Anpassungen liegen in `src/desktop/`, `src-tauri/` und den kontrollierten Build-Transforms in `vite.config.ts`; dadurch gibt es keine zu pflegenden Upstream-Patches. Für ein Update den gewünschten Bulwark-Commit im Submodul auschecken, danach `upstream/bulwark.lock.json` mit Commit, Version und Lizenz aktualisieren und `npm run build`, `cargo check --manifest-path src-tauri/Cargo.toml` sowie die Browser-Integrationstests ausführen. Anschließend insbesondere Mail-Lesen, Composer, Plugins und geteilte Konten prüfen. Änderungen an Upstream-Dateien gehören nur dann ins Submodul, wenn sie zuerst upstream übernommen wurden.



## Composer-Transport (erster Schritt)



Der originale Composer bleibt unverändert. `src/desktop/bootstrap.ts` leitet Identitätsabfragen und den expliziten Sende-Batch an den Tauri-Befehl `composer_request` weiter. `src-tauri/src/sync.rs` beschränkt ihn auf diese Methoden, prüft Schreibrechte und Capability, nutzt HTTPS und führt keine automatische Wiederholung aus. Zugangsdaten bleiben in Rust. Der Versand ist online-only; eine unklare Transportantwort wird als Fehler angezeigt, damit Nutzer die Zustellung prüfen können, bevor sie erneut senden. Der Sendevorgang ist noch nicht redb-first und umfasst keine Uploads/Anhänge. Entwürfe werden als lokale Email-Objekte in redb gespeichert und nach Neustart angezeigt, aber noch nicht mit Stalwart synchronisiert. Dies ist keine vollständige Mail-Parität.



## Produktname und lokale Daten



Die sichtbare Produktkennung lautet Herakles Mail. Die bestehende Tauri-Anwendungskennung `org.jmapdesktop.client` bleibt vorerst stabil, weil Tauri sie für das App-Datenverzeichnis mit der redb-Mailreplik verwendet. Ein Wechsel benötigt einen getesteten Datenbank-Migrationspfad; ohne ihn würde die bisherige lokale Mailkopie scheinbar verschwinden.





## Diagnose-Protokoll



Native Ereignisse werden als eine JSON-Zeile pro Event in `logs/herakles.jsonl` im Tauri-App-Datenordner geschrieben; bei 2 MiB wird rotiert, drei ältere Dateien bleiben erhalten. Auf Windows liegt der Ordner unter `%APPDATA%\\org.jmapdesktop.client\\logs`. Rust protokolliert Sync-, Composer-, Entwurfs- und Speichervorgänge. Felder enthalten Eventname und Fehlerklasse, keine JMAP-Rohantworten, Zugangsdaten, Adressen oder Nachrichteninhalte. Die Statuszeile zeigt zusätzlich ausstehende und blockierte Entwurfslöschungen; genaue Syncfehler erscheinen dort.



## Push und Online-Modus



Der native Client hält Stalwarts JMAP-EventSource-Verbindung (SSE) offen. `StateChange`-Ereignisse lösen einen normalen Delta-Abgleich nach redb aus; bei Verbindungsabbruch verbindet Rust erneut. Bei aktiver SSE-Verbindung gibt es keinen periodischen Abgleich. Ein Abgleich erfolgt beim Start, nach lokalen Schreibabsichten, beim Wechsel von Offline auf Online und nach Push-Reconnect. StateChange-Meldungen werden fuer 750 ms gebuendelt. Nur solange Push getrennt ist, erfolgt ein Sicherheitsabgleich nach 60 Sekunden. Die SSE-Verbindung wird nach 90 Sekunden ohne Daten als unterbrochen behandelt (angeforderter Ping: 30 Sekunden). Der Schalter unten rechts setzt einen gespeicherten Offline-Modus, der Sync, Push und Versand pausiert; im Automatikmodus folgt die App dem System-Netzwerkstatus. „Online“ zeigt einen erfolgreichen Serverabgleich an, nicht nur eine aktive WLAN-Verbindung. Auf Android funktioniert die SSE-Verbindung im Vordergrund; Benachrichtigungen bei pausierter oder geschlossener App benötigen später eine Android-Push-Subscription.



## Mail: Verschieben und Papierkorb



Der lokale Email/set-Adapter akzeptiert zus�tzlich mailboxIds-Ersetzungen und

JSON-Pointer-Patches. Er pr�ft vorhandene Zielordner, mayAddItems/mayRemoveItems

und mindestens eine verbleibende Ordnerzuordnung. Gemischte ung�ltige Patches

werden vor dem Speichern verworfen. Bulwarks originale Verschiebe- und

Papierkorbaktionen bleiben unver�ndert.



Die vollst�ndige gew�nschte Ordnerzuordnung wird als dauerhafte Absicht in der

bestehenden redb-Warteschlange gespeichert. Der Worker sendet eine idempotente

mailboxIds-Ersetzung mit ifInState; bei Konflikten bleibt die Absicht f�r den

n�chsten Abgleich erhalten. Wie bei Flags gewinnt die sp�teste lokale Absicht

nach Aktualisierung der Serverbasis. Permanente Ablehnungen bleiben sichtbar

blockiert. Endg�ltiges L�schen normaler Nachrichten bleibt gesperrt.



Synthetische Tests pr�fen Ordnerrechte, Pointer-Escapes, ung�ltige Mischpatches,

Neustart, Kontotrennung, Vollabgleich und Best�tigung vor einer neueren �nderung.

Ein Live-Verschiebetest und die Pr�fung der nativen Oberfl�che auf Linux stehen

noch aus. Laut Benutzer startet und l�uft der Client inzwischen auch auf Linux;

eine vollst�ndige Linux-Funktionspr�fung ist damit noch nicht dokumentiert.



Der Browser-Integrationstest pr�ft zus�tzlich die originale Papierkorbaktion bei

simuliertem Serverausfall, den Pending-Status, erneutes Laden und anschlie�enden

Reconnect. Die native Testsuite umfasst jetzt 38 erfolgreiche synthetische Tests;

f�nf Live-Tests bleiben standardm��ig ignoriert.



## Mail-Anhaenge und Offline-Downloads



`src-tauri/src/blobs.rs` speichert unveraenderliche Blob-Bytes in einer eigenen

redb-Tabelle, getrennt nach Profil/Server/Login/JMAP-Konto. `local_blob` prueft

aktuelle lokale Sitzungsrechte und eine Referenz aus einer lokal vorhandenen

Nachricht. Ein Cacheeintrag allein berechtigt nicht zum Lesen. Nach serverseitiger

Nachrichtenloeschung sind verwaiste Cacheeintraege nicht mehr ueber IPC abrufbar.



Bulwarks originaler Blob-Hook liest zuerst redb. Bei einem Cachemiss fordert er

`sync_blob` an; nur Rust laedt die Datei und speichert sie vor dem erneuten lokalen

Lesen. Parallele Anfragen derselben Datei werden im Desktop-Adapter gebuendelt.

Der Download verwendet JMAPs downloadUrl-Vorlage gemaess RFC 8620 Abschnitt 6.2

(https://www.rfc-editor.org/rfc/rfc8620.html#section-6.2). Platzhalter werden

percent-codiert; ausschliesslich dieselbe HTTPS-Herkunft wie die API ist erlaubt.

HTTP-Umleitungen werden nicht verfolgt. Zugangsdaten und Downloadantworten werden

nicht protokolliert. Antwortgroesse und bekannte Part-Groesse werden geprueft;

unvollstaendige Downloads werden nicht in redb veroeffentlicht.



Beim regulaeren Mailabgleich werden bis zu vier fehlende Teile mit jeweils

hoechstens 1 MiB vorgeladen (maximal 4 MiB und zehn Sekunden pro Durchlauf).

Groessere Teile und Originalnachrichten laden bei Bedarf nach. Das Downloadlimit

liegt bei 25 MiB pro Blob, das Cachelimit bei 256 MiB pro Konto. Ein voller Cache

verhindert weitere Downloads, vorhandene Dateien bleiben lesbar. Automatische

Bereinigung, ein Cachemanager, Uploads und Composer-Anhaenge fehlen noch. Bereits

heruntergeladene Anhaenge funktionieren offline; noch fehlende Dateien brauchen

eine Verbindung. Das betrifft auch noch nicht gespeicherte HTML-Inlinebilder.



Verifikation: Native Tests pruefen Binaerdaten nach Neustart, Kontotrennung,

Referenzpruefung nach Loeschung, Unveraenderlichkeit, Dateigroessenlimit und sichere

Downloadvorlagen. Der Browser-Integrationstest laedt ueber die originale

Anhang-Schaltflaeche eine synthetische Binaerdatei und vergleicht ihre Bytes;

ein zweiter Download bei simuliertem Serverausfall verwendet den lokalen Cache.

`live_blob_read_only` erzwingt read-only, laedt hoechstens einen kleinen Anhang aus

einer auf zehn Nachrichten begrenzten Stichprobe und benutzt eine temporaere

Datenbank. Der erste Live-Versuch am 2026-11-04 scheiterte vor dem Serverkontakt,

weil im Testprozess kein nutzbarer Zugang geladen werden konnte.


Die Verbindungsanzeige unterscheidet Offline, Sync mit aktuellem Arbeitsschritt
(Sitzung, Ordner, Kalender, Nachrichten oder lokale Ansicht) und Online. Ein
Abgleichfehler nach erfolgreichem Serverkontakt erscheint separat. Tests pruefen
Leerlauf ohne Polling bei aktivem Push, gebuendelte Push-Meldungen, Sicherheits-
abgleich bei Push-Ausfall, Reconnect und den manuellen Offline-/Online-Wechsel.

## Composer: Offline-Ablehnung und Submission-Bereinigung

Offline-Sendeversuche werden vor jedem Serverzugriff nativ abgelehnt. Der
Desktopadapter uebersetzt ausschliesslich diese lokale Offline-Ablehnung in
Bulwarks OfflineUnavailableError. Netzwerkfehler nach moeglichem Versand werden
nicht als sicher ungesendet eingestuft und niemals automatisch wiederholt.
Ein Eintrag in Gesendet bzw. eine angenommene EmailSubmission beweist keine
Ablage im Posteingang des Empfaengers.

Angenommene Submission-IDs erhalten einen kontogetrennten Beleg in redb. Der
urspruengliche Bulwark-Aufruf zur anschliessenden Bereinigung darf ausschliesslich
diese Submission-Metadaten entfernen; unbekannte IDs, andere Konten, Update-Patches
und der Nur-Lese-Modus bleiben gesperrt. Nachrichten werden dabei nicht geloescht
oder nochmals versendet. Gemischte Entwurfs-/Submission-Bereinigungsbatches werden
auf den lokalen Entwurfspfad und den begrenzten nativen Submission-Pfad aufgeteilt.
Ein Fehler beim lokalen Speichern des Belegs wird protokolliert, darf aber einen
bereits angenommenen Versand nicht nachtraeglich als gescheitert zurueckgeben.

Die Desktop-Statuszeile reserviert unten 40 Pixel ausserhalb des Bulwark-
Arbeitsbereichs. Verbindungsumschalter, Profile und Abgleichstatus liegen darin;
der Composer und seine Senden-Schaltflaeche enden darueber. Browserpruefungen
kontrollieren die getrennten Bereiche bei 1440 und 640 Pixeln Fensterbreite
sowie mit geoeffnetem Composer im Desktoplayout.

## Desktop-Schreibjournal und Offline-Freigabe (aktueller Stand)

Die Offline-Schreiblogik liegt vollstaendig ausserhalb des unveraenderten
Bulwark-Submoduls: in src/desktop/ und im nativen Rust-Adapter. Die unteren
Status- und Freigabedialoge gehoeren zur Desktophuelle. Gepruefte Build-Transforms
verbinden die originalen Composer- und Kalenderaktionen mit den Desktopadaptern;
es gibt keinen Bulwark-Fork und keine Aenderung am Upstream-Quellbaum.

Eine strukturierte Mail wird vor jedem Versand als fertiger JMAP-Auftrag mit
stabiler Message-ID in redb gespeichert. Bei gehaltenen Aufträgen erhält die
originale Oberfläche ein ausdrückliches queued-Ergebnis. Bei freigegebenem
Online-Versand liefert der native Pfad die echte JMAP-Antwort zurück. Offline nimmt der
Client Senden ebenso an wie CalendarEvent/create/update/destroy sowie
FileNode-Umbenennen, Verschieben, Ordneranlegen und Loeschen. Kalender- und
Dateiabsichten ueberlagern die separate Serverbasis; ein lesender Abgleich
ueberschreibt sie nicht. Neue lokale Objekte haben stabile lokale IDs. Ihre
späteren Server-IDs werden kontogetrennt fuer abhaengige Aktionen gespeichert.
Neue Datei-Inhalte, neue Composer-Anhaenge, rohe S/MIME-Sendungen, Kontakte,
Kalenderverwaltung und weitere noch nicht portierte Methoden bleiben gesperrt;
nicht gespeicherte Uploads werden niemals als erfolgreich bestaetigt.

Offline-Auftraege bleiben gehalten. Beim Wiederverbinden oder Neustart mit
vorhandenen Auftraegen fragt die Desktophuelle, welche Aktionen uebertragen
werden sollen. Ohne Auswahl bleiben sie lokal; der lesende Hintergrundabgleich
laeuft weiter. Freigaben gelten fuer konkrete Operationsnummern je Konto.
Weitere Bearbeitungen erhalten neue Nummern und sind durch eine alte Freigabe
nicht abgedeckt. Ein erneuter Offline-Wechsel haelt noch nicht abgeschlossene
Auftraege wieder zur Pruefung zurueck. Bereits begonnene Anfragen koennen dadurch
nicht rueckgaengig gemacht werden. Reine Entwuerfe bleiben getrennt vom explizit
vorgemerkten Versand. Das Bearbeiten einer vorgemerkten Mail verschiebt sie in
einer redb-Transaktion zurueck zu einem lokalen Entwurf und zieht den noch nicht
gestarteten Versandauftrag zurueck.

Nur der native Synchronisierer uebertraegt freigegebene Auftraege. Identitaeten,
Nachrichten, Kalender und Dateimetadaten werden von der UI ausschliesslich aus
redb gelesen. Der native Worker aktualisiert auch die lokalen Identitaeten.
Der fruehere direkte Composer-IPC-Transport wurde entfernt. Ein Nur-Lese-Modus
und aktuelle Account-Capabilities werden weiterhin nativ geprueft.

Vor einem moeglicherweise wirksamen Serveraufruf wird der Auftrag dauerhaft
als laufend markiert. Ein Neustart mit laufendem Auftrag oder eine unklare
Transportantwort fuehrt zu einem sichtbaren unklaren Status ohne automatischen
Neuversand. Definitive Ablehnungen bleiben blockiert. Kalender- und Dateiupdates
verwenden ifInState und ueberschreiben externe Aenderungen bei einem Konflikt
nicht automatisch. Bestaetigte Operationen werden erst nach einem erfolgreichen
lesenden Folgeabgleich aus der lokalen Ueberlagerung entfernt. Submission-
Metadaten bereinigt ebenfalls nur der Worker. Eine angenommene Submission ist
kein Nachweis fuer die Ablage im Empfaengerpostfach.

Die Statusleiste zaehlt nur ausstehende Aktionen; normale Entwuerfe werden nicht mitgezaehlt. Die Uebersicht
zeigt Aktion, Objekt, Konto und Status, ermoeglicht die Auswahl zur Freigabe und
das Oeffnen des zugehoerigen Objekts. Dauerhaft blockierte und unklare Auftraege
werden nicht durch eine pauschale Freigabe erneut gesendet. Ein allgemeiner
Konflikteditor, Cachebereinigung und vollstaendige Datei-Inhaltsbearbeitung
bleiben offen.

Native Tests pruefen Persistenz, Kontotrennung, atomare Entnahme zum Bearbeiten,
Freigabe einzelner Versionen, lokale Kalenderueberlagerung ohne Veraenderung der
Serverbasis, Dateiabhaengigkeiten, Rollback ungueltiger Mischpatches und die
Sperre gegen Wiederholung unklarer Sendungen. Der Browsertest klickt Senden im
originalen Composer bei simuliertem Offline-Modus, speichert daneben Termin-
und Dateiaktionen, laedt neu und prueft die getrennte Freigabe beim Reconnect.
Alle diese Tests verwenden synthetische Daten; es wurden keine echten Mails
versendet und keine Live-Kalender oder Live-Dateien veraendert.


## Lokale Uploads und Composer-Anhänge

`src-tauri/src/uploads.rs` hält Datei-Bytes und Metadaten atomar in eigenen
redb-Tabellen. Die Ablage ist wie die Mailreplik nach Profil, Server, Login und
JMAP-Konto getrennt. Lokale Blob-IDs bleiben über Neustarts stabil. Die originale
Bulwark-Schnittstelle `holdUpload` speichert Dateien per IPC; sie führt keinen
Browser-Upload aus. Entwürfe und Versandaufträge können diese lokalen IDs auch
in verschachtelten MIME-Strukturen verwenden. Fehlende oder kontofremde lokale
Referenzen werden vor dem Speichern abgelehnt.

Grenzen: 25 MiB pro Datei und 256 MiB Uploadablage pro Konto, zusätzlich zum
bestehenden Downloadcache. Auch leere Dateien werden unterstützt. Automatische
Bereinigung verwaister Uploads, vollständige Cacheverwaltung und Ersetzen
bestehender Datei-Inhalte sind noch offen. Lokal entfernte Anhänge geben daher
noch keinen Speicher frei.

Nur der native Schreibpfad überträgt Dateien, und nur als Abhängigkeit eines
freigegebenen Versand- oder FileNode/create-Auftrags. Die Binärübertragung folgt
[RFC 8620 Abschnitt 6.1](https://www.rfc-editor.org/rfc/rfc8620.html#section-6.1).
Der Transport prüft aktuelle Kontounterstützung, Schreibschutz, HTTPS-Herkunft,
Serverlimit und die Uploadantwort einschließlich Konto, Größe und Blob-ID.
Umleitungen werden nicht verfolgt; Antworten und Inhalte werden nicht geloggt.

Blob-ID-Zuordnungen werden dauerhaft pro Konto gespeichert. Eine aktuelle
Zuordnung wird kurz wiederverwendet, nach fünf Minuten werden die lokalen Bytes
neu übertragen, damit abgelaufene unreferenzierte Serverblobs nicht dauerhaft
verwendet werden. Nur Blob-ID-Felder werden im abgeleiteten Serverauftrag ersetzt;
der lokale Entwurf und Nachrichtentext bleiben unverändert. Die letzte Zuordnung
kann nach einer lesenden Synchronisierung auch lokale Datei- und Anhangszugriffe
unter der Server-ID beantworten, solange das Objekt weiterhin lesbar referenziert
ist. Uploadfehler halten den Auftrag zur erneuten Prüfung zurück. Erst nach
Uploadvorbereitung wird ein Versand als laufend markiert. Unklare Antworten nach
dem eigentlichen Versand bleiben weiterhin ohne automatischen Neuversand.

Verifikation: Native Tests prüfen binäre Persistenz, Kontotrennung, lokale Entwürfe
mit Anhängen, doppelte MIME-Referenzen, persistente Zuordnungen, Uploadfehler,
Offline-Sperren, Dateierstellung im Journal und Transportgrenzen. Der Browsertest
fügt über den originalen Composer eine Binärdatei an, liest sie offline bytegenau,
merkt die Mail offline vor und prüft Wiederladen und gezielte spätere Freigabe.
Diese Prüfungen verwenden synthetische Daten; echte Uploads, Versand mit Anhängen
und neue Serverdateien wurden nicht ausgeführt.


## Blockierte und unklare Aktionen lokal auflösen

Die Desktop-Änderungsübersicht bietet pro Auftrag „Prüfen“. Der Vergleich zeigt
die betroffenen Felder aus dem zuletzt gespeicherten Serverstand und der lokalen
Absicht. Es findet dabei kein Netzwerkzugriff statt. Bei unklaren Sendungen wird
kein fehlender Servereintrag behauptet: Die Zustellung wurde nicht automatisch
geprüft.

- Definitiv abgelehnte Mailänderungen sowie Kalender- und Dateiaktionen können
  erneut vorgemerkt werden. Für Kalender-/Dateiupdates wird nur der gespeicherte
  Patch auf die neue Serverbasis angewendet; externe Änderungen an anderen
  Feldern bleiben erhalten. Gelöschte Objekte und nicht mehr anwendbare Patches
  lassen sich nicht auf diese Weise erneut vormerken.
- Der Ersatzauftrag erhält eine neue Operationsnummer und wartet auf Freigabe.
  Frühere Auswahlen gelten dafür nicht. Ein zwischen Vergleich und Auflösung
  geänderter Serverstand oder eine geänderte Operationsnummer erfordert erneute
  Prüfung. Eine ältere Mailabsicht darf neuere lokale Änderungen nicht überholen.
- Abgelehnte Sendungen werden weiterhin über „Als Entwurf bearbeiten“ korrigiert.
  Es gibt keinen pauschalen Versand-Retry. Unklare Aufträge erhalten überhaupt
  keine Wiederholungsaktion. Sie lassen sich nach ausdrücklicher Bestätigung
  einer Serverprüfung nur lokal abschließen.
- „Lokale Absicht verwerfen“ entfernt genau den ausgewählten Auftrag. Die Ansicht
  zeigt anschließend wieder die Serverbasis und verbleibende lokale Änderungen.
  Es wird nichts auf dem Server zurückgenommen. Rein lokale Auflösung ist auch
  offline und im Nur-Lese-Modus möglich; erneutes Vormerken bleibt im Nur-Lese-Modus gesperrt.
- Laufende und bereits bestätigte Aufträge können nicht verworfen werden.
  Eine noch nicht zugeordnete lokale Neuerstellung mit abhängigen Aufträgen
  bleibt erhalten, bis die untergeordneten Absichten entfernt wurden.

`src-tauri/src/changes.rs` prüft Konto, Capability, Prüfstand und erlaubten
Zustandsübergang. Die Auflösungsbefehle verwenden denselben nativen Sync-Lock wie
der Schreibworker. redb-Transaktionen schützen das Ersetzen und Entfernen exakter
Operationen; Konflikt-Rebasing prüft die Basis erneut unter dem Datenbankschreiblock.
Verworfene Journalaufträge bleiben einschließlich ihres ursprünglichen Status,
Fehlers und Auftrags als lokaler Beleg in einer separaten redb-Tabelle erhalten.
Diese Belege sind derzeit noch nicht in einer eigenen Historienansicht zugänglich.
Mailflags und Entwurfslöschungen benötigen keinen unklaren Versandbeleg und werden
beim Verwerfen aus ihrer jeweiligen Queue entfernt. Lokale Zustandskennungen und
Ansichten werden nach der Auflösung aktualisiert, ohne einen Versand anzustoßen.

Verifikation: Native Tests prüfen veraltete Prüfstände, Erhalt externer Felder,
neue Freigabenummern, neuere lokale Mailabsichten, Kontotrennung, Nur-Lese-Sperren,
Abhängigkeiten und persistente unklare Versandbelege. Der Browsertest prüft den
Vergleich bei 1440 und 640 Pixeln Breite, erneutes Vormerken ohne Transport,
bestätigtes Verwerfen und Abschließen einer unklaren Sendung ohne Wiederholung.
Alle Prüfungen verwenden synthetische Daten.


## Begrenzter Upload-Live-Test

Am 14. November 2026 wurde `upload_roundtrip::live_upload_blob_roundtrip` nach
Korrektur der HTTP-Antwortbehandlung erfolgreich gegen Stalwart ausgeführt.
Er verwendet genau 1024 feste synthetische Bytes pro bewusst gestarteter
Prüfausführung. Er legt keine Email oder FileNode auf dem Server an und ruft
keinen Versand auf. Die Calls, die der Test an `uploads::prepare` übergibt,
sind nur lokale Daten zur Prüfung der Blob-ID-Ersetzung; sie werden niemals an
einen JMAP-Methodentransport übergeben. Der Rücklesepfad erzwingt Nur-Lese-Betrieb.

Geprüft sind lokale Ablage und Wiederöffnen vor dem Upload, Offline-Sperre,
Upload mit dauerhafter Blob-ID-Zuordnung, binärer Vergleich des Downloads,
Downloadcache sowie lokale Lesezugriffe und Kontotrennung nach erneutem
redb-Öffnen. Die abschließende Offlineprüfung erhält keine Netzwerkschnittstelle.
Die eigentliche Testdatenbank liegt in einem temporären Verzeichnis.

Der erste Uploadversuch wurde vom Client nicht bestätigt. Die damalige
Implementierung erwartete ausschließlich HTTP 201. Stalwarts
[UploadResponse](https://github.com/stalwartlabs/stalwart/blob/main/crates/jmap/src/api/mod.rs)
verwendet `JsonResponse::new`, dessen
[Standardstatus HTTP 200](https://github.com/stalwartlabs/stalwart/blob/main/crates/http-proto/src/lib.rs)
ist. Der Client akzeptiert jetzt HTTP 200 und 201, jeweils nur mit vollständiger
passender Blob-Antwort. HTTP 202/204, Weiterleitungen und Fehlerstatus bestätigen
keinen Upload. Ein Regressionstest prüft die Erfolgsstatus, Kontozuordnung,
Dateigröße, fehlende Felder und Antwortgrößenlimit. Uploadfehler geben höchstens
Statuscode oder Fehlerklasse aus, niemals Antworttext.

Insgesamt gab es zwei Uploadversuche mit je 1 KiB Testdaten. Der erste bleibt
unbestätigt; die identifizierte Korrekturprüfung hat Upload, Rücklesen und
Offline-Neustart vollständig bestätigt. Die dauerhaften Belege
`.cache/live-upload-roundtrip.json` und
`.cache/live-upload-roundtrip-http200-fix.json` enthalten nur Status und
Testumfang. Ein Beleg wird vor dem Netzwerkaufruf mit `create_new` gespeichert
und sperrt eine Wiederholung derselben Prüfausführung auch nach einem Timeout.
Es gibt keine automatische Uploadwiederholung. Unreferenzierte Blobs bleiben
der serverseitigen Aufbewahrung/Bereinigung überlassen; eine sofortige Löschung
wird nicht zugesagt. Es wurde keine Nachricht versendet und kein vorhandenes
Serverobjekt geändert. Das ist noch kein Nachweis für den Versand mit Anhängen
oder für FileNode/create.

Der Test ist standardmäßig ignoriert und verlangt im Prozess
`JMAP_UPLOAD_TEST=one-blob-1024`. Der bekannte Korrekturlauf wird zusätzlich durch
`JMAP_UPLOAD_RUN=http200-fix` bezeichnet. Beide Ausführungen besitzen bereits
Belege und sind dadurch lokal gegen erneutes Hochladen gesperrt. Die dauerhafte
Kontoeinstellung `read_only` wird nicht verändert.

Für Profilzugänge kann `JMAP_TEST_REGISTRY` explizit auf die App-Datenbank zeigen.
Der Test kopiert sie unter einem gemeinsam gehaltenen Dateilock in ein temporäres
Verzeichnis und liest ausschließlich die Profilregistrierung. Eine eventuell
nötige redb-Wiederherstellung betrifft nur diese Kopie; die Quelldatei bleibt
bytegleich. Ein aktiver Writer und Quellen über 512 MiB werden abgelehnt.
Der passende vorhandene Schlüsselbundzugang wird anschließend nur in Rust geladen,
ohne Migration, Speichern oder Änderung des globalen App-Kontexts. Synthetische
Tests prüfen die Quellunveränderlichkeit und das Ablehnen geöffneter Datenbanken,
fehlender Profilauswahl, zusätzlicher Uploads und fehlerhafter Rücklesedaten.


## Lesende Kontakte und Adressbücher

`src-tauri/src/contacts.rs` synchronisiert Adressbücher und ContactCards in einer
nach Profil, Server, Login und JMAP-Konto getrennten redb-Ablage. Der native Worker
verwendet `AddressBook/query/get` und `ContactCard/query/get` mit der Contacts-
Capability; sein Transport erzwingt auch bei freigegebenen Mail-Schreibaktionen
Nur-Lese-Betrieb. Der Wire-Datenfluss folgt Bulwarks vorhandenen Methoden und
Stalwarts [ContactCard/get](https://github.com/stalwartlabs/stalwart/blob/main/crates/jmap/src/contact/get.rs).

In dieser ersten Stufe läuft pro Abgleich ein vollständiger, paginierter Abruf:
Abfragen verlangen bis zu 250 IDs, Get-Aufrufe enthalten höchstens 50 IDs.
Zustände vor/nach dem Download, Seitenpositionen, Gesamtzahlen und die vollständige
Abdeckung angeforderter IDs werden geprüft. Ein geänderter Zustand, fehlende oder
doppelte Objekte und Netzwerkabbrüche veröffentlichen keinen Teilbestand.
Die komplette Adressbuch-/Kontaktkopie wird erst nach erfolgreicher Prüfung in
einem redb-Schreibvorgang ersetzt. Dadurch verschwinden auch serverseitig gelöschte
Objekte explizit aus der Replik. Grenzen sind 100.000 Objekte pro Sammlung und
32 MiB pro gespeichertem Snapshot. Paginierte Staging-Ablage und ContactCard/changes
sind noch nicht implementiert; dieser Abgleich baut den Bestand im Arbeitsspeicher
auf. Ein erfolgreicher Leerbestand ersetzt den bisherigen Bestand. Eine definitive
Zugriffsverweigerung oder nicht unterstützte Methode veröffentlicht stattdessen
einen gesperrten Leerbestand, sodass alte Kontakte nicht weiter ausgeliefert werden.

`local_contacts` beantwortet lokale Get-/Query-Batches samt Ergebnisreferenzen,
Eigenschaftsauswahl, Sortierung und Seitenabfragen. Jeder Aufruf prüft die aktuelle
lokale Sitzungszugehörigkeit und Kontofähigkeit. Gleiche Objekt-IDs in verschiedenen
Konten bleiben getrennt; entfernte Freigaben können keine alten Snapshots lesen.
Der lokale allgemeine JMAP-Adapter unterstützt ebenfalls Kontaktmethoden für
kombinierte Mail-/Kontaktbatches. Nicht lesbare Adressbücher und Kontakte ohne
lesbare Buchzuordnung werden ausgeblendet. Die ausgelieferte Buchzuordnung enthält
nur lokal verfügbare lesbare Bücher.

Die Suche unterstützt Text, Namen, E-Mail, Telefon, Organisation, Notiz, Spitzname,
Adressbuch, UID, Kontaktart und Gruppenmitglied sowie AND/OR/NOT. Textsuche ist eine
lokale Suche nach Teilzeichenfolgen ohne sprachabhängigen Serverindex; nicht
unterstützte Filter und Sortierungen werden ausdrücklich abgelehnt. Lokale
Änderungsabfragen verlangen einen neuen Vollabruf (`cannotCalculateChanges`).

Der Bootstrap lädt Bulwarks originale Kontakt-Stores aus IPC und aktualisiert sie
nach dem Hintergrundabgleich. Die Contacts-Capability wird pro persönlichem oder
freigegebenem Konto ausgewertet; ein separat benanntes primäres Kontaktkonto wird
berücksichtigt. Kontaktänderungen werden im Host auch für Bulwarks lokalen Fallback
abgelehnt; native und HTTP-Schreibgrenzen lehnen ContactCard/set und AddressBook/set
weiterhin ab. Kontrollierte Build-Transforms deaktivieren die primären Neu-/Edit-
Schaltflächen. Weitere bisher vorhandene Editor-/Menüpfade können zwar angezeigt
werden, bestätigen aber keine Änderung. Die Desktophülle kennzeichnet die Ansicht
als Nur Lesen. Es gibt noch keine Kontakt-Schreibwarteschlange.

Kontaktfotos werden noch nicht synchronisiert. Der lokale Responder liefert kein
`media` aus, damit das originale Avatar-Element keine externen Bildadressen oder
noch nicht lokal unterstützte Contact-Blobs automatisch nachlädt. Kontaktgruppen
werden als vorhandene ContactCards lesend erhalten; die Principal-Firmenübersicht
und volle Filter-/Schreibparität bleiben außerhalb dieser Stufe.

Verifikation: Die synthetischen nativen Tests prüfen einen Bestand mit 260
Kontakten, Pagination, Fehler und Zustandswechsel ohne Teilveröffentlichung,
Löschungen, erneute Rechtefreigabe, Offline-Neustart, Suche, Ergebnisreferenzen,
Kontotrennung, entfernte Freigaben, Foto-/Buchfilterung und die dauerhafte
Schreibsperre. Die Browserprüfung nutzt die originale Kontaktansicht mit
persönlichem und freigegebenem Adressbuch, Suche, Detailanzeige, deaktivierter
Bearbeitung und Wiederladen bei nicht erreichbarem Server. Es werden ausschließlich
synthetische Kontakte angezeigt oder fotografiert.

`contacts::tests::live_contacts_read_only` hat am 15. November 2026 zwei
Adressbücher und sechs Kontakte in eine temporäre redb-Datenbank geladen, deren
Neustart und einen Folgeabgleich erfolgreich geprüft. Der Test erzwingt unabhängig
von der gespeicherten Kontoeinstellung read_only und prüft vor dem Datenabruf,
dass die beiden Set-Methoden lokal gesperrt sind. Keine Kontaktinhalte oder
Zugangsdaten wurden protokolliert; es wurde nichts auf dem Server geändert.

## Begrenzter nativer Downloadcache (16. November 2026)

Bulwarks lib/offline/cache.ts dient als Vorlage fuer Speicherlimit, aelteste Eintraege zuerst und die Trennung von ausgehenden Daten. Die Rust-Portierung begrenzt weiterhin nur heruntergeladene Binaerdaten auf 256 MiB je Kontoscope, bei maximal 25 MiB pro Blob. Gezielt angeforderte Downloads verdraengen bei Platzmangel die zuerst gespeicherten Downloads atomar mit dem neuen Eintrag. Eine dauerhafte redb-Reihenfolge ueberlebt Neustarts; vorhandene Bloecke ohne Reihenfolge bleiben beim Oeffnen erhalten und werden bei Platzmangel zuerst verdraengt. Lesen und doppelte Schreibaufrufe erneuern das Alter nicht.

Automatisches Vorladen verdraengt keine Eintraege, um wiederholte Verdraengungsdownloads beim Abgleich zu vermeiden. Mailreplik, JMAP-Checkpoints, Entwuerfe, Schreibjournal und lokale Uploads liegen ausserhalb dieser Bereinigung. Verdraengte Downloads sind offline nicht mehr verfuegbar und werden bei Bedarf online erneut geladen. Anders als Bulwark werden keine Nachrichtengruppen entfernt: Unsere redb-Replik bleibt die verbindliche Arbeitsgrundlage. Kein globales Speicherlimit, keine Verschluesselung, keine Uploadbereinigung und keine garantierte Verkleinerung der redb-Datei; freigegebene Seiten koennen wiederverwendet werden. Ein Einstellungsdialog fehlt weiterhin.

Synthetische Tests pruefen dauerhafte FIFO-Reihenfolge, Kontotrennung, unveraenderliche Duplikate, Prefetch ohne Verdraengung, Ablehnung zu grosser Eintraege ohne Datenverlust sowie Migration des alten Downloadcache. Keine Live-Serverzugriffe erforderlich.

## Einstellbarer Downloadcache (16. November 2026)

In den Einstellungen steht oberhalb der originalen Bulwark-Ansicht der aufklappbare
Bereich Offline-Speicher / Downloadcache. Er zeigt je Konto der aktuellen lokalen
Sitzung Downloadbytes, Eintragszahl und Limit. Die Anzeige umfasst nicht die gesamte
redb-Datei oder lokale Uploads. Die Belegung kann lokal aktualisiert werden.

Das Limit ist in ganzen MiB zwischen 25 und 4096 einstellbar (Standard 256).
Rust validiert Kontozugehoerigkeit und Wertebereich. Limit und gegebenenfalls
notwendige FIFO-Bereinigung werden in derselben redb-Transaktion geschrieben.
Neue Downloads lesen das Limit innerhalb ihrer Schreibtransaktion; Prefetch
verdraengt weiterhin keine Eintraege. Die Einstellung funktioniert offline und
im Nur-Lese-Modus. Eine Verkleinerung zeigt vor dem Speichern den Hinweis auf
sofortiges Entfernen alter Downloads und deren fehlende Offline-Verfuegbarkeit.
Keine Aenderung betrifft den Server, Entwuerfe, Schreibjournal oder Uploadablage.

Native Tests pruefen Grenzwerte, Neustart, Kontotrennung, sofortige Bereinigung,
Vergroesserung und Anwendung des gespeicherten Limits auf Download und Prefetch.
Die Browserpruefung nutzt synthetische Konten und nachgebildetes IPC, prueft
Speichern/Wiederladen und zeigt den Bereich auch bei 640 Pixeln Breite.

## Bulwark-Aufrufe gegen Stalwart-Semantik (16. November 2026)

Die erste gezielte Angleichung betrifft Email/query. Ausgangspunkte sind Bulwarks
getEmails, getTagCounts und buildListSort im unveraenderten Submodul
(lib/jmap/client.ts und lib/message-list-order.ts). Diese Funktionen kombinieren
Mailbox-/Keywordfilter, mehrstufige Sortierung, Position/Limit und Ergebnisreferenzen
auf Email/get und Thread/get. Die bestehenden Abrufe im Rust-Worker bleiben bestehen;
diese Stufe erweitert die lokale Beantwortung, ohne zusaetzliche Netzwerkpfade.

Als Serverreferenz wurde Stalwart-Commit
e7cde9b3fac32d2a254ef782efec56f2373ec5d4 festgehalten:
- crates/jmap/src/email/query/filter.rs: Thread-Keywordfilter ueber sichtbare Nachrichten.
- crates/jmap/src/api/query.rs: negative Positionen, Anchor-Offsets und Antwortposition.

email_query.rs validiert unterstuetzte Filterformen und Queryargumente vor der
Auswertung, auch bei leerer Replik oder nicht passenden Nachrichten. Unbekannte
Filter ergeben unsupportedFilter; falsche Werttypen invalidArguments.
allInThreadHaveKeyword, someInThreadHaveKeyword und noneInThreadHaveKeyword werden
ueber alle lokal sichtbaren Nachrichten desselben Kontos und Threads ausgewertet,
bevor ein zusaetzlicher Mailboxfilter oder collapseThreads die Ergebnisliste begrenzt.
Sortierkomparatoren verwenden bei fehlendem isAscending den Standard true.
Negative Positionen zaehlen vom Ende der gefilterten/sortierten/kollabierten Liste;
Anchor-Offsets werden danach angewendet. Ein Offset hinter dem Listenende liefert
keine IDs und ohne Anchor die Antwortposition 0 entsprechend Stalwarts Builder.

Es handelt sich um eine an redb angepasste Implementierung nach Referenzcode,
nicht um die Einbindung von Stalwarts gesamtem Server. Die Referenz ist kein
Nachweis fuer die Version des betriebenen Servers. Threadfilter koennen nur den
lokal synchronisierten, lesbaren Bestand beruecksichtigen. Die Auswertung arbeitet
weiterhin im Speicher; kein Stalwart-Suchindex wurde portiert. Volltextsuche bleibt
eine vereinfachte lokale Teilzeichenfolgensuche, Datumsfilter verwenden weiterhin
Zeichenkettenvergleich; vollstaendige Such-, Datums- und Sortierparitaet ist offen.

Synthetische Tests pruefen die redb-Replik nach Neustart, Threadfilter ueber
Mailboxgrenzen, Kontotrennung, verschachtelte Filter, negative Positionen,
Anchor/Thread-Collapse, leere Seiten und Fehler unabhaengig vom Datenbestand.
Es wurden keine Live-Schreibtests oder Serveraenderungen ausgefuehrt.

## Vollstaendige statische JMAP-Bestandsaufnahme (16. November 2026)

Die [Kompatibilitaetsmatrix](jmap/matrix.md) ordnet die erfassten Bulwarkmethoden
Stalwart-Quellreferenzen, lokalen Implementierungen und konkreten Testbelegen zu.
[Pflege und Grenzen](jmap/README.md) beschreiben den reproduzierbaren AST-Abgleich.
Argumente, Eigenschaften, Filter, Sortierung, Ergebnisreferenzen, dynamische
Methoden und HTTP-/Push-Transporte stehen maschinenlesbar in jmap/inventory.json.
Neue Methoden, unbewertete dynamische Kontexte oder veraltete Artefakte lassen
check:jmap fehlschlagen. Die Releasepruefung fuehrt diesen Check und die Tests
des Generators aus. Die Bestandsaufnahme implementiert die ausgewiesenen
Funktionsluecken nicht und behauptet keine vollstaendige Protokollparitaet.

## Email/get: Standardprojektion und vollstaendigere Offline-Daten (16. November 2026)

email_get.rs beantwortet Email/get ueber den nativen Batchresponder aus der lokalen
Mailreplik. Die Standard-Eigenschaftsliste folgt Stalwarts email/get.rs; headers
und bodyStructure werden nur auf ausdrueckliche Anforderung ausgegeben. id bleibt
immer enthalten, unbekannte Eigenschaften und falsche Argumenttypen werden auch
bei leerem Bestand abgelehnt. ids:null liest den lokalen Bestand; explizite IDs
werden geordnet dedupliziert, fehlende/in diesem Konto nicht vorhandene IDs stehen
in notFound. Interne gespeicherte Felder gelangen nicht durch die Standardprojektion.

BodyProperties werden rekursiv auf bodyStructure, textBody, htmlBody und attachments
angewendet. subParts bleibt rekursiv vorhanden. BodyValues werden nur entsprechend
fetchTextBodyValues, fetchHTMLBodyValues oder fetchAllBodyValues ausgegeben; ohne
Fetchoption ist die Map leer. maxBodyValueBytes begrenzt UTF-8-Bytes je Teil. Die
Plaintext-/HTML-Kuerzung stammt direkt aus Stalwarts message/body.rs und erhaelt
isEncodingProblem sowie bereits vorhandene isTruncated-Markierungen.

Headernamen werden ohne Beachtung der ASCII-Grossschreibung gesucht. Ohne all gilt
das letzte Vorkommen, mit all bleiben Reihenfolge und Mehrfachwerte erhalten.
Unterstuetzt sind asRaw, asText, asAddresses, asGroupedAddresses, asMessageIds,
asDate und asURLs sowie root-/part-headers. Die Verarbeitung verwendet Stalwart
Labs' mail-parser 1.0 mit full_encoding; Datumsformen werden auf UTC normalisiert.
Die Stalwart-Referenz und Lizenzhinweise stehen in NOTICE und der JMAP-Matrix.

Der Worker fordert nun sender, root-headers und Part-headers sowie alle textuellen
BodyValues an. Abgeschnittene Teile werden je betroffener Nachricht einmal mit
maxBodyValueBytes=8000000 nachgeladen. Antwortkonto/ID-Abdeckung der Basisauswahl,
Nachrichten-ID, Bodywert-Abdeckung und unveraenderter Email-State des Nachladens
werden vor der Veroeffentlichung geprueft. Fehler oder Zustandswechsel lassen den
alten Email-Checkpoint und Datenbestand erhalten. Abbruch vor der nachfolgenden
Snapshotbestaetigung veroeffentlicht ebenfalls keine halbe Replik.

Ein persistenter, kontogetrennter Projektionsmarker erzwingt beim naechsten
Online-Mailabgleich einmalig einen vollstaendigen Ersatzsnapshot fuer alte Repliken.
Die alte Replik bleibt bis zum erfolgreichen Commit lesbar. Der Marker wird erst
danach gesetzt; ein Absturz davor fuehrt hoechstens zu einem erneuten Vollabgleich.
Lokale Entwuerfe, Flags und Schreibabsichten bleiben erhalten. Danach nutzt der
Worker wieder Deltas. Vor dem Upgrade fehlen alte Header/Textdaten offline weiterhin.

Grenzen: Teile oberhalb von 8 MB bleiben ehrlich als abgeschnitten markiert;
maxBodyValueBytes=0 bedeutet keine weitere lokale Kuerzung, aber keinen Netzabruf.
Der Responder parst keine Roh-MIME-Blobs neu, sondern projiziert die vom Server
synchronisierten Strukturen. Optionale Objektmetadaten-Erweiterungen bleiben offen.
Die Replik liegt weiterhin unverschluesselt vor; vollstaendige Texte und Header
vergroessern ihre Datenmenge. Kein Paging der gesamten lokalen Replik hinzugefuegt.

Synthetische native Tests pruefen Standard-/Teilprojektion, IDs/notFound,
Kontotrennung und Neustart, rekursive MIME-Teile, alle Fetchmodi, Headerformen und
Mehrfachwerte, UTF-8-/HTML-Kuerzung sowie Bulwarks Query/Get/Thread-Batch mit
Ergebnisreferenzen. Workerpruefungen decken die einmalige Datenmigration,
Nachladen, Rueckkehr zu Deltas und Fehler/Zustandsrennen ohne Teilveroeffentlichung ab.
Es wurden keine echten Nachrichten gelesen, versendet oder Serverobjekte geaendert.

## Korrektur der Entwurfslistenanzeige (16. November 2026)

Alte lokale Entwuerfe enthielten teilweise keine threadId. Die Daten konnten
lokal geladen werden, die originale Bulwark-Liste brach jedoch beim Rendern ab
und zeigte die allgemeine Fehlermeldung fuer die Nachrichtenliste.
Die fehlende Threadzuordnung wurde mit einem synthetischen Browserfall reproduziert.

Die native lokale Mailansicht ergaenzt fuer lokale Entwuerfe ohne nichtleere
threadId deren eigene id als stabilen kontogetrennten Thread. Unterschiedliche
Entwuerfe werden dadurch nicht zusammengefasst. Bestehende Threadzuordnungen
bleiben erhalten. Diese Kompatibilitaetsprojektion veraendert die alten
redb-Datensaetze nicht und erfindet fuer alte Entwuerfe keine Datumswerte.
Neue lokale Entwuerfe erhalten beim Speichern eine Threadzuordnung und, wenn
receivedAt fehlt oder ungueltig ist, einen lokalen Erstellungszeitstempel;
sentAt wird dadurch nicht gesetzt. Der Upstream bleibt unveraendert.

Der native Regressionstest prueft alte Null-/Leer-Threadwerte, Kontotrennung,
Neustart, vorhandene Threadzuordnungen und unveraenderte persistierte Altdaten.
Der Browsertest navigiert vom Posteingang zu einem lokalen Altentwurf ohne
Datum und wieder zurueck, mit der vom nativen Responder gelieferten Thread-ID.
Keine Diagnose-Probes, zusaetzlichen Mailabrufe oder Konsoleninterzeptoren bleiben
im Bootstrap zurueck. Es wurden keine Serverobjekte veraendert.


### Lokale Einladungsverarbeitung

CalendarEvent/parse wird im Desktopadapter vor der allgemeinen Sperre unbekannter
Parse-Methoden behandelt. Der Rust-Synchronisierungspfad holt fehlende ICS-Blobs;
die Umwandlung selbst liest ausschliesslich kontogetrennte, aktuell referenzierte
redb-Blobs und verwendet Stalwart Labs calcard 0.3.14. Ein Mailkonto braucht zum
Lesen seiner Einladung keine Kalender-Capability. Kalenderkonvertierung,
Zeitzonen, Wiederholungen und Teilnehmer werden durch calcard bereitgestellt.
Metadaten und Eigenschaftsauswahl folgen dem Stalwart-Parse-Responder.
Grenzen: 16 Blobs, 2 MiB je Blob, 256 Eintraege. Fehlerhafte und fehlende Blobs
werden als notParsable bzw. notFound zurueckgegeben. Parsing legt keine Termine
an und versendet keine Zusagen.

Drei native Regressionstests pruefen Konvertierung, Fehler und Autorisierung
sowie Offline-Neustart und Kontotrennung. Die synthetischen Browsertests pruefen
die originale Einladungsanzeige mit dem nativen Adapter. Stand dieser Erweiterung:
81 native Tests erfolgreich, neun explizit ausgeblendete Live-Tests.
Eine gezielte lesende Pruefung der angefragten Scadix-Einladung bestaetigte einen
bereits vorhandenen Termin im persoenlichen Standardkalender: UID, Start,
Zeitzone und Dauer stimmten ueberein. Es wurde kein Duplikat angelegt und keine
RSVP gesendet. Der zugehoerige ignorierte Preflight-Test benoetigt explizite
Profilauswahl und JMAP_CALENDAR_PREFLIGHT=scadix; er erzwingt read-only.


### Versionsschutz bei Kalenderaenderungen und Absagen

Der unveraenderte Bulwark-Banner verwendet UID, SEQUENCE, Recurrence-ID und
Absender-/Organisatorpruefung fuer Absagen. Seine originale Absageplanung wird
synthetisch getestet: alte Absagen, ganze Serien, einzelne Vorkommen mit
Zeitzonenumrechnung und Erhalt anderer Serienausnahmen.
Die native CalendarEvent/set-Grenze vergleicht ifInState jetzt mit dem gesamten
lokalen Zustand (Event-State, Calendar-State und Journalrevision). Ein blosses
local-calendar:-Praefix genuegt nicht. Explizite sequence-Updates muessen
nichtnegative Zahlen sein und duerfen die gespeicherte Version nicht absenken.
Aenderungen ohne sequence behalten ihre bisherige Version; der Schutz ersetzt
keine iTIP-Absenderauthentifizierung. Die UI erhaelt stateMismatch und
invalidProperties unverfaelscht zurueck.

Die getrennte Abfrage fuer recurrenceOverrides muss dieselben vorhandenen und
fehlenden IDs wie die Hauptabfrage enthalten, mit genau einer Zeile je vorhandenem Termin. Stalwart laesst leere optionale
Override-Eigenschaften weg; diese werden nach ID-/Zustandspruefung als null
behandelt. Vorhandene Werte muessen Objekt oder null sein. Bei fehlenden, doppelten oder
fremden Objekten bleibt der zuvor gespeicherte Snapshot erhalten. Tests decken
abgebrochene Updates, Neustart, dauerhaft gehaltene Verschiebungen, Ausschluss
eines Vorkommens und Loeschen ohne Scheduling-Nachrichten ab. Es wurden keine
Live-Kalenderaenderungen vorgenommen. Automatische Uebernahme einer eingehenden
REQUEST-Aktualisierung aus der Mail wird hierdurch nicht neu implementiert;
Stalwarts vorhandene Scheduling-Verarbeitung und Bulwarks Aktionen bleiben
massgeblich.


### Einladungsaktualisierung und belastbare Teilnahmeantworten

Der Mailabgleich laeuft vor dem Kalenderabgleich. Kalenderdeltas uebernehmen
Termine nur, wenn der Zustand von CalendarEvent/get mit dem newState der
Aenderungsabfrage uebereinstimmt. Ein synthetischer Test prueft Verschiebung,
Serienausschluss, Loeschung, konkurrierende Version und Offline-Neustart.

Ein gepruefter Build-Transform ergaenzt den unveraenderten Bulwark-Banner um
InvitationUpdate. METHOD aus dem ICS hat Vorrang vor heuristischer Erkennung
anhand der Teilnehmer. Der lokale Status kommt ausschliesslich aus dem nativen,
kontogetrennten redb-Snapshot, auch fuer Termine ausserhalb der sichtbaren
Kalenderspanne. Angezeigt werden aktuelle, veraltete, widerspruechliche und
neuere Einladungen. Ein hoeherer SEQUENCE-Wert erlaubt nach ausdruecklichem
Klick das Vormerken der Terminangaben eines Einzeltermins. UID muss eindeutig
sein; Organisator muss mit dem vorhandenen Termin uebereinstimmen. Die
originale Bulwark-Herkunftspruefung muss trusted melden oder einen bestaetigten
abweichenden Absender (sender_mismatch). Fehlende Authentifizierung und
Organisatorwechsel erlauben keine Uebernahme. Serientermine/-instanzen bleiben
in diesem manuellen Uebernahmepfad gesperrt; deren Scheduling sowie die originale
Absageplanung werden weiterhin von Stalwart/Bulwark behandelt.

Der gezielte Patch laesst Teilnehmerantworten, Kalenderzuordnung und persoenliche
Erinnerungen unveraendert. ifInState bindet ihn an die zuvor gelesene lokale
Revision. sendSchedulingMessages=false verhindert eine automatische RSVP.
Die originalen Vorher-/Nachher-Plugin-Hooks werden mit Bulwarks Eventprojektion
benutzt. Native Rechte und Nur-Lese-Modus werden erneut geprueft. Die Anzeige
bezeichnet eine vorgemerkte Aenderung als lokal; Ablehnungen und unklare
Uebertragungen werden im bestehenden Aenderungsdialog behandelt.

RSVP uebernimmt weiterhin Bulwarks originale Aktionen. Ein Offline-RSVP, das
mit einem frueheren stillen Update oder einer lokalen Terminanlage verschmilzt,
behaelt sendSchedulingMessages=true; spaetere stille Bearbeitungen loeschen
diesen Auftrag nicht. Ein Journalauftrag gilt nur bei Bestaetigung genau
seines Objekt-/Erstellungsschluessels und vorhandenem newState als angenommen.
Widerspruechliche oder fremde Bestaetigungen werden uncertain, definitive
Ablehnungen blocked. stateMismatch, fehlende Schreibrechte und geloeschte
Objekte erhalten verstaendliche Meldungen ohne rohe Serverantworten.

Stand: 87 native Tests erfolgreich, zehn bewusst ausgeblendete Live-Tests.
Acht Desktoptests pruefen unter anderem die originale RSVP-Logik fuer Zusage,
Vorbehalt und Absage, Absender-/Organisatorunterschiede, Nur-Lese-Anzeige,
Vormerken einer Verschiebung offline, Konfliktmeldung und Vermeidung einer
Teilnahmeantwort beim reinen Uebernehmen von Terminangaben.
Der neue live_calendar_lifecycle_roundtrip ist vorbereitet, aber nicht gelaufen.
Sein genauer Umfang und die erforderliche separate Freigabe stehen in
calendar-live-validation.md. Echte RSVP-Zustellung wird damit nicht nachgewiesen.


### Live-Kalenderpruefung vom 17. November 2026

Der Benutzer autorisierte bis zu fuenf synthetische Termine und eine Einladung
an mollument@gmail.com. live_calendar_five_event_suite erzeugte genau fuenf
Termine: Einzeltermin, Ganztag, woechentliche Serie, privater Termin mit
Erinnerung sowie eingeladenen Termin. Alle fuenf Create-Antworten wurden bestaetigt.
Beim ersten Replikationsabgleich brach der Lauf ab, bevor die geplanten Updates
fuer Verschiebung, Serienausnahme und Verfuegbarkeit ausgefuehrt wurden.
Die abschliessende Bereinigung bestaetigte fuer alle fuenf ausschliesslich neu
erzeugten UID-/ID-Paare die Loeschung und anschliessendes notFound. Fuer den
eingeladenen Termin wurden sendSchedulingMessages=true bei Anlage und Loeschung
vom Server angenommen. Die externe Gmail-Zustellung wurde nicht nachgewiesen.

Gefundener Kompatibilitaetsfehler: Stalwart liefert leere recurrenceOverrides
nicht als null, sondern laesst die Eigenschaft trotz expliziter Auswahl weg.
Der lokale Responder akzeptiert dies jetzt nach vollstaendiger ID-/State-Pruefung
als null. Fehlende ganze Zeilen, doppelte IDs und ungueltige vorhandene Werte
bleiben Fehler. Ein Regressionstest und ein ausschliesslich lesender erneuter
Live-Abgleich in temporaeres redb bestanden. Der schreibende Lauf wurde nicht
wiederholt; seine dauerhafte Quittung bleibt unveraendert erhalten.
Stand: 88 native Tests erfolgreich, zwoelf ignorierte Live-Tests. Die geplanten
Optionen Verschiebung/Serienausnahme/Verfuegbarkeitswechsel sowie echte RSVP-
Zustellung haben weiterhin keinen vollstaendigen Live-Nachweis.


### Erfolgreicher Offline-/Reconnect-Live-Test mit einem TEST-Termin

Nach zusaetzlicher ausdruecklicher Freigabe wurde genau ein weiterer Termin mit
Titel `TEST Herakles - Offline- und Kalenderoptionen` erzeugt und wieder geloescht.
Der ignorierte Test `live_calendar_offline_options_roundtrip` verwendet die native
Schreibvalidierung, denselben Produktions-Worker wie sync_pending_changes und
eine temporaere redb-Datenbank. Das Gate lautet
JMAP_CALENDAR_TEST=one-test-offline-options; die dauerhafte separate Quittung ist
.cache/calendar-offline-options-attempt.json. Fruehere Quittungen bleiben erhalten.

Alle fuenf Phasen bestanden am 17. November 2026:

1. Offline-Anlage mit sichtbarer TEST-Markierung.
2. Offline-Verschiebung, privat/belegt und Erinnerung 15 Minuten vorher.
3. Woechentliche Serie mit drei Vorkommen und Ausschluss eines Vorkommens.
4. Umstellung auf Ganztag, frei, ohne verbleibende Serienregel.
5. Offline-Loeschung und bestaetigte Entfernung auf dem Server.

In jeder Phase: Netzwerkmodus deaktivieren, Aenderung nativ vormerken, Worker
im Offline-Modus abweisen, redb schliessen und wieder oeffnen, lokale Darstellung
und held-Auftrag pruefen. Beim Wiederverbinden bleiben die Auftraege bis zur
gezielten Freigabe gehalten. Danach uebertraegt der echte Worker den Auftrag;
ein zweiter Worker-Lauf uebertraegt nichts erneut. Anschliessender lesender
Serverabgleich bestaetigt die Optionen und raeumt den bestaetigten Journalauftrag
ab. UID-Abfragen pruefen auf genau einen Termin bzw. nach Loeschung keinen.
Kein Abschalten des Betriebssystemnetzes: geprueft wurde der native Offline-Modus
und die dauerhafte Queue mit Neustart der lokalen redb-Ablage, keine komplette
Neuanmeldung des Desktopfensters. Der Rueckabgleich benutzt eine getrennte strikt
lesende Verbindung als Prueforakel.

Die Produktion wurde nur zur Testbarkeit aufgeteilt: Der IPC-Handler behaelt
Netzwerkpruefung und SyncGate und ruft sync_pending_changes_inner auf. Dieser
Worker erzwingt die Netzwerksperre nochmals und verwendet die explizit uebergebene
native Konfiguration; Zugangsdaten werden weder in JS noch in Testausgaben
gegeben. Gespeicherte Nur-Lese-Einstellungen wurden nicht geaendert. Es wurden
keine Teilnehmer und keine Scheduling-Nachrichten verwendet. Testtermin geloescht,
alle fuenf Phasen und Cleanup bestaetigt. Die externe Zustellung der vorherigen
Einladung/Absage an Gmail bleibt davon unabhaengig unbestaetigt.
Stand: 88 native Standardtests erfolgreich, 13 bewusst ignorierte Live-Tests;
der zusaetzlich explizit gestartete Offline-Live-Test bestand.

# Google-Kontakte nach Stalwart übernehmen

Die erste Ausbaustufe ist ein manuell ausgelöster, einseitiger Import:
Google → lokale Vorschau → **Kontakte übertragen** → Stalwart.
Es gibt keinen automatischen Hintergrundimport,
keine Rückübertragung zu Google und keine automatischen Kontaktlöschungen.

## Benutzung

1. In Herakles Mail **Kontakte → Google-Kontakte übernehmen** öffnen.
2. **Mit Google verbinden** anklicken. Der native Rust-Handler öffnet die
   Google-Anmeldung im Standardbrowser des Betriebssystems. Es gibt keine
   eingebettete Anmeldung und keine Client-ID-/Client-Secret-Felder im Benutzerablauf.
3. Im Browser das Google-Konto wählen und den lesenden Kontaktzugriff erlauben.
   Die App empfängt die Antwort über einen lokalen Loopback-Callback; das
   Browserfenster kann danach geschlossen werden. Die Anmeldung läuft nach
   fünf Minuten ab.
4. **Google-Kontakte abrufen** und ein lokales beschreibbares Stalwart-Adressbuch
   wählen. Die Kontaktliste wird automatisch vorbereitet. Falls kein Ziel angeboten wird, zuerst
   die Stalwart-Kontakte regulär abgleichen.
5. **Kontakte übertragen** anklicken. Neue/geänderte und sicher ungesendete vorbereitete
   Kontakte sind vorausgewählt (höchstens 500 pro Durchgang). Unter **Kontakte prüfen
   und Auswahl ändern** lässt sich die Auswahl anpassen. Dubletten und Konflikte
   bleiben gesperrt. Es werden ausschließlich diese Kontaktaufträge übertragen;
   vorgemerkte Mails und andere Änderungen bleiben unberührt. Offline werden die
   Kontakte dauerhaft vorgemerkt; später denselben Button erneut anklicken.
   Im Nur-Lese-Modus sind Vorschau und
   Google-Abruf möglich, Vormerken und Übertragen nach Stalwart gesperrt.

## Einmalige Einrichtung durch den App-Herausgeber

Die Google-OAuth-Registrierung gehört zu Herakles Mail, nicht zum einzelnen
Benutzer. Ohne registrierten OAuth-Client kann Google keine Anmeldung für
Herakles genehmigen. Für den lokalen Windows-Build wurde am 17. November 2026 ein registrierter
Desktop-Client aus einer lokal ignorierten JSON-Datei eingebunden. Builds ohne
App-Konfiguration zeigen einen deaktivierten Verbindungsbutton und einen klaren
Hinweis. Es wird keine Anmeldung mit Platzhalterkennung geöffnet.

Der Herausgeber muss einmal in einem Google-Cloud-Projekt die People API aktivieren,
den OAuth-Zustimmungsbildschirm einrichten und einen Client vom Typ **Desktop-App**
erstellen. Im Teststatus müssen zugelassene Google-Adressen als Testnutzer
hinterlegt werden. Eine öffentliche Verteilung kann eine Google-Prüfung erfordern;
Testzugänge können erneute Anmeldung erfordern.

Beim nativen Build werden die Prozessvariablen HERAKLES_GOOGLE_CLIENT_ID und
optional HERAKLES_GOOGLE_CLIENT_SECRET übernommen. Cargo verfolgt Änderungen an
beiden Variablen. Sie werden nicht über VITE-Variablen oder IPC an React gegeben.
Die vorhandene Release-CI liest die ID aus der gleichnamigen GitHub-Repositoryvariable
und das Client-Secret aus dem gleichnamigen GitHub-Secret. Ein Build ohne ID bleibt
möglich, bietet aber keine funktionsfähige neue Google-Anmeldung. Bereits gespeicherte
Google-Zugänge behalten ihre zugehörige Client-Konfiguration für Token-Erneuerungen.
Desktop-OAuth-Clients sind öffentliche Clients: Ein mitgeliefertes Client-Secret ist
kein vor Benutzern verborgenes Servergeheimnis; PKCE schützt den Anmeldecode.
Eine Änderung der App-Registrierung benötigt einen neuen nativen Build. Der Benutzer
muss weder ein eigenes Google-Projekt anlegen noch OAuth-Daten eingeben.

Für lokale Entwicklung kann die von Google heruntergeladene Desktop-Client-JSON
unter dem ignorierten Pfad .cache/google-oauth-client.json gespeichert werden.
Die Datei nicht ins Repository aufnehmen und keine Tokens oder Passwörter im Chat
teilen. Der Helfer liest ausschließlich die Client-Konfiguration und gibt deren
Werte nicht aus:

```powershell
npm run tauri:google -- check .cache/google-oauth-client.json
npm run tauri:google -- dev .cache/google-oauth-client.json
```

Für einen nativen Release-Build statt dev den Modus build verwenden. Der Helfer
setzt die beiden nativen Buildvariablen nur für den gestarteten Tauri-Prozess.
Google-OAuth-Endpunkte bleiben in Rust fest vorgegeben; URLs aus der JSON-Datei
werden nicht übernommen. Für dev muss ein bereits laufender Entwicklungsserver
auf Port 1420 zuvor beendet werden.

Google-Grundlagen:
[Desktop-OAuth mit PKCE](https://developers.google.com/identity/protocols/oauth2/native-app),
[People API](https://developers.google.com/people/v1/contacts).

## Umfang und Konflikte

- Unterstützt: Name (Anzeigename), E-Mail-Adressen und Telefonnummern, einschließlich
  verfügbarer Home-/Work-Kontexte und primärer Einträge.
- Noch nicht importiert: Fotos, Gruppen, Anschriften, strukturierte Namensbestandteile,
  Organisationen, Geburtstage, Notizen und sonstige Felder. Vorhandene zusätzliche
  Stalwart-Felder werden bei Updates nicht angefasst. Die Oberfläche nennt diesen
  begrenzten Feldumfang ausdrücklich.
- Es werden nur gespeicherte Google-Kontakte geladen. Automatisch gesammelte
  „Weitere Kontakte“, SIM-Kontakte und nur auf dem Handy gespeicherte Kontakte
  werden nicht geladen. Dafür wäre eine separate Importquelle nötig.
- Jeder Abruf lädt den Google-Bestand vollständig und paginiert neu (maximal
  100.000 Kontakte und 32 MiB Antwortdaten insgesamt). Ein unterbrochener oder
  ungültiger Abruf ersetzt die letzte vollständig gespeicherte Quelle nicht.
- Eine stabile Import-UID aus Google-Kontokennung und Ressourcenkennung verhindert
  erneute Neuanlage derselben bereits zugeordneten Kontakte innerhalb eines
  Stalwart-Zielkontos. Das Zieladressbuch wird bei Folgeimporten nicht automatisch
  gewechselt.
- Mögliche Dubletten anhand Name, E-Mail oder Telefonnummer werden gesperrt, auch
  unter den Google-Quellkontakten. Es findet kein automatisches Zusammenführen
  mit vorhandenen fremden Kontakten statt. Es gibt noch keinen manuellen
  Zuordnungs-/Merge-Editor.
- Bei Folgeimporten werden nur gegenüber der letzten Importbasis geänderte Felder
  betrachtet. Hat Stalwart dasselbe Feld ebenfalls abweichend geändert, wird der
  Kontakt als Konflikt gesperrt. Andere Stalwart-Felder bleiben erhalten.
- Pro Auswahl können bis zu 500 Kontakte vorgemerkt werden. Jeder einzelne Kontakt
  und seine Importbasis werden gemeinsam in einer redb-Transaktion gespeichert.
  Ein Speicherfehler kann eine teilweise vorgemerkte Auswahl hinterlassen; die
  Vorschau wird dann verworfen und die Queue zeigt die tatsächlich gespeicherten
  Aufträge. Ein unverändert erneut vorgemerkter Kontakt erzeugt kein Duplikat.
- Sicher ungesendete, unveränderte Kontaktaufträge können mit demselben Button
  übertragen werden. Unklare oder blockierte Aufträge werden nicht erneut gesendet. Nach Verwerfen
  eines sicher ungesendeten Auftrags wird die vorherige Importbasis atomar
  wiederhergestellt. Bei ausdrücklich lokal abgeschlossenem unklarem Auftrag
  bleibt die Zuordnung erhalten; eine automatische erneute Anlage wird verhindert.
- Ein entfernter Quellkontakt löscht nichts in Stalwart. Fehlt ein früher
  importierter Zielkontakt, wird er nicht automatisch neu angelegt.
- Gesendete Kontaktaufträge bleiben bis zum passenden lesenden UID-/Feldnachweis
  als lokale Überlagerung bestehen. Mehrdeutige Antworten und Netzwerkabbrüche
  nach Übertragungsbeginn werden nicht automatisch erneut gesendet.

## Datenfluss und Zugangsdaten

Der Google-Adapter ist eine explizite Ausnahme zum bisherigen alleinigen
Stalwart-Ziel. Nur `google_auth.rs` spricht als nativer Google-Leseworker mit den
fest vorgegebenen Google-Endpunkten. Die React-Oberfläche liest Quellbestand,
Zielbestand und Vorschau ausschließlich über IPC aus redb. Es gibt keinen
Google-Netzwerkzugriff im Frontend und keinen neuen Proxy-Dienst.

Die Anmeldung nutzt PKCE/S256, einen zufälligen State und den Systembrowser.
Angefordert werden `openid`, `email` und `contacts.readonly`. Refresh-Token und die beim Anmelden benutzte
OAuth-Client-Konfiguration liegen getrennt pro lokalem Kontozugang im OS-Schlüsselbund;
Access-Tokens bleiben im Rust-Prozess. Browsercallback und Antworten werden nicht
protokolliert. Konto-/Profilwechsel und Offline-Modus werden vor Veröffentlichung
erneut geprüft; Quellbestände und Importjournale sind kontogetrennt.

**Google-Verbindung lokal trennen** entfernt den gespeicherten Zugang und die
lokale Google-Quelle. Bereits importierte Stalwart-Kontakte, Importzuordnungen und
vorgemerkte Queue-Aufträge bleiben erhalten. Die Google-seitige OAuth-Freigabe wird
dadurch nicht widerrufen; das ist über die Google-Kontoeinstellungen möglich.

Stalwart-Schreibzugriffe laufen ausschließlich durch den bestehenden Rust-Worker.
Der Queue-Auftrag ist immer zunächst `held`, auch online. Der Übertragungsbutton
gibt ausschließlich die ausgewählten Kontaktaufträge frei. Der Worker prüft erneut
Nur-Lese-Modus, Kontozugehörigkeit, Fähigkeit und Adressbuchrechte und verwendet
`ifInState`. Vor-/Nachher-Hooks verwenden Bulwarks originale Kontakt-Hooks und
Kontaktprojektion. Allgemeine Kontaktbearbeitung, `ContactCard/set` über UI-/
Plugin-JMAP und Adressbuchverwaltung bleiben gesperrt.

## Verifikation und offene Live-Prüfung

Synthetische Rust-Tests prüfen PKCE/Callback-Zuordnung, Seitenfehler, Offline-Abbruch,
Kontoisolation, atomare Importbasis/Queue, Neustart, Dubletten, Drei-Wege-Konflikte,
Verwerfen und unklare Creates ohne Wiederholung. Browsertests prüfen Vorschau,
Auswahl, Konfliktsperre, Nur-Lese-Modus und Vormerken über die IPC-Grenze.

Noch kein Live-Nachweis: Google-OAuth mit eigenen Client-Daten, echter People-Abruf
und bestätigtes Stalwart-Kontaktschreiben. Keine bisherigen Live-Quittungen wurden
verändert und keine Live-Schreibtests für Kontakte ausgeführt. Ein echter Testlauf
muss gesondert begrenzt werden; normale Benutzeraktionen im Importdialog bleiben
an Auswahl, Schreibmodus und den ausdrücklichen Übertragungsbutton gebunden.

Die aktuelle Prüfung umfasst 98 erfolgreiche native Standardtests (13 ignorierte
Live-Tests), 13 erfolgreiche Desktop-Browsertests, erfolgreichen Frontend- und
nativen Debug-Build sowie den JMAP-Audit. Der Einrichtungsdialog wurde zusätzlich
im echten Windows-Desktopfenster mit aktivem HERAKLES_FORCE_READ_ONLY geöffnet;
lokale Zieladressbücher und die Nur-Lese-Sperre wurden angezeigt. Dabei wurde
keine Google-Anmeldung durchgeführt und kein Kontaktauftrag angelegt.

Der konfigurierte Windows-Release-Build vom 17. November wurde erfolgreich erstellt
(MSI und NSIS), die lokale startbare EXE aktualisiert und geöffnet. Die vorherige
EXE wurde im ignorierten Cache gesichert. Google-Anmeldung, Zustimmung und echter
People-Abruf wurden dabei nicht automatisiert oder als erfolgreich behauptet.

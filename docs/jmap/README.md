# JMAP-Abgleich pflegen

[matrix.md](matrix.md) ist die lesbare Bestandsaufnahme; [inventory.json](inventory.json)
enthaelt alle Fundstellen und Argumentausdruecke. [review.json](review.json) enthaelt
die bewertete Unterstuetzung und [stalwart-reference.json](stalwart-reference.json)
die gepinnte Stalwart-Revision mit Dateipfaden und Git-Blob-IDs.

## Umfang

Der Generator parst alle JS/TS-Dateien im unveraenderten Bulwark-Submodul mit einem
TypeScript/JSX-faehigen AST-Parser. Tests, Fixtures, node_modules und Buildausgaben
werden ausgeschlossen. Webserverrouten, Entwicklungsmock und Werkzeuge werden
bewusst mit erfasst und an jeder Fundstelle als solche gekennzeichnet.
Kommentare werden nicht als Aufrufe gezaehlt. Referenzliterale und Aufruftupel
sind getrennt. Lokale Konstanten sowie endliche Typunionen fuer Methodennamen
werden aufgeloest. properties-, filter-, sort- und Ergebnisreferenzfelder enthalten
ihren Quellausdruck; dynamische Werte/Spreads bleiben ausdruecklich dynamisch.
HTTP fetch, EventSource, WebSocket, XMLHttpRequest und sendBeacon werden gesondert
erfasst. Gepruefte dynamische Hilfsfunktionen stehen in review.json.

Das ist keine vollstaendige Programmanalyse: Importaliase, beliebiger Kontrollfluss,
Reflection, zur Laufzeit nachgeladene Plugins oder fremde Methoden koennen statisch
nicht vollstaendig bestimmt werden. Die Matrix belegt keine Laufzeiterreichbarkeit,
keine Aktivierung von Stalwart-Funktionen und keine Gleichheit aller Argumente.
Ein Ergebnisreferenz-Literal kann auch in einer Antwortpruefung vorkommen.
Die benoetigten Bulwark-Funktionen laufen zudem teilweise nur im Webserver.
Eine vollstaendige statische Bestandsaufnahme ist keine vollstaendige Clientimplementierung.

## Aktualisierung

1. `npm run audit:jmap` ausfuehren. Neue Methoden und dynamische Kontexte brauchen
   zuerst eine explizite Bewertung in review.json; der Generator bricht sonst ab.
2. Pro Methode den tatsaechlichen Desktoppfad pruefen: bootstrap.ts, TS-Adapter,
   native IPC-Befehle, redb-Responder und Worker. Einen Methodenstring oder ein
   Transport-Allowlisting niemals als Implementierungsnachweis werten.
3. Stalwart-Quellpfade an der festgehaltenen Revision pruefen. Fuer ein bewusstes
   Referenzupdate die Git-Dateipfade und Blob-IDs aus dem GitHub-Tree dieser Revision
   erneuern. Der normale Generator greift nicht auf das Netzwerk zu.
4. Lokale Belege mit file und anchor, Tests zusaetzlich mit name, eintragen.
   Der Generator loest Anker zu Zeilen auf und prueft ihre Existenz. Testbelege
   koennen auch nur einen Worker oder eine Ablehnung pruefen; Grenzen explizit nennen.
5. `npm run test:jmap-audit` und `npm run check:jmap` ausfuehren. Matrix und JSON
   zusammen mit review.json versionieren. Die Releasepruefung fuehrt beide Checks aus.

SHA-256-Werte der beteiligten lokalen Quellen und Tests werden mit normalisierten
Zeilenenden in inventory.json gespeichert. Aenderungen erfordern eine erneute
Generierung; diese ersetzt keine erneute fachliche Bewertung. Upstream-Commit,
Stalwart-Commit und lokale Quellbelege bleiben dadurch nachvollziehbar.

## Aktuell wichtigste Befunde

- Email/get besitzt einen Standardprojektor fuer Eigenschaften, Header und MIME-Optionen;
  Textteile ueber 8 MB bleiben abgeschnitten. Lokale Suche und Datumsvergleiche
  entsprechen weiterhin nicht Stalwarts Suchindex.
- Mailbox/query ist stark eingeschraenkt; Mailbox/set fehlt.
- Kalender/Dateien werden teilweise in TypeScript aus nativen Snapshots beantwortet.
  Gemischte Batches und Ergebnisreferenzen sind nicht ueberall gleich implementiert.
- Identity/get und Submission/get/query besitzen Sonderfaelle; leere Submission-
  Ersatzantworten sind keine Submission-Replik.
- Kontakt-/Adressbuchaenderungen, viele Freigabe-/Benachrichtigungs-, Sieve-, Quota-
  und Selfservice/Adminmethoden fehlen. Native Blob- und SSE-Pfade ersetzen nicht
  die gleichnamigen JMAP-Methoden.

Die Matrix priorisiert diese Luecken. Dieser Abgleich fuehrt keine Serveranfragen
oder Live-Schreibtests aus und erweitert keine bestehenden Schreibfreigaben.

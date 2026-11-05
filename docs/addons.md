# Bulwark-Add-ons im Desktop-Client

## Verwendung

1. Öffne Einstellungen → Erweiterungen → Plugins.
2. Wähle „Plugin installieren“ und eine normale Bulwark-Plugin-ZIP.
3. Prüfe die angefragten Berechtigungen und schalte das Plugin ein.
4. Plugin-Einstellungen und Ein/Aus-Status bleiben lokal erhalten.

Der ursprüngliche Bulwark-Paketvalidator prüft Manifest, Paketstruktur und Code.
Die originalen Berechtigungsdialoge und die Integritätsprüfung bleiben aktiv.
Eine Installation umgeht keine Server-Schreibregeln: Der native Nur-Lese-Schutz gilt
auch für Mailaktionen aus Plugins.

## Implementiert und geprüft

Die originalen Settings- und Plugin-Komponenten, ZIP-Installation, Aktivierung,
Sandbox-Runtime und Host-RPC sind eingebunden. Ein synthetisches Test-Plugin wird
installiert, aktiviert, schreibt über `api.storage` und wird nach dem Neuladen
wieder geladen. Der Browsertest bildet die native IPC-Grenze nach; die tatsächliche
native Protokoll-Ausführung ist noch nicht vollständig über alle Plattformen geprüft.

JS-Pakete, Themes, Vorschauen und zwischengespeicherte Plugin-Dateien liegen über
`src/desktop/plugin-storage.ts` in redb. Bulwarks synchron erwartete UI-Speicherung
wird beim Start aus redb geladen; Änderungen werden geordnet zurückgeschrieben.
Die ursprüngliche kontobezogene Namensgebung für Plugin-Daten bleibt erhalten.
Beim nativen Schließen werden ausstehende UI-Schreibvorgänge abgewartet.

## Sandbox

Untrusted Plugins laufen mit `sandbox="allow-scripts"` in einem Frame mit opaker
Herkunft. Das gilt auch während der Desktop-Entwicklung. Nur die originale
Host-RPC-Schnittstelle verbindet sie mit der Anwendung.

Das native Protokoll `bulwark-plugin` liefert ausschließlich die Sandbox und deren
öffentliche JS-/CSS-Assets. Es stellt keine Maildaten oder IPC-Berechtigungen bereit.
Die Sandbox erhält ihre eigene CSP mit der für Bulwarks Bundle-Format benötigten
Codeauswertung. Die Hauptanwendung erlaubt weiterhin kein `unsafe-eval`.

## Grenzen

- Privilegierte Plugins werden ausdrücklich abgelehnt; deren Signatur-, Admin- und Kryptografie-Anbindung fehlt noch.
- Server-Proxys, verwaltete Server-Plugins und Credential-Vaults sind noch nicht portiert.
- Externe HTTP-Dienste und eingebettete externe Sidebar-Seiten sind derzeit nicht freigegeben.
- Mail-Lese-APIs funktionieren nur, soweit der lokale JMAP-Adapter sie unterstützt. Im ausdrücklich aktivierten Test-Schreibmodus sind ausschließlich Gelesen-/Markiert-Updates über die lokale Warteschlange erlaubt; andere Mail-Schreibaktionen bleiben gesperrt.
- Kalender-, Kontakt-, Datei- und Versand-APIs benötigen weitere native Datenadapter.

Daher ist keine pauschale Kompatibilität mit allen vorhandenen Bulwark-Plugins
zugesagt. Die ursprünglichen Erweiterungspunkte bleiben die Grundlage; es wird kein
zweites, inkompatibles Add-on-Format eingeführt.


Blob-Downloads nutzen inzwischen den nativen redb-Cache auch fuer die originalen
Mail-Anhangsfunktionen. Uploads und Composer-Anhaenge bleiben offen; Limits und
Offline-Verhalten stehen in docs/architecture.md.

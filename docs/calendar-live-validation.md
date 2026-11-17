# Vorbereitete Live-Kalenderpruefung

Der Test ist implementiert in `src-tauri/src/calendar_roundtrip.rs`, standardmaessig
ignoriert und bislang nicht ausgefuehrt. Er erfordert eine gesonderte ausdrueckliche
Freigabe gemaess AGENTS.md; die bisherige Flag-/Blobfreigabe reicht nicht.

## Genau begrenzter naechster Lauf

- Explizit gewaehltes vorhandenes Profil und Login, persoenliches Primaerkonto,
  genau ein beschreibbarer Standardkalender.
- Ein neuer synthetischer Einzeltermin `Herakles synthetic calendar test`,
  15. Januar 2030, 10:00 UTC, 30 Minuten. Eindeutige eigens erzeugte UID.
- Eine Verschiebung ausschliesslich dieses neuen Termins auf 11:00 UTC.
- Lesender Rueckabgleich, temporaeres redb, Neustart und Offline-Verifikation.
- Ein Loeschversuch ausschliesslich fuer den bestaetigten Testtermin; vor dem
  Loeschen muss seine UID erneut uebereinstimmen. Anschliessend lesender Nachweis.
- Keine Teilnehmer, keine Mails und ueberall sendSchedulingMessages=false.
- Keine Bearbeitung bestehender Termine, keine Aenderung gespeicherter
  Nur-Lese-Einstellungen, keine automatische Wiederholung.

Vor der ersten Mutation wird `.cache/calendar-live-lifecycle-attempt.json`
exklusiv und dauerhaft angelegt. Eine vorhandene Quittung sperrt weitere Laeufe.
Fehler loeschen diese Quittung nicht. Unklarer Create-Status wird nicht wiederholt.
Ist die Identitaet des Testtermins nicht mehr bestaetigbar, erfolgt keine
Loeschung und die Quittung fordert manuelle Pruefung. Eine echte Teilnahmereaktion
mit Mailzustellung wird von diesem Test nicht abgedeckt; dafuer ist danach eine
separat autorisierte Organisator-/Teilnehmer-Testkonstellation erforderlich.

## Ausfuehrung erst nach Freigabe

Die Gate-Variable ist `JMAP_CALENDAR_TEST=one-synthetic-event`; Profil und Login
werden mit `JMAP_TEST_PROFILE` und `JMAP_TEST_ACCOUNT` explizit ausgewaehlt.
Der einzige auszuwaehlende ignorierte Test heisst
`calendar_roundtrip::live_calendar_lifecycle_roundtrip`. Standardtests setzen
das Gate nicht und fuehren ihn nicht aus. Es werden weder Zugangsdaten noch
Kalenderinhalte in die Testausgabe geschrieben.


## Autorisierte Fuenf-Termine-Suite und Ergebnis

Der Benutzer erweiterte die Freigabe auf fuenf synthetische Termine und Einladung
an mollument@gmail.com. Der separate ignorierte Test heisst
`calendar_roundtrip::live_calendar_five_event_suite`, Gate
`JMAP_CALENDAR_TEST=five-events-mollument`. Seine dauerhafte Quittung ist
`.cache/calendar-five-event-attempt.json`; sie darf nicht entfernt oder fuer
einen erneuten Lauf geleert werden.

Am 17. November 2026: fuenf Anlagen bestaetigt, Einladung angefordert,
Replikationsabgleich wegen ausgelassener optionaler recurrenceOverrides-Felder
abgebrochen, danach alle fuenf Testtermine UID-geprueft geloescht und Loeschung
lesend bestaetigt. Fuer den eingeladenen Termin wurde auch die Absage angefordert.
Keine automatische Wiederholung. Geplante Update- und Offline-Optionspruefungen
wurden wegen des Abbruchs nicht erreicht; externe Mailzustellung ist unbestaetigt.
Der Kompatibilitaetsfehler wurde anschliessend behoben und mit einem rein lesenden
Live-Abgleich bestaetigt. Die urspruengliche fehlgeschlagene Suite-Quittung bleibt
als Nachweis erhalten; kein weiterer Termin wurde dafuer erzeugt.


## Erfolgreicher Zusatzlauf: ein TEST-Termin, Offline und Reconnect

Nach gesonderter Freigabe wurde am 17. November 2026 genau ein weiterer Termin
`TEST Herakles - Offline- und Kalenderoptionen` verwendet, ohne Teilnehmer/Mails.
Der neue Test `calendar_roundtrip::live_calendar_offline_options_roundtrip`
(Gate `one-test-offline-options`) bestand vollstaendig. Seine separate Quittung
`.cache/calendar-offline-options-attempt.json` meldet fuenf bestandene Phasen
und cleanupVerified=true; sie sperrt einen erneuten Lauf.

Offline-Anlage, Verschiebung mit Privatstatus/Erinnerung, Wochenserie mit Ausnahme,
Ganztagsumstellung und Loeschung wurden jeweils durch die native Schreibgrenze
vorgemerkt, nach redb-Neustart lokal geprueft und erst nach Reconnect und gezielter
Freigabe durch den echten Synchronisierungsworker uebertragen. Jeweils kein
zweites Senden im naechsten Worker-Lauf, anschliessend lesender Servernachweis.
Der Testtermin ist geloescht. Betriebssystemnetz und laufendes Desktopfenster
wurden dabei nicht beendet; getestet wurde der native Offline-Modus mit lokaler
redb-Wiedereroeffnung. Echte externe RSVP-Zustellung ist ein separater Nachweis.


## Desktop-Nachweis nach App-Freigabe

Am 17. November 2026 wurde der aktuelle Entwicklungsbuild von b69f3f1
(natives Debug-Binary mit Vite auf Port 1420) im echten Windows-Fenster mit
HERAKLES_FORCE_READ_ONLY=1 geprüft. Nach Öffnen des Kalenders und Umschalten
auf den nativen Offline-Modus blieb ein bereits gespeicherter Termin sichtbar.
Nach vollständigem Schließen und erneutem Starten desselben Builds blieben
Offline-Modus und Kalenderdaten erhalten. Reconnect endete mit Online / Verbunden
und Abgleich abgeschlossen. Die Anzeige blieb Nur Lesen; es gab vor und nach
dem Test keine ausstehenden Aktionen. Keine Termine wurden verändert, keine
Testobjekte angelegt und keine früheren Live-Quittungen zurückgesetzt.

Dieser Nachweis ergänzt den vorherigen Worker-/redb-Test um den echten
Fensterneustart und lesenden Reconnect. Er weist keine Freigabe eines gehaltenen
Schreibauftrags im Fenster und keine externe RSVP-Zustellung nach. Die zuvor
geöffnete Root-EXE war noch vom 13. November; der maßgebliche Nachweis wurde
deshalb mit src-tauri/target/debug/herakles-mail.exe wiederholt.

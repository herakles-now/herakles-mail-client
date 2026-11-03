import { useEffect, useRef, useState } from 'react';
import { mailRepository } from './mail-repository';
import type { Email, LocalMail } from './mail-repository';

export function LocalMailbox({ accountId }: { accountId: string }) {
  const [mail, setMail] = useState<LocalMail | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [status, setStatus] = useState('Lokale Nachrichten werden geladen …');
  const [busy, setBusy] = useState(false);
  const [readOnly, setReadOnly] = useState(true);
  const syncing = useRef(false);
  const mounted = useRef(false);

  async function refreshLocal() {
    const result = await mailRepository.read(accountId);
    if (mounted.current) setMail(result);
  }

  async function synchronize() {
    if (syncing.current) return;
    syncing.current = true;
    setBusy(true);
    setStatus('Abgleich mit Stalwart läuft …');
    try {
      await mailRepository.synchronize(accountId);
      if (mounted.current) setStatus('Lokaler Datenbestand ist abgeglichen.');
    } catch (error) {
      if (mounted.current) setStatus(typeof error === 'string' ? error : 'Abgleich fehlgeschlagen. Lokale Daten bleiben verfügbar.');
    } finally {
      try { await refreshLocal(); } catch {
        if (mounted.current) setStatus('Lokaler Datenbestand konnte nicht gelesen werden.');
      }
      syncing.current = false;
      if (mounted.current) setBusy(false);
    }
  }

  useEffect(() => {
    mounted.current = true;
    mailRepository.isReadOnly()
      .then((value) => { if (mounted.current) setReadOnly(value); })
      .catch(() => { if (mounted.current) setReadOnly(true); });
    // Display the replica before starting any server request.
    refreshLocal().then(() => { if (mounted.current) void synchronize(); }).catch(() => {
      if (mounted.current) setStatus('Lokale Nachrichten konnten nicht geladen werden.');
    });
    const online = () => { void synchronize(); };
    window.addEventListener('online', online);
    const timer = window.setInterval(online, 60_000);
    return () => {
      mounted.current = false;
      window.removeEventListener('online', online);
      window.clearInterval(timer);
    };
  }, [accountId]);

  async function toggle(email: Email, keyword: '$seen' | '$flagged') {
    try {
      await mailRepository.setKeyword(accountId, email.id, keyword, !email.keywords[keyword]);
      await refreshLocal();
      if (mounted.current) setStatus('Änderung lokal gespeichert; für den Serverabgleich vorgemerkt.');
      void synchronize();
    } catch (error) {
      if (mounted.current) setStatus(typeof error === 'string' ? error : 'Änderung konnte nicht lokal gespeichert werden.');
    }
  }

  const selected = mail?.emails.find((email) => email.id === selectedId);
  return <section className="mail-section" aria-labelledby="mail-heading">
    <div className="mail-toolbar"><h2 id="mail-heading">Lokales Postfach</h2>
      <button onClick={() => { void synchronize(); }} disabled={busy}>{busy ? 'Abgleich …' : 'Jetzt synchronisieren'}</button>
    </div>
    <p role="status">{status}</p>
    {readOnly && <p className="notice">Nur-Lese-Modus: Änderungen am Server sind technisch gesperrt.</p>}
    <p>{mail?.emails.length ?? 0} Nachrichten · {mail?.pending.length ?? 0} ausstehende Änderungen</p>
    {mail?.pending.some((operation) => operation.blocked) && <div className="notice" role="alert">
      <p>Einige Änderungen wurden vom Server dauerhaft abgelehnt und bleiben zur Klärung gespeichert:</p>
      <ul>{mail.pending.filter((operation) => operation.blocked).map((operation) =>
        <li key={operation.sequence}>{operation.emailId}: {operation.keyword} ({operation.blocked})</li>)}</ul>
    </div>}
    {mail && !mail.initialized && <p>Für dieses Konto liegt noch kein vollständiger Erstabgleich vor.</p>}
    <div className="mail-layout">
      <ul className="message-list" aria-label="Lokal gespeicherte Nachrichten">{mail?.emails.map((email) =>
        <li key={email.id}><button className={`message-row ${email.keywords.$seen ? '' : 'unread'}`}
          aria-pressed={selectedId === email.id} onClick={() => setSelectedId(email.id)}>
          <span>{email.from?.map((from) => from.name || from.email).join(', ') || 'Unbekannter Absender'}</span>
          <strong>{email.keywords.$flagged ? '★ ' : ''}{email.subject || '(Kein Betreff)'}</strong>
          <span>{email.preview}</span>
        </button></li>)}</ul>
      <article className="message-body">{selected ? <>
        <h3>{selected.subject || '(Kein Betreff)'}</h3>
        <div className="message-actions">
          <button disabled={readOnly} onClick={() => { void toggle(selected, '$seen'); }}>{selected.keywords.$seen ? 'Als ungelesen' : 'Als gelesen'}</button>
          <button disabled={readOnly} onClick={() => { void toggle(selected, '$flagged'); }}>{selected.keywords.$flagged ? 'Markierung entfernen' : 'Markieren'}</button>
        </div>
        {selected.textBody?.length ? selected.textBody.map((part) => <div key={part.partId}>
          <pre>{selected.bodyValues?.[part.partId]?.value || 'Dieser Nachrichtenteil ist lokal nicht verfügbar.'}</pre>
          {selected.bodyValues?.[part.partId]?.isTruncated && <p>Dieser Nachrichtenteil wurde nur teilweise synchronisiert.</p>}
        </div>) : <p>{selected.preview || 'Keine lokale Textansicht vorhanden.'}</p>}
      </> : <p>Wähle eine Nachricht aus dem lokalen Speicher.</p>}</article>
    </div>
  </section>;
}

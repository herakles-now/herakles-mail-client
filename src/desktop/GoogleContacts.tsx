import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { refreshLocalViews } from './bootstrap';
import { allowGoogleContact, notifyGoogleContacts } from './google-contact-hooks';
import type { ContactCard } from '@/lib/jmap/types';

type Book = { accountId: string; id: string; name: string; accountName: string };
type Status = { oauthConfigured: boolean; connected: boolean; email: string; fetchedAt: number; count: number; books: Book[]; readOnly: boolean };
type Row = { resource: string; title: string; status: string; reason: string; current: Record<string, unknown> | null; incoming: Record<string, unknown>; targetId: string | null; uid: string; patch: Record<string, unknown> };
type Preview = { revision: string; rows: Row[]; readOnly: boolean };
const labels: Record<string, string> = { new: 'Neu', update: 'Geändert', duplicate: 'Mögliche Dublette', conflict: 'Prüfung nötig', pending: 'Bereit zum Übertragen', transferred: 'Übertragen', sending: 'Übertragung läuft', review: 'Prüfung nötig', unsupported: 'Keine unterstützten Felder', unchanged: 'Unverändert' };
function values(value: unknown): string {
  if (value == null) return '—';
  if (typeof value !== 'object') return String(value);
  if (Array.isArray(value)) return value.map(values).join(', ');
  const item = value as Record<string, unknown>;
  if (typeof item.full === 'string') return item.full || '—';
  if (typeof item.address === 'string') return item.address;
  if (typeof item.number === 'string') return item.number;
  return Object.values(item).map(values).join(', ') || '—';
}
export function GoogleContacts() {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [target, setTarget] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<Row | null>(null);
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [filter, setFilter] = useState('');
  const dialog = useRef<HTMLDialogElement>(null);
  const book = status?.books.find((b) => JSON.stringify([b.accountId, b.id]) === target);
  const reload = async () => {
    const result = await invoke<Status>('google_import_status');
    setStatus(result);
    setTarget((previous) => result.books.some((b) => JSON.stringify([b.accountId, b.id]) === previous) ? previous : result.books[0] ? JSON.stringify([result.books[0].accountId, result.books[0].id]) : '');
    return result;
  };
  useEffect(() => {
    if (!open) return;
    dialog.current?.showModal();
    void reload().then(async (result) => {
      const chosen = result.books.find((b) => JSON.stringify([b.accountId, b.id]) === target) ?? result.books[0];
      if (result.fetchedAt && chosen) await showPreview(chosen);
    }).catch((reason) => setError(String(reason)));
  }, [open]);
  const action = async (label: string, task: () => Promise<void>) => {
    if (busy) return;
    setBusy(label); setError(''); setNotice('');
    try { await task(); } catch (reason) { setError(String(reason)); } finally { setBusy(''); }
  };
  const showPreview = async (chosen: Book) => {
    setPreview(null); setSelected(new Set()); setDetail(null);
    const result = await invoke<Preview>('google_import_preview', { accountId: chosen.accountId, bookId: chosen.id });
    setPreview(result);
    setSelected(new Set(result.rows.filter((row) => ['new', 'update', 'pending'].includes(row.status)).slice(0, 500).map((row) => row.resource)));
  };
  const compare = async () => { if (book) await showPreview(book); };
  const close = () => { if (!busy) { dialog.current?.close(); setOpen(false); setPreview(null); setSelected(new Set()); setDetail(null); setError(''); setNotice(''); } };
  const available = preview?.rows.filter((r) => ['new', 'update', 'pending'].includes(r.status)) ?? [];
  const rows = preview?.rows.filter((r) => `${r.title} ${values(r.incoming)}`.toLocaleLowerCase().includes(filter.toLocaleLowerCase())) ?? [];
  return <>
    <button type="button" onClick={() => setOpen(true)}>Google-Kontakte übernehmen</button>
    {open && createPortal(<dialog ref={dialog} className="desktop-google-import" aria-labelledby="google-import-title" onCancel={(event) => { event.preventDefault(); close(); }}>
      <header><h2 id="google-import-title">Google-Kontakte übernehmen</h2><button type="button" disabled={!!busy} onClick={close} aria-label="Google-Import schließen">Schließen</button></header>
      <p>Übertrage deine Google-Kontakte nach Stalwart. Mit „Kontakte übertragen“ bestätigst du die ausgewählten Kontakte direkt hier. Google wird dabei nur gelesen. Es gibt keine automatischen Löschungen.</p>
      <p>Übernommen werden Name, E-Mail-Adressen und Telefonnummern. Fotos, Gruppen, Anschriften und weitere Felder werden noch nicht importiert.</p>
      {!status && !error && <p>Lade lokale Importdaten …</p>}
      {status && <>
        <section aria-label="Google-Verbindung">
          {status.connected ? <><p>Google verbunden: {status.email || 'Konto eingerichtet'}</p><button type="button" disabled={!!busy} onClick={() => void action('Kontakte abrufen …', async () => { setPreview(null); setSelected(new Set()); await invoke('google_fetch_contacts'); const result = await reload(); const chosen = result.books.find((b) => JSON.stringify([b.accountId, b.id]) === target) ?? result.books[0]; if (chosen) await showPreview(chosen); })}>Google-Kontakte abrufen</button><button type="button" disabled={!!busy} onClick={() => void action('Verbindung trennen …', async () => { await invoke('google_disconnect'); setPreview(null); setSelected(new Set()); await reload(); })}>Google-Verbindung lokal trennen</button></> : <>
            <p>Verbinde dein Google-Konto und erlaube Herakles Mail, deine Kontakte zu lesen. Die Anmeldung erfolgt bei Google im Browser.</p>
            <button type="button" disabled={!!busy || !status.oauthConfigured} onClick={() => void action('Anmeldung im Browser abschließen …', async () => { await invoke('google_connect'); await reload(); })}>Mit Google verbinden</button>
            {!status.oauthConfigured && <p>Die Google-Anmeldung ist in dieser App-Version noch nicht eingerichtet.</p>}
          </>}
          {!!status.fetchedAt && <p>Letzter Abruf: {new Date(status.fetchedAt * 1000).toLocaleString('de-DE')} · {status.count} Kontakte. Abruf erfolgt nur auf Knopfdruck.</p>}
        </section>
        <section aria-label="Ziel und Vorschau">
          <label>Zieladressbuch<select value={target} disabled={!!busy} onChange={(e) => { setTarget(e.target.value); setPreview(null); setSelected(new Set()); setDetail(null); const chosen = status.books.find((b) => JSON.stringify([b.accountId, b.id]) === e.target.value); if (chosen && status.fetchedAt) void action('Kontakte laden …', () => showPreview(chosen)); }}><option value="">Bitte wählen</option>{status.books.map((b) => <option key={JSON.stringify([b.accountId, b.id])} value={JSON.stringify([b.accountId, b.id])}>{b.accountName} · {b.name}</option>)}</select></label>
          {!status.books.length && <p>Kein beschreibbares Adressbuch lokal verfügbar. Bitte Stalwart-Kontakte abgleichen und die Kontorechte prüfen.</p>}
          <button type="button" disabled={!!busy || !book || !status.fetchedAt} onClick={() => void action('Vorschau erstellen …', compare)}>Kontaktliste aktualisieren</button>
          {status.readOnly && <p>Nur-Lese-Modus: Vorschau ist möglich, Übernahme nach Stalwart ist gesperrt.</p>}
        </section>
      </>}
      {preview && <section aria-label="Kontaktvorschau">
        <p>{preview.rows.length} Kontakte · {available.length} übernehmbar. Mögliche Dubletten und Konflikte werden übersprungen und müssen manuell geprüft werden.</p>
        <details><summary>Kontakte prüfen und Auswahl ändern</summary>
        <label>Vorschau durchsuchen<input value={filter} onChange={(e) => setFilter(e.target.value)} /></label>
        <button type="button" disabled={!!busy || preview.readOnly} onClick={() => setSelected(new Set(available.slice(0, 500).map((r) => r.resource)))}>Bis zu 500 übernehmbare Kontakte auswählen</button>
        <button type="button" disabled={!!busy} onClick={() => setSelected(new Set())}>Auswahl aufheben</button>
        <div className="desktop-google-rows"><table><thead><tr><th>Auswahl</th><th>Name</th><th>Status</th><th>Prüfen</th></tr></thead><tbody>{rows.map((row) => <tr key={row.resource}><td><input type="checkbox" aria-label={`${row.title} übernehmen`} checked={selected.has(row.resource)} disabled={!!busy || preview.readOnly || !['new', 'update', 'pending'].includes(row.status)} onChange={(e) => setSelected((old) => { const next = new Set(old); if (e.target.checked) next.add(row.resource); else next.delete(row.resource); return next; })} /></td><td>{row.title}</td><td>{labels[row.status] ?? row.status}</td><td><button type="button" onClick={() => setDetail(row)}>Vergleichen</button></td></tr>)}</tbody></table></div>
        {detail && <section aria-label={`Vergleich ${detail.title}`}><h3>{detail.title}</h3><p>{detail.reason}</p><table><thead><tr><th>Feld</th><th>Stalwart</th><th>Google</th></tr></thead><tbody>{(['name', 'emails', 'phones'] as const).map((key, index) => <tr key={key}><th>{['Name', 'E-Mail', 'Telefon'][index]}</th><td>{values(detail.current?.[key])}</td><td>{values(detail.incoming[key])}</td></tr>)}</tbody></table></section>}
        </details>
        <button type="button" disabled={!!busy || preview.readOnly || !book || selected.size === 0 || selected.size > 500} onClick={() => void action('Kontakte werden übertragen …', async () => {
          if (!book) return;
          try {
            const approved: Row[] = [];
            for (const row of preview.rows.filter((row) => selected.has(row.resource))) {
              if (row.status === 'pending' || await allowGoogleContact(row, book.id)) approved.push(row);
            }
            if (!approved.length) { setNotice('Die Übernahme wurde durch Erweiterungen abgebrochen.'); return; }
            const result = await invoke<{ transferred: number; waiting: number; review: number; warning: string | null; readbackPending: boolean; contacts: ContactCard[] }>('google_import_transfer', { accountId: book.accountId, bookId: book.id, revision: preview.revision, resources: approved.map((row) => row.resource) });
            await notifyGoogleContacts(result.contacts.filter((contact) => approved.some((row) => row.status !== 'pending' && row.uid === contact.uid)), new Set(approved.filter((row) => row.status === 'new').map((row) => row.uid)));
            setNotice([`${result.transferred} Kontakte übertragen.`, result.waiting ? `${result.waiting} Kontakte bleiben lokal. Klicke bei bestehender Verbindung erneut auf „Kontakte übertragen“.` : '', result.review ? `${result.review} Kontakte müssen zuerst geprüft werden; sie werden nicht automatisch erneut übertragen.` : '', result.readbackPending ? 'Die lokale Ansicht wird beim nächsten Abgleich bestätigt.' : '', result.warning ?? ''].filter(Boolean).join(' '));
          } finally {
            setSelected(new Set()); setPreview(null); setDetail(null);
            window.dispatchEvent(new Event('desktop-changes-updated'));
            await refreshLocalViews(); await reload(); await compare();
          }
        })}>Kontakte übertragen ({selected.size})</button>
      </section>}
      {busy && <p role="status">{busy}</p>}{notice && <p role="status">{notice}</p>}{error && <p role="alert">{error}</p>}
    </dialog>, document.body)}
  </>;
}

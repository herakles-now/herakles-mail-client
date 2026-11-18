import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { useAuthStore } from '@/stores/auth-store';
import { useEmailStore } from '@/stores/email-store';
import { useCalendarStore } from '@/stores/calendar-store';
import { useContactStore } from '@/stores/contact-store';
import { buildContactsPath } from '@/lib/deep-links';
import { navigate } from './navigation';
import { refreshLocalViews } from './bootstrap';

interface Change {
  accountId: string;
  sequence?: number;
  journalId?: number;
  objectId: string;
  kind: 'mail' | 'draft' | 'send' | 'calendar' | 'file' | 'contact';
  title: string;
  action: string;
  requiresReview: boolean;
  status?: string;
  blocked?: string;
  transferable: boolean;
}
const key = (row: Change) => JSON.stringify([row.accountId, row.journalId != null ? `journal:${row.journalId}` : row.sequence ?? row.objectId]);
const selection = ({ accountId, sequence, journalId }: Change) => ({ accountId, sequence, journalId });
interface ChangeDetails {
  title: string;
  revision: string;
  fields: { label: string; current: unknown; desired: unknown }[];
  warning: string;
  canRetry: boolean;
  canDiscard: boolean;
  uncertain: boolean;
}
function displayValue(value: unknown): string {
  if (value == null) return 'Nicht vorhanden';
  if (typeof value === 'boolean') return value ? 'Ja' : 'Nein';
  if (Array.isArray(value)) return value.length ? value.map(displayValue).join(', ') : 'Keine';
  if (typeof value === 'object') {
    const item = value as Record<string, unknown>;
    if (typeof item.name === 'string') return item.name;
    if (typeof item.email === 'string') return item.email;
    return Object.entries(item).filter(([, enabled]) => enabled != null && enabled !== false)
      .map(([name, child]) => child === true ? name : `${name}: ${displayValue(child)}`).join(', ') || 'Keine';
  }
  return String(value);
}

export function PendingChanges({ connectivity, readOnly }: { connectivity: string; readOnly: boolean }) {
  const [rows, setRows] = useState<Change[]>([]);
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [inspection, setInspection] = useState<{ row: Change; details: ChangeDetails } | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const prompted = useRef(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const refresh = useCallback(async () => {
    try { setRows(await invoke<Change[]>('pending_changes')); }
    catch { setError('Änderungen konnten nicht geladen werden.'); }
  }, []);
  useEffect(() => {
    void refresh();
    const update = () => { void refresh(); };
    window.addEventListener('desktop-write-queued', update);
    window.addEventListener('desktop-changes-updated', update);
    return () => {
      window.removeEventListener('desktop-write-queued', update);
      window.removeEventListener('desktop-changes-updated', update);
    };
  }, [refresh, connectivity]);
  useEffect(() => {
    if (connectivity === 'offline') prompted.current = false;
    if (document.querySelector('.desktop-google-import[open]')) return;
    if (connectivity === 'online' && !prompted.current && rows.some((row) => row.requiresReview && row.kind !== 'contact')) {
      prompted.current = true;
      setSelected(new Set());
      setOpen(true);
    }
  }, [connectivity, rows]);
  useEffect(() => {
    if (open) dialog.current?.showModal();
    else { dialog.current?.close(); trigger.current?.focus(); }
  }, [open]);
  const openObject = async (row: Change) => {
    try {
      const { client, activeAccountId } = useAuthStore.getState();
      if (!client) throw new Error('Konto ist nicht verfügbar.');
      if (row.kind === 'calendar') {
        const event = await client.getCalendarEvent(row.objectId, row.accountId);
        if (!event) throw new Error('Der Termin ist nicht mehr lokal verfügbar.');
        const calendarState = useCalendarStore.getState();
        const existing = calendarState.events.find((item) => (item.originalId || item.id) === row.objectId && item.accountId === row.accountId);
        const shared = row.accountId !== client.getCalendarsAccountId();
        const mappedCalendars = Object.fromEntries(Object.entries(event.calendarIds || {}).map(([id, value]) => [calendarState.calendars.find((cal) => (cal.originalId || cal.id) === id && cal.accountId === row.accountId)?.id || (shared ? `${row.accountId}:${id}` : id), value]));
        const sourced = { ...existing, ...event, id: existing?.id || (shared ? `${row.accountId}:${event.id}` : event.id), originalId: event.id, originalCalendarIds: event.calendarIds, calendarIds: mappedCalendars, accountId: row.accountId, localAccountId: activeAccountId ?? undefined, isShared: shared };
        useCalendarStore.setState({ events: [...calendarState.events.filter((item) => item.id !== sourced.id), sourced] });
        navigate('/de/calendar'); setOpen(false);
        window.setTimeout(() => window.dispatchEvent(new CustomEvent('desktop-open-calendar-event', { detail: sourced })), 100);
        return;
      }
      if (row.kind === 'contact') {
        await refreshLocalViews();
        const contact = useContactStore.getState().contacts.find((item) => (item.originalId || item.id) === row.objectId && (!item.accountId || item.accountId === row.accountId));
        if (!contact) throw new Error('Dieser Kontakt ist nicht mehr lokal verfügbar.');
        navigate(`/de${buildContactsPath({ contactId: contact.id })}`); setOpen(false); return;
      }
      if (row.kind === 'file') { const id = row.accountId === client.getFilesAccountId() ? row.objectId : `${row.accountId}:${row.objectId}`; navigate(`/de/files?preview=${encodeURIComponent(id)}`); setOpen(false); return; }
      const editSend = row.kind === 'send' && row.journalId != null && ['held', 'ready', 'blocked'].includes(row.status ?? 'held');
      const objectId = editSend ? await invoke<string>('edit_queued_send', { accountId: row.accountId, journalId: row.journalId }) : row.objectId;
      if (editSend) { await refresh(); window.dispatchEvent(new Event('desktop-changes-updated')); }
      const email = await client.getEmail(objectId, row.accountId);
      if (!email) throw new Error('Diese Nachricht ist nicht mehr lokal verfügbar.');
      navigate('/de/mail');
      const sourced = { ...email, sourceAccountId: row.accountId, sourceClientAccountId: activeAccountId ?? undefined };
      useEmailStore.getState().selectEmail(sourced);
      setOpen(false);
      if (row.kind === 'draft' || editSend) window.setTimeout(() => window.dispatchEvent(new CustomEvent('desktop-open-draft', { detail: sourced })), 100);
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Öffnen fehlgeschlagen.'); }
  };
  const approve = async () => {
    setBusy(true); setError('');
    try {
      const changes = rows.filter((row) => selected.has(key(row)) && row.transferable && !row.blocked)
        .map(({ accountId, sequence, journalId }) => ({ accountId, sequence, journalId }));
      await invoke('approve_pending_changes', { changes });
      setSelected(new Set());
      await refresh();
      window.dispatchEvent(new Event('desktop-write-queued'));
    } catch { setError('Freigabe konnte nicht gespeichert werden.'); }
    finally { setBusy(false); }
  };
  const inspect = async (row: Change) => {
    setBusy(true); setError(''); setAcknowledged(false);
    try { setInspection({ row, details: await invoke<ChangeDetails>('change_details', { selection: selection(row) }) }); }
    catch (reason) { setError(typeof reason === 'string' ? reason : 'Aktion konnte nicht geprüft werden.'); }
    finally { setBusy(false); }
  };
  const resolve = async (action: 'discard' | 'retry') => {
    if (!inspection) return;
    setBusy(true); setError('');
    try {
      await invoke('resolve_change', { selection: selection(inspection.row), action,
        expectedRevision: inspection.details.revision, acknowledgeUncertain: acknowledged });
      setInspection(null); setSelected(new Set()); setAcknowledged(false);
      await refresh();
      window.dispatchEvent(new Event('desktop-changes-updated'));
      await refreshLocalViews();
    } catch (reason) { setError(typeof reason === 'string' ? reason : 'Aktion konnte nicht aufgelöst werden.'); }
    finally { setBusy(false); }
  };
  const pending = rows.filter((row) => row.transferable && row.status !== 'applied').length;
  return <>
    <button ref={trigger} type="button" className="desktop-changes-toggle" title="Ausstehende Aktionen und lokale Entwürfe anzeigen" onClick={() => { setInspection(null); setError(''); setOpen(true); void refresh(); }}>
      {pending > 0 ? `${pending} ${pending === 1 ? 'ausstehende Aktion' : 'ausstehende Aktionen'}` : 'Keine ausstehenden Aktionen'}
    </button>
    {createPortal(<dialog ref={dialog} className="desktop-changes-dialog" aria-labelledby="desktop-changes-title" onCancel={() => setOpen(false)} onClose={() => setOpen(false)}>
      <h2 id="desktop-changes-title">Lokale Änderungen übertragen?</h2>
      {!inspection && <p>Wähle die Änderungen aus, die beim Server gespeichert werden sollen. Mit „Später“ bleiben sie lokal erhalten.</p>}
      {connectivity !== 'online' && <p>Die Auswahl kann nach erfolgreicher Verbindung freigegeben werden.</p>}
      {readOnly && <p>Nur-Lese-Modus · Übertragung ist gesperrt.</p>}
      {error && <p role="alert">{error}</p>}
      {inspection ? <section className="desktop-change-inspection" aria-label="Aktion prüfen">
        <h3>{inspection.details.title}</h3>
        <p>{inspection.details.warning}</p>
        <div className="desktop-change-comparison"><table>
          <thead><tr><th>Eigenschaft</th><th>Gespeicherter Serverstand</th><th>Lokale Absicht</th></tr></thead>
          <tbody>{inspection.details.fields.map((field, index) => <tr key={index}>
            <th scope="row">{field.label}</th><td>{displayValue(field.current)}</td><td>{displayValue(field.desired)}</td>
          </tr>)}</tbody>
        </table></div>
        {inspection.details.canDiscard && <label className="desktop-change-confirm">
          <input type="checkbox" checked={acknowledged} disabled={busy} onChange={(event) => setAcknowledged(event.target.checked)} />
          {inspection.details.uncertain ? 'Ich habe den Serverstatus geprüft. Nur lokal abschließen; keine Serveränderung zurücknehmen.' : 'Diese lokale Absicht verwerfen. Bereits übertragene Änderungen bleiben auf dem Server.'}
        </label>}
        <div className="desktop-changes-actions">
          <button type="button" disabled={busy} onClick={() => { setInspection(null); setError(''); }}>Zurück</button>
          <button type="button" disabled={busy} onClick={() => { void inspect(inspection.row); }}>Stand neu laden</button>
          {inspection.details.canRetry && <button type="button" disabled={busy || readOnly} onClick={() => { void resolve('retry'); }}>Erneut vormerken</button>}
          {inspection.details.canDiscard && <button type="button" disabled={busy || !acknowledged} onClick={() => { void resolve('discard'); }}>{inspection.details.uncertain ? 'Lokal abschließen' : 'Lokale Absicht verwerfen'}</button>}
        </div>
      </section> : <><div className="desktop-changes-list">
        {rows.length === 0 && <p>Keine ausstehenden Änderungen oder lokalen Entwürfe.</p>}
        {rows.map((row) => <div className="desktop-change" key={key(row)}>
          <input type="checkbox" aria-label={`${row.title}: ${row.action}`} disabled={!row.transferable || !!row.blocked || ['sending', 'applied'].includes(row.status ?? '') || readOnly || busy}
            checked={selected.has(key(row))} onChange={(event) => setSelected((previous) => {
              const next = new Set(previous); if (event.target.checked) next.add(key(row)); else next.delete(key(row)); return next;
            })} />
          <div><strong>{row.title}</strong><p>{row.action}</p><small>Konto: {row.accountId} · {row.status === 'sending' ? 'Wird übertragen' : row.status === 'applied' ? 'Bestätigt · lokale Ansicht wird aktualisiert' : row.blocked ? `Prüfung nötig: ${row.blocked}` : !row.transferable ? 'Nur lokal gespeichert' : row.requiresReview ? 'Wartet auf Freigabe' : 'Freigegeben'}</small></div>
          <div className="desktop-change-buttons">
            <button type="button" disabled={busy || row.status === 'sending'} onClick={() => { void openObject(row); }}>{row.kind === 'send' && ['held', 'ready', 'blocked'].includes(row.status ?? 'held') ? 'Als Entwurf bearbeiten' : row.kind === 'send' && row.status === 'applied' ? 'Lokal öffnen' : 'Öffnen'}</button>
            {row.transferable && !['sending', 'applied'].includes(row.status ?? '') && <button type="button" disabled={busy} onClick={() => { void inspect(row); }}>Prüfen</button>}
          </div>
        </div>)}
      </div>
      <p>Zum Versand vorgemerkte Mails und gespeicherte Änderungen werden erst nach deiner Freigabe übertragen. Reine Entwürfe bleiben Entwürfe.</p>
      <div className="desktop-changes-actions"><button type="button" onClick={() => setOpen(false)}>Später</button>
        <button type="button" disabled={busy || readOnly || connectivity !== 'online' || selected.size === 0} onClick={() => { void approve(); }}>Auswahl übertragen ({selected.size})</button></div></>}
    </dialog>, document.body)}
  </>;
}

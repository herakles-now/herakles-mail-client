import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { useAuthStore } from '@/stores/auth-store';
import { useEmailStore } from '@/stores/email-store';
import { useCalendarStore } from '@/stores/calendar-store';
import { navigate } from './navigation';

interface Change {
  accountId: string;
  sequence?: number;
  journalId?: number;
  objectId: string;
  kind: 'mail' | 'draft' | 'send' | 'calendar' | 'file';
  title: string;
  action: string;
  requiresReview: boolean;
  status?: string;
  blocked?: string;
  transferable: boolean;
}
const key = (row: Change) => JSON.stringify([row.accountId, row.journalId != null ? `journal:${row.journalId}` : row.sequence ?? row.objectId]);

export function PendingChanges({ connectivity, readOnly }: { connectivity: string; readOnly: boolean }) {
  const [rows, setRows] = useState<Change[]>([]);
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
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
    if (connectivity === 'online' && !prompted.current && rows.some((row) => row.requiresReview)) {
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
  const pending = rows.filter((row) => row.transferable && row.status !== 'applied').length;
  return <>
    <button ref={trigger} type="button" className="desktop-changes-toggle" title="Ausstehende Aktionen und lokale Entwürfe anzeigen" onClick={() => { setOpen(true); void refresh(); }}>
      {pending > 0 ? `${pending} ${pending === 1 ? 'ausstehende Aktion' : 'ausstehende Aktionen'}` : 'Keine ausstehenden Aktionen'}
    </button>
    {createPortal(<dialog ref={dialog} className="desktop-changes-dialog" aria-labelledby="desktop-changes-title" onCancel={() => setOpen(false)} onClose={() => setOpen(false)}>
      <h2 id="desktop-changes-title">Lokale Änderungen übertragen?</h2>
      <p>Wähle die Änderungen aus, die beim Server gespeichert werden sollen. Mit „Später“ bleiben sie lokal erhalten.</p>
      {connectivity !== 'online' && <p>Die Auswahl kann nach erfolgreicher Verbindung freigegeben werden.</p>}
      {readOnly && <p>Nur-Lese-Modus · Übertragung ist gesperrt.</p>}
      {error && <p role="alert">{error}</p>}
      <div className="desktop-changes-list">
        {rows.length === 0 && <p>Keine ausstehenden Änderungen oder lokalen Entwürfe.</p>}
        {rows.map((row) => <div className="desktop-change" key={key(row)}>
          <input type="checkbox" aria-label={`${row.title}: ${row.action}`} disabled={!row.transferable || !!row.blocked || ['sending', 'applied'].includes(row.status ?? '') || readOnly || busy}
            checked={selected.has(key(row))} onChange={(event) => setSelected((previous) => {
              const next = new Set(previous); if (event.target.checked) next.add(key(row)); else next.delete(key(row)); return next;
            })} />
          <div><strong>{row.title}</strong><p>{row.action}</p><small>Konto: {row.accountId} · {row.status === 'sending' ? 'Wird übertragen' : row.status === 'applied' ? 'Bestätigt · lokale Ansicht wird aktualisiert' : row.blocked ? `Prüfung nötig: ${row.blocked}` : !row.transferable ? 'Nur lokal gespeichert' : row.requiresReview ? 'Wartet auf Freigabe' : 'Freigegeben'}</small></div>
          <button type="button" disabled={row.status === 'sending'} onClick={() => { void openObject(row); }}>{row.kind === 'send' && ['held', 'ready', 'blocked'].includes(row.status ?? 'held') ? 'Als Entwurf bearbeiten' : row.kind === 'send' && row.status === 'applied' ? 'Lokal öffnen' : 'Öffnen'}</button>
        </div>)}
      </div>
      <p>Zum Versand vorgemerkte Mails und gespeicherte Änderungen werden erst nach deiner Freigabe übertragen. Reine Entwürfe bleiben Entwürfe.</p>
      <div className="desktop-changes-actions"><button type="button" onClick={() => setOpen(false)}>Später</button>
        <button type="button" disabled={busy || readOnly || connectivity !== 'online' || selected.size === 0} onClick={() => { void approve(); }}>Auswahl übertragen ({selected.size})</button></div>
    </dialog>, document.body)}
  </>;
}

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { calendarHooks } from '@/lib/plugin-hooks';
import { eventToView } from '@/lib/plugin-projection';
import type { CalendarEvent } from '@/lib/jmap/types';
import type { IJMAPClient } from '@/lib/jmap/client-interface';
import { desktopIsReadOnly, refreshLocalViews } from './bootstrap';
import { planInvitationUpdate, type InvitationRecord } from './invitation-updates';

interface Props { incoming: InvitationRecord | null; accountId?: string; client: IJMAPClient | null; trusted: boolean; method: string; }
interface Snapshot { accountId: string; events: Record<string, InvitationRecord>; calendars: InvitationRecord[]; accessDenied?: boolean; eventState: string | null; calendarState: string | null; localRevision?: number; }
const messages = {
  current: 'Die Einladung entspricht dem lokalen Kalenderstand.',
  outdated: 'Diese Einladung ist veraltet. Im Kalender steht eine neuere Version.',
  conflict: 'Einladung und Kalender widersprechen sich. Bitte den aktuellen Termin prüfen.',
  unsupported: 'Diese Serienänderung muss zunächst mit dem Server abgeglichen werden.',
  untrusted: 'Herkunft oder Organisator der Änderung konnten nicht ausreichend bestätigt werden.',
  update: 'Die Einladung enthält eine neuere Version als dein lokaler Kalender.',
};
export function InvitationUpdate({ incoming, accountId, client, trusted, method }: Props) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const refresh = useCallback(async () => {
    if (!accountId || !incoming?.uid) return;
    try { setSnapshot({ ...await invoke<Snapshot>('local_calendar_snapshot', { accountId }), accountId }); }
    catch { setSnapshot(null); setError('Der lokale Kalenderstand konnte nicht geprüft werden.'); }
  }, [accountId, incoming?.uid]);
  useEffect(() => {
    setSnapshot(null); setNotice(''); void refresh();
    const update = () => { void refresh(); };
    window.addEventListener('desktop-changes-updated', update);
    return () => window.removeEventListener('desktop-changes-updated', update);
  }, [refresh, incoming]);
  if (method !== 'request' || !incoming || !accountId || !client) return null;
  const plan = snapshot && snapshot.accountId === accountId && snapshot.eventState && !snapshot.accessDenied ? planInvitationUpdate(incoming, Object.values(snapshot.events), trusted) : null;
  if (plan?.kind === 'missing') return null; // Keep Bulwark's original import action.
  const allowed = plan?.kind === 'update' && !desktopIsReadOnly() && snapshot?.calendars.some((calendar) => snapshot.events[plan.id]?.calendarIds?.[calendar.id] && calendar.myRights?.mayWriteAll);
  const apply = async () => {
    if (!allowed || !snapshot || plan?.kind !== 'update') return;
    setBusy(true); setError('');
    try {
      if (!(await calendarHooks.onBeforeEventUpdate.intercept({ eventId: plan.id, patch: plan.patch }))) return;
      const result = await invoke<InvitationRecord>('local_object_set', { method: 'CalendarEvent/set', args: {
        accountId, ifInState: `local-calendar:${snapshot.eventState}:${snapshot.calendarState}:${snapshot.localRevision ?? 0}`,
        sendSchedulingMessages: false, update: { [plan.id]: plan.patch },
      } });
      if (!Object.hasOwn(result.updated ?? {}, plan.id)) throw new Error('rejected');
      // Both views read back the durable redb overlay. A queued change is not an RSVP.
      window.dispatchEvent(new Event('desktop-write-queued'));
      await refreshLocalViews(); await refresh();
      if (calendarHooks.onAfterEventUpdate.size) void calendarHooks.onAfterEventUpdate.emit(eventToView({ ...snapshot.events[plan.id], ...plan.patch } as CalendarEvent));
      setNotice('Terminänderung lokal vorgemerkt. Es wurde keine Teilnahmeantwort gesendet. Den Übertragungsstatus findest du unter „Ausstehende Änderungen“.');
    } catch (reason) {
      const code = typeof reason === 'string' ? reason : reason instanceof Error ? reason.message : '';
      await refresh();
      setError(code === 'stateMismatch' ? 'Der Kalender hat sich inzwischen geändert. Bitte erneut prüfen.' : code === 'forbidden' ? 'Dieser Kalender ist nur lesbar.' : 'Die Terminänderung konnte nicht vorgemerkt werden. Bitte den Kalender prüfen.');
    } finally { setBusy(false); }
  };
  return <div className="desktop-invitation-update text-xs space-y-2" aria-live="polite">
    {plan && <p>{messages[plan.kind]}</p>}
    {allowed && <button type="button" className="rounded border px-2 py-1" disabled={busy} onClick={() => { void apply(); }}>Terminangaben übernehmen</button>}
    {notice && <p>{notice}</p>}{error && <p role="alert">{error}</p>}
  </div>;
}

import { readDesktopBlob } from './blobs';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { flushDesktopStorage } from './storage';
import { flushBeforeClose } from './close';
import { JMAPClient } from '@/lib/jmap/client';
import { OfflineUnavailableError, setOfflineHooks } from '@/lib/jmap/offline-hooks';
import { useAuthStore } from '@/stores/auth-store';
import { useAccountStore } from '@/stores/account-store';
import { setClientLookup } from '@/stores/client-registry';
import { usePolicyStore } from '@/stores/policy-store';
import { useEmailStore } from '@/stores/email-store';
import { useCalendarStore } from '@/stores/calendar-store';
import { answerCalendar } from './calendar';
import { DEFAULT_POLICY } from '@/lib/admin/types';
import { applyLiteConfig } from '@/lib/lite-config';
import { fetchConfig } from '@/hooks/use-config';
import { accountsFor, sessionFingerprint, MAIL_CAPABILITY, CALENDAR_CAPABILITY } from './accounts';
import type { DesktopSession } from './accounts';
export interface Bootstrap { session: DesktopSession; serverUrl: string; username: string; readOnly: boolean; }
let currentLogin: { info: Bootstrap; client: JMAPClient; localId: string } | null = null;

async function invokeComposer(methodCalls: import('@/lib/jmap/offline-hooks').OfflineMethodCall[]) {
  try { return await invoke<import('@/lib/jmap/offline-hooks').OfflineResponse>('composer_request', { methodCalls }); }
  catch (reason) {
    if (typeof reason === 'string' && reason.startsWith('Offline-Modus aktiv:')) {
      throw new OfflineUnavailableError('Offline: Die Nachricht wurde nicht versendet. Der Entwurf bleibt erhalten.');
    }
    throw reason;
  }
}

export async function bootstrapBulwark(): Promise<Bootstrap> {
  (globalThis as unknown as { __DESKTOP_PLUGIN_URL__: string }).__DESKTOP_PLUGIN_URL__ = convertFileSrc('index.html', 'bulwark-plugin');
  // Keep this optional for browser tests; native close waits for durable UI/plugin writes.
  if ((window as unknown as { __TAURI_INTERNALS__?: { metadata?: unknown } }).__TAURI_INTERNALS__?.metadata) {
    const appWindow = getCurrentWindow();
    await appWindow.onCloseRequested(async () => {
      window.dispatchEvent(new Event('pagehide'));
      await flushBeforeClose(flushDesktopStorage, () => window.dispatchEvent(new Event('desktop-storage-error')));
    });
  }
  const info = await invoke<Bootstrap>('desktop_bootstrap');
  const client = JMAPClient.fromOfflineSession(info.serverUrl, info.username, info.session);
  client.supportsSyntheticCalendarIds = async () => false;
  setOfflineHooks({
    skipNetwork: () => true,
    answer: async (requestClient, methodCalls) => {
      if (methodCalls.some(([method]) => method.startsWith('Calendar') || method.startsWith('ParticipantIdentity/'))) {
        // Calendar writes are not part of the initial mail-flag writing scope.
        return answerCalendar(methodCalls, currentLogin?.client.getCalendarsAccountId() || client.getCalendarsAccountId(), true);
      }
      // Bulwark's composer remains upstream-owned. Only identity lookup and
      // its explicit JMAP submission batch cross the native authenticated
      // transport; the renderer never receives credentials or a server URL
      // with authorization attached.
      const submissionCleanup = methodCalls.some(([method, args]) => method === 'EmailSubmission/set' && args.destroy && !args.create);
      if (submissionCleanup) {
        const responses: import('@/lib/jmap/offline-hooks').OfflineResponse['methodResponses'] = [];
        for (const call of methodCalls) {
          const response = call[0] === 'EmailSubmission/set'
            ? await invokeComposer([call])
            : await invoke<import('@/lib/jmap/offline-hooks').OfflineResponse>('local_jmap', { accountId: requestClient.getAccountId(), methodCalls: [call] });
          responses.push(...response.methodResponses);
        }
        return { methodResponses: responses };
      }
      if (methodCalls.some(([method]) => method === 'EmailSubmission/set' || method === 'Identity/get')) {
        if (typeof navigator !== 'undefined' && navigator.onLine) {
          const response = await invokeComposer(methodCalls);
          if (methodCalls.some(([method]) => method === 'EmailSubmission/set')) {
            window.dispatchEvent(new Event('desktop-write-queued'));
          }
          return response;
        }
        if (methodCalls.some(([method]) => method === 'EmailSubmission/set')) {
          throw new OfflineUnavailableError('Offline: Die Nachricht wurde nicht versendet. Der Entwurf bleibt erhalten.');
        }
      }
      const response = await invoke<import('@/lib/jmap/offline-hooks').OfflineResponse>('local_jmap', { accountId: requestClient.getAccountId(), methodCalls });
      for (const [method, result] of response.methodResponses) if (method === 'Email/set' && (
        Object.keys(result.updated || {}).length || Object.keys(result.created || {}).length || (result.destroyed || []).length
      )) {
        window.dispatchEvent(new Event('desktop-write-queued'));
      }
      return response;
    },
    record: () => {}, noteNetwork: () => {}, serverGone: async () => true,
    blob: (_client, blobId, accountId) => readDesktopBlob(accountId, blobId), recordBlob: () => {}, holdUpload: async () => null,
  });
  const config = applyLiteConfig({ appName: 'Herakles Mail', jmapServerUrl: info.serverUrl, noThirdPartyTraffic: true });
  const policy = { ...DEFAULT_POLICY, features: { ...DEFAULT_POLICY.features, pluginsEnabled: true, pluginsUploadEnabled: true, requirePluginApproval: false } };
  const nativeFetch = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const raw = input instanceof Request ? input.url : String(input);
    const url = new URL(raw, window.location.href);
    if (url.origin === window.location.origin && url.pathname.startsWith('/api/')) {
      const method = init?.method || (input instanceof Request ? input.method : 'GET');
      if (method !== 'GET') return Response.json({ error: 'Diese Serverfunktion ist im Desktop-Client noch nicht verfügbar.' }, { status: 501 });
      switch (url.pathname) {
        case '/api/config': return Response.json(config);
        case '/api/admin/policy': return Response.json(policy);
        case '/api/plugins': return Response.json({ plugins: [] });
        case '/api/themes': return Response.json({ themes: [] });
        case '/api/version': return Response.json({ version: '1.20.0', buildId: 'desktop' });
        default: return Response.json({ error: 'Desktop-Adapter für diese Funktion fehlt noch.' }, { status: 501 });
      }
    }
    if (url.origin !== window.location.origin && !['blob:', 'data:'].includes(url.protocol)) {
      throw new TypeError('Desktop: externe Netzwerkzugriffe laufen ausschließlich über native Adapter.');
    }
    return nativeFetch(input, init);
  };
  await fetchConfig();
  await usePolicyStore.getState().fetchPolicy();
  const localId = useAccountStore.getState().addAccount({
    label: info.username, serverUrl: info.serverUrl, username: info.username, authMode: 'basic',
    rememberMe: false, displayName: info.username, email: info.username,
    lastLoginAt: Date.now(), isConnected: true, hasError: false, isDefault: true,
  });
  useAccountStore.getState().setActiveAccount(localId);
  useAuthStore.setState({ client, isAuthenticated: true, isLoading: false, error: null,
    serverUrl: info.serverUrl, username: info.username, activeAccountId: localId,
    connectionLost: false, restoringAccounts: false, checkAuth: async () => {},
    getClientForAccount: (id) => id === localId ? client : undefined,
    getAllConnectedClients: () => new Map([[localId, client]]), reconnectOfflineClients: async () => true,
  });
  setClientLookup((id) => id === localId ? client : undefined);
  currentLogin = { info, client, localId };
  useCalendarStore.getState().setSupported(accountsFor(info.session, CALENDAR_CAPABILITY).length > 0);
  if (useCalendarStore.getState().supportsCalendar) await useCalendarStore.getState().fetchCalendars(client);
  return info;
}
let syncTask: Promise<void> | null = null;
let syncAgain = false;
export function synchronizeBulwark(): Promise<void> {
  if (syncTask) { syncAgain = true; return syncTask; }
  syncTask = (async () => {
    let failure: unknown;
    do {
      syncAgain = false;
      try { await synchronizeOnce(); failure = undefined; } catch (error) { failure = error; }
    } while (syncAgain);
    if (failure) throw failure;
  })().finally(() => { syncTask = null; });
  return syncTask;
}
async function synchronizeOnce() {
    if (!currentLogin) return;
    const progress = (label: string) => window.dispatchEvent(new CustomEvent('desktop-sync-progress', { detail: label }));
    progress('Sitzung');
    // Refresh the account inventory, not only message state: new/revoked shares change the session.
    const step = async <T,>(name: string, action: () => Promise<T>): Promise<T> => {
      try { return await action(); } catch (error) {
        const code = error instanceof Error ? error.name.replace(/[^a-zA-Z0-9._-]/g, '').slice(0, 48) : 'native_error';
        await invoke('log_frontend_event', { level: 'error', area: 'sync', event: name, code }).catch(() => {});
        throw error;
      }
    };
    const info = await step('session_refresh_failed', () => invoke<Bootstrap>('refresh_desktop_session'));
    window.dispatchEvent(new Event('desktop-server-reachable'));
    if (sessionFingerprint(info.session) !== sessionFingerprint(currentLogin.info.session) || info.readOnly !== currentLogin.info.readOnly) {
      const client = JMAPClient.fromOfflineSession(info.serverUrl, info.username, info.session);
      client.supportsSyntheticCalendarIds = async () => false;
      const { localId } = currentLogin;
      currentLogin = { info, client, localId };
      useAuthStore.setState({ client,
        getClientForAccount: (id) => id === localId ? client : undefined,
        getAllConnectedClients: () => new Map([[localId, client]]) });
      setClientLookup((id) => id === localId ? client : undefined);
    } else currentLogin.info = info;
    const { client } = currentLogin;
    const mailAccounts = accountsFor(info.session, MAIL_CAPABILITY);
    const calendarAccounts = accountsFor(info.session, CALENDAR_CAPABILITY);
    useCalendarStore.getState().setSupported(calendarAccounts.length > 0);
    const failures: string[] = [];
    const attempt = async (command: string, accountId: string) => {
      try { await step(`${command}_failed`, () => invoke(command, { accountId })); } catch (error) { failures.push(error instanceof Error ? error.message : typeof error === 'string' ? error : 'Ein Konto konnte nicht abgeglichen werden.'); }
    };
    // Fetch every folder tree before downloading entire mailboxes, so shares appear promptly.
    for (const [index, id] of mailAccounts.entries()) { progress(`Ordner ${index + 1}/${mailAccounts.length}`); await attempt('sync_mailboxes', id); }
    await useEmailStore.getState().fetchMailboxes(client);
    for (const [index, id] of calendarAccounts.entries()) { progress(`Kalender ${index + 1}/${calendarAccounts.length}`); await attempt('sync_calendar', id); }
    if (calendarAccounts.length) {
      await step('local_calendars_refresh_failed', () => useCalendarStore.getState().fetchCalendars(client));
      const range = useCalendarStore.getState().dateRange;
      if (range) await useCalendarStore.getState().fetchEvents(client, range.start, range.end, { silent: true });
    }
    for (const [index, id] of mailAccounts.entries()) { progress(`Nachrichten ${index + 1}/${mailAccounts.length}`); await attempt('sync_mail', id); }
    progress('Lokale Ansicht');
    await step('local_mailboxes_refresh_failed', () => useEmailStore.getState().fetchMailboxes(client));
    await step('local_emails_refresh_failed', () => useEmailStore.getState().fetchEmails(client));
    if (failures.length) throw new Error([...new Set(failures)].join(' · '));
}

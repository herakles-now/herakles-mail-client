import { readDesktopBlob, stageDesktopUpload } from './blobs';
import { answerFiles } from './files';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { flushDesktopStorage } from './storage';
import { flushBeforeClose } from './close';
import { JMAPClient } from '@/lib/jmap/client';
import { setOfflineHooks } from '@/lib/jmap/offline-hooks';
import { useAuthStore } from '@/stores/auth-store';
import { useAccountStore } from '@/stores/account-store';
import { setClientLookup } from '@/stores/client-registry';
import { usePolicyStore } from '@/stores/policy-store';
import { useEmailStore } from '@/stores/email-store';
import { useCalendarStore } from '@/stores/calendar-store';
import { useContactStore } from '@/stores/contact-store';
import { useIdentityStore } from '@/stores/identity-store';
import { useFileStore } from '@/stores/file-store';
import { answerCalendar } from './calendar';
import { DEFAULT_POLICY } from '@/lib/admin/types';
import { applyLiteConfig } from '@/lib/lite-config';
import { fetchConfig } from '@/hooks/use-config';
import { accountsFor, sessionFingerprint, MAIL_CAPABILITY, CALENDAR_CAPABILITY, CONTACT_CAPABILITY } from './accounts';
import type { DesktopSession } from './accounts';
export interface Bootstrap { session: DesktopSession; serverUrl: string; username: string; readOnly: boolean; }
let currentLogin: { info: Bootstrap; client: JMAPClient; localId: string } | null = null;
export function desktopIsReadOnly(): boolean { return currentLogin?.info.readOnly ?? true; }
const rejectContactWrite = async (): Promise<never> => { throw new Error('Kontakte stehen im Desktop-Client derzeit nur lesend zur Verfügung.'); };

function initializeReadOnlyContacts() {
  // Also reject upstream's local-only fallback paths. Contacts must not appear
  // successfully edited in a browser store without a native durable intention.
  useContactStore.setState({
    contacts: [], addressBooks: [], contactsLoaded: false, selectedContactId: null, directoryPrincipals: [], directoryLoaded: false,
    createContact: rejectContactWrite, updateContact: rejectContactWrite, deleteContact: rejectContactWrite,
    addLocalContact: rejectContactWrite, updateLocalContact: rejectContactWrite, deleteLocalContact: rejectContactWrite,
    createGroup: rejectContactWrite, updateGroup: rejectContactWrite, deleteGroup: rejectContactWrite,
    addMembersToGroup: rejectContactWrite, removeMembersFromGroup: rejectContactWrite,
    bulkDeleteContacts: rejectContactWrite, bulkAddToGroup: rejectContactWrite, moveContactToAddressBook: rejectContactWrite,
    createAddressBook: rejectContactWrite, renameAddressBook: rejectContactWrite, removeAddressBook: rejectContactWrite,
    setDefaultAddressBook: rejectContactWrite, shareAddressBook: rejectContactWrite, importContacts: rejectContactWrite,
    renameKeyword: rejectContactWrite, removeKeyword: rejectContactWrite, addKeyword: rejectContactWrite,
    addToTrustedSendersBook: rejectContactWrite, removeFromTrustedSendersBook: rejectContactWrite,
  });
}

// Re-read only the local IPC adapters after resolving an intention. This must
// work offline and must not start transmission of a newly held retry.
export async function refreshLocalViews(): Promise<void> {
  if (!currentLogin) return;
  const { client, info } = currentLogin;
  if (accountsFor(info.session, MAIL_CAPABILITY).length) {
    await useEmailStore.getState().fetchMailboxes(client);
    await useEmailStore.getState().fetchEmails(client);
    const selected = useEmailStore.getState().selectedEmail;
    if (selected) {
      const refreshed = await client.getEmail(selected.id, selected.sourceAccountId).catch(() => null);
      useEmailStore.setState({ selectedEmail: refreshed ? { ...selected, ...refreshed } : null });
    }
  }
  if (accountsFor(info.session, CALENDAR_CAPABILITY).length) {
    await useCalendarStore.getState().fetchCalendars(client);
    const range = useCalendarStore.getState().dateRange;
    if (range) await useCalendarStore.getState().fetchEvents(client, range.start, range.end, { silent: true });
  }
  if (useFileStore.getState().client) await useFileStore.getState().refresh();
}

function desktopClient(info: Bootstrap): JMAPClient {
  const client = JMAPClient.fromOfflineSession(info.serverUrl, info.username, info.session);
  client.supportsSyntheticCalendarIds = async () => false;
  const supportsContacts = client.supportsContacts.bind(client);
  client.supportsContacts = (accountId) => supportsContacts(accountId || client.getContactsAccountId());
  return client;
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
  const client = desktopClient(info);
  (globalThis as unknown as { __DESKTOP_QUEUE_SUBMISSION__: (methodCalls: unknown[], draftId?: string, draftAccountId?: string) => Promise<unknown> }).__DESKTOP_QUEUE_SUBMISSION__ = async (methodCalls, draftId, draftAccountId) => {
    const result = await invoke('queue_submission', { methodCalls, draftId: draftId ?? null, draftAccountId: draftAccountId ?? null });
    window.dispatchEvent(new Event('desktop-write-queued'));
    return result;
  };
  client.supportsSyntheticCalendarIds = async () => false;
  setOfflineHooks({
    skipNetwork: () => true,
    answer: async (requestClient, methodCalls) => {
      if (methodCalls.every(([method]) => method === 'Identity/get')) {
        return { methodResponses: await Promise.all(methodCalls.map(async ([method, args, id]) =>
          [method, await invoke<Record<string, any>>('local_identities', { accountId: args.accountId || requestClient.getAccountId() }), id] as [string, Record<string, any>, string])) };
      }
      if (methodCalls.some(([method]) => method.startsWith('ContactCard/') || method.startsWith('AddressBook/'))) return invoke<import('@/lib/jmap/offline-hooks').OfflineResponse>(methodCalls.every(([method]) => method.startsWith('ContactCard/') || method.startsWith('AddressBook/')) ? 'local_contacts' : 'local_jmap', { accountId: requestClient.getAccountId(), methodCalls });
      if (methodCalls.some(([method]) => method.startsWith('FileNode/'))) return answerFiles(methodCalls);
      if (methodCalls.some(([method]) => method.startsWith('Calendar') || method.startsWith('ParticipantIdentity/'))) {
        // Calendar writes are not part of the initial mail-flag writing scope.
        return answerCalendar(methodCalls, currentLogin?.client.getCalendarsAccountId() || client.getCalendarsAccountId(), currentLogin?.info.readOnly ?? info.readOnly);
      }
      // Submissions are prepared by the original client, then stored by the
      // desktop boundary. Only the native sync worker can submit them.
      const submissionCleanup = methodCalls.some(([method, args]) => method === 'EmailSubmission/set' && args.destroy && !args.create);
      if (submissionCleanup) {
        const responses: import('@/lib/jmap/offline-hooks').OfflineResponse['methodResponses'] = [];
        for (const call of methodCalls) {
          const response = call[0] === 'EmailSubmission/set'
            ? { methodResponses: [[call[0], { accountId: call[1].accountId, destroyed: [] }, call[2]]] as import('@/lib/jmap/offline-hooks').OfflineResponse['methodResponses'] }
            : await invoke<import('@/lib/jmap/offline-hooks').OfflineResponse>('local_jmap', { accountId: requestClient.getAccountId(), methodCalls: [call] });
          responses.push(...response.methodResponses);
        }
        return { methodResponses: responses };
      }
      if (methodCalls.every(([method]) => method === 'EmailSubmission/get' || method === 'EmailSubmission/query')) return {
        methodResponses: methodCalls.map(([method, args, id]) => [method, { accountId: args.accountId, state: 'local', list: [], ids: [], notFound: args.ids || [] }, id]),
      };
      if (methodCalls.some(([method]) => method === 'EmailSubmission/set')) throw new Error('Versandaufträge müssen zuerst lokal gespeichert werden.');
      const response = await invoke<import('@/lib/jmap/offline-hooks').OfflineResponse>('local_jmap', { accountId: requestClient.getAccountId(), methodCalls });
      for (const [method, result] of response.methodResponses) if (method === 'Email/set' && (
        Object.keys(result.updated || {}).length || Object.keys(result.created || {}).length || (result.destroyed || []).length
      )) {
        window.dispatchEvent(new Event('desktop-write-queued'));
      }
      return response;
    },
    record: () => {}, noteNetwork: () => {}, serverGone: async () => true,
    blob: (_client, blobId, accountId) => readDesktopBlob(accountId, blobId), recordBlob: () => {}, holdUpload: (_client, file, accountId) => stageDesktopUpload(file, accountId),
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
  initializeReadOnlyContacts();
  if (accountsFor(info.session, CONTACT_CAPABILITY).length) {
    await useContactStore.getState().fetchAddressBooks(client);
    await useContactStore.getState().fetchContacts(client);
  }
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
      const client = desktopClient(info);
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
    for (const id of accountsFor(info.session, 'urn:ietf:params:jmap:submission')) await attempt('sync_identities', id);
    if (accountsFor(info.session, 'urn:ietf:params:jmap:submission').length) {
      await step('local_identities_refresh_failed', async () => { useIdentityStore.getState().setIdentities(await client.getIdentities()); });
    }
    await useEmailStore.getState().fetchMailboxes(client);
    const contactAccounts = accountsFor(info.session, CONTACT_CAPABILITY);
    for (const [index, id] of contactAccounts.entries()) { progress(`Kontakte ${index + 1}/${contactAccounts.length}`); await attempt('sync_contacts', id); }
    if (contactAccounts.length) {
      await step('local_contacts_refresh_failed', async () => {
        await useContactStore.getState().fetchAddressBooks(client);
        await useContactStore.getState().fetchContacts(client);
      });
    } else useContactStore.setState({ contacts: [], addressBooks: [], contactsLoaded: true, selectedContactId: null });
    for (const [index, id] of mailAccounts.entries()) { progress(`Nachrichten ${index + 1}/${mailAccounts.length}`); await attempt('sync_mail', id); }
    for (const id of accountsFor(info.session, 'urn:ietf:params:jmap:filenode')) await attempt('sync_files', id);
    // Read scheduling results after the mail pass, before presenting invitations.
    for (const [index, id] of calendarAccounts.entries()) { progress(`Kalender ${index + 1}/${calendarAccounts.length}`); await attempt('sync_calendar', id); }
    if (calendarAccounts.length) {
      await step('local_calendars_refresh_failed', () => useCalendarStore.getState().fetchCalendars(client));
      const range = useCalendarStore.getState().dateRange;
      if (range) await useCalendarStore.getState().fetchEvents(client, range.start, range.end, { silent: true });
    }
    const writesApplied = await step('pending_changes_failed', () => invoke<boolean>('sync_pending_changes'));
    // Read back accepted submissions and mutations before refreshing the visible stores.
    if (writesApplied) {
      for (const id of calendarAccounts) await attempt('sync_calendar', id);
      for (const id of mailAccounts) await attempt('sync_mail', id);
      for (const id of accountsFor(info.session, 'urn:ietf:params:jmap:filenode')) await attempt('sync_files', id);
      if (calendarAccounts.length) {
        const range = useCalendarStore.getState().dateRange;
        if (range) await useCalendarStore.getState().fetchEvents(client, range.start, range.end, { silent: true });
      }
    }
    progress('Lokale Ansicht');
    await step('local_mailboxes_refresh_failed', () => useEmailStore.getState().fetchMailboxes(client));
    await step('local_emails_refresh_failed', () => useEmailStore.getState().fetchEmails(client));
    if (failures.length) throw new Error([...new Set(failures)].join(' · '));
}

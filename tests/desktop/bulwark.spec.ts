import { test, expect } from '@playwright/test';
import JSZip from 'jszip';
import { readFile } from 'node:fs/promises';

for (const writable of [false, true]) {
test(`original Bulwark mail and plugins use native adapters (${writable ? 'writable' : 'read-only'})`, async ({ page }) => {
  await page.clock.setFixedTime(new Date('2026-11-03T10:00:00Z'));
  const values = new Map<string, string>();
  let serverAvailable = true;
  let networkEnabled = true;
  let sharesDiscovered = false;
  let syncBarrier: Promise<void> | null = null;
  let failMailSync = false;
  const syncedMail = new Set<string>();
  const syncedCalendars = new Set<string>();
  const syncedContacts = new Set<string>();
  let cacheLimitMib = 256;
  let flagged = false;
  let mailboxIds: Record<string, boolean> = { inbox: true };
  const pendingWrites = new Set<string>();
  const heldWrites = new Set<string>();
  let showLocalDraft = false;
  const journalWrites = new Map<number, any>();
  const resolvedWrites: any[] = [];
  let nextJournalId = 0;
  let eventTitle = 'Offline-Kalendertermin';
  let invitationMode = false;
  let invitationApplied = false;
  let invitationStoredSequence = 1;
  let rejectInvitationUpdate = true;
  const invitationWrites: any[] = [];
  let fileName = 'Offline-Datei.txt';
  const cachedBlobs = new Set<string>();
  const stagedUploads = new Map<string, { base64: string; mediaType: string }>();
  const attachmentBytes = Buffer.from([0, 128, 255, 65, 10]);
  const invitationBytes = Buffer.from('BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:synthetic-invitation-1\r\nDTSTART:20261103T110000Z\r\nDTEND:20261103T120000Z\r\nSUMMARY:Synthetische Einladung\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n');
  const calls: string[] = [];
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.route('**/plugin-sandbox-assets/**', async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), 'access-control-allow-origin': '*' } });
  });
  await page.exposeFunction('nativeFixture', async (command: string, args: Record<string, any>) => {
    calls.push(command);
    const email = { id: 'email-1', threadId: 'thread-1', blobId: 'blob-1', subject: 'Offline-Nachricht aus redb',
      from: [{ name: 'Desktop Test', email: 'sender@example.test' }], to: [{ email: 'reader@example.test' }],
      ...(invitationMode ? { authenticationResults: { dmarc: { result: 'pass', domain: 'example.test' } } } : {}),
      keywords: { $seen: true }, mailboxIds: { inbox: true }, receivedAt: '2026-11-03T10:00:00Z', size: 100,
      preview: 'Diese Nachricht kommt aus dem lokalen Repository.', hasAttachment: true,
      textBody: [{ partId: '1', type: 'text/plain' }], htmlBody: [], attachments: [{ partId: '2', blobId: 'attachment-1', type: 'application/octet-stream', name: 'offline-test.bin', size: attachmentBytes.length, disposition: 'attachment' }, { partId: 'ics', blobId: 'invite-1', type: 'text/calendar', name: 'invitation.ics', size: invitationBytes.length, disposition: 'attachment' }],
      bodyValues: { '1': { value: 'Lokal verfügbarer Testinhalt.', isTruncated: false } } };
    const inbox = { id: 'inbox', name: 'Inbox', role: 'inbox', parentId: null, sortOrder: 0,
      totalEmails: 1, unreadEmails: 0, totalThreads: 1, unreadThreads: 0, isSubscribed: true,
      myRights: { mayReadItems: true, mayAddItems: false, mayRemoveItems: false, maySetSeen: false,
        maySetKeywords: false, mayCreateChild: false, mayRename: false, mayDelete: false, maySubmit: false } };
    switch (command) {
      case 'download_cache_usage': return [{ accountId: 'account', name: 'Personal cache', usedBytes: Math.min(40, cacheLimitMib) * 1024 * 1024, limitBytes: cacheLimitMib * 1024 * 1024, count: 2 }, { accountId: 'shared-mail', name: 'Shared cache', usedBytes: 0, limitBytes: 256 * 1024 * 1024, count: 0 }];
      case 'set_download_cache_limit':
        if (args.accountId !== 'account' || args.limitMib < 25 || args.limitMib > 4096) throw 'invalid cache limit';
        cacheLimitMib = args.limitMib; return null;

      case 'stage_upload': {
        if (!writable || args.accountId !== 'account') throw 'forbidden';
        const blobId = `local-upload-${stagedUploads.size + 1}`;
        stagedUploads.set(JSON.stringify([args.accountId, blobId]), { base64: args.base64, mediaType: args.mediaType });
        return { blobId, size: Buffer.from(args.base64, 'base64').length, type: args.mediaType };
      }
      case 'local_blob': if (args.blobId === 'invite-1' && cachedBlobs.has(JSON.stringify([args.accountId, args.blobId]))) return { base64: invitationBytes.toString('base64'), mediaType: 'text/calendar' };
        if (stagedUploads.has(JSON.stringify([args.accountId, args.blobId]))) return stagedUploads.get(JSON.stringify([args.accountId, args.blobId]));
        return cachedBlobs.has(JSON.stringify([args.accountId, args.blobId])) ? { base64: attachmentBytes.toString('base64'), mediaType: 'application/octet-stream' } : null;
      case 'sync_blob':
        if (!serverAvailable) throw new Error('Server offline');
        if (!['attachment-1', 'invite-1'].includes(args.blobId)) throw new Error('Unknown fixture blob');
        cachedBlobs.add(JSON.stringify([args.accountId, args.blobId]));
        return null;
      case 'local_calendar_parse': {
        expect(cachedBlobs.has(JSON.stringify([args.accountId, 'invite-1']))).toBeTruthy();
        expect(args.blobIds).toEqual(['invite-1']);
        return { accountId: args.accountId, parsed: { 'invite-1': [{ '@type': 'Event', uid: 'synthetic-invitation-1', sequence: 2, status: 'confirmed', title: 'Synthetische Einladung', start: '2026-11-03T11:00:00', timeZone: 'Etc/UTC', duration: 'PT1H', calendarIds: null, id: null, organizerCalendarAddress: 'mailto:sender@example.test', participants: { reader: { calendarAddress: 'mailto:reader@example.test', name: 'Test Reader', roles: { attendee: true } } } }] }, notFound: [], notParsable: [] };
      }
      case 'push_status': return true;
      case 'profile_inventory': return { profiles: [{ id: 'fixture', name: 'Test', activeAccountId: 'login', accounts: [{ id: 'login', label: 'Desktop Test' }] }], activeProfileId: 'fixture' };
      case 'has_account_configuration': return true;
      case 'set_network_enabled':
        if (!args.enabled || !networkEnabled) for (const account of pendingWrites) heldWrites.add(account);
        if (!args.enabled || !networkEnabled) for (const row of journalWrites.values()) if (row.status === 'ready') row.status = 'held';
        networkEnabled = args.enabled; return null;
      case 'local_identities': return { accountId: args.accountId, state: 'identity', list: [{ id: 'identity-1', name: 'Desktop Test', email: 'reader@example.test', mayDelete: false }], notFound: [] };
      case 'sync_identities':
      case 'sync_files': if (!serverAvailable) throw new Error('Server offline'); return null;
      case 'local_files': return { state: 'files-1', list: [{ id: 'file-1', name: fileName, parentId: null, type: 'text/plain', size: 10, myRights: { mayRead: true, mayRename: true, mayDelete: true } }] };
      case 'local_object_set': {
        if (!writable) throw 'forbidden';
        const calendar = args.method === 'CalendarEvent/set';
        if (invitationMode && calendar) {
          expect(args.args.accountId).toBe('account');
          expect(args.args.ifInState).toMatch(/^local-calendar:/);
          if (rejectInvitationUpdate) { rejectInvitationUpdate = false; throw 'stateMismatch'; }
          invitationWrites.push(args.args);
          invitationApplied = true; invitationStoredSequence = 2;
        }
        if (calendar) eventTitle = args.args.update['event-1'].title;
        else fileName = args.args.update['file-1'].name;
        const journalId = ++nextJournalId;
        journalWrites.set(journalId, { journalId, accountId: args.args.accountId, objectId: calendar ? 'event-1' : 'file-1', kind: calendar ? 'calendar' : 'file', title: calendar ? eventTitle : fileName, action: 'Ändern', status: networkEnabled && serverAvailable ? 'ready' : 'held', transferable: true });
        return { accountId: args.args.accountId, oldState: 'local', newState: `local:${journalId}`, updated: { [calendar ? 'event-1' : 'file-1']: null }, notUpdated: {} };
      }
      case 'queue_submission': {
        if (!writable) throw 'forbidden';
        const email = Object.values(args.methodCalls[0][1].create)[0] as any;
        const journalId = ++nextJournalId;
        journalWrites.set(journalId, { journalId, accountId: 'account', objectId: `local-send-${journalId}`, kind: 'send', title: email.subject, action: 'Mail versenden', status: networkEnabled && serverAvailable ? 'ready' : 'held', transferable: true, email, methodCalls: args.methodCalls });
        return { scheduled: false, queued: true, outboxId: journalId };
      }
      case 'sync_pending_changes': {
        if (!networkEnabled || !serverAvailable) throw new Error('Server offline');
        let changed = false;
        for (const [id, row] of journalWrites) if (row.status === 'ready') { calls.push(`transport:${row.kind}`); journalWrites.delete(id); changed = true; }
        return changed;
      }
      case 'log_frontend_event': return null;
      case 'composer_request': {
        if (!networkEnabled) throw 'Offline-Modus aktiv: Aenderungen bleiben lokal vorgemerkt.';
        if (args.methodCalls.every(([method]: [string]) => method === 'Identity/get')) return { methodResponses: args.methodCalls.map(([method, arg, id]: [string, any, string]) => [method, { accountId: arg.accountId, state: 'identity', list: [{ id: 'identity-1', name: 'Desktop Test', email: 'reader@example.test', mayDelete: false }], notFound: [] }, id]) };
        if (!args.methodCalls.every(([method, arg]: [string, any]) => method === 'EmailSubmission/set' && Array.isArray(arg.destroy))) throw new Error('Unexpected send in fixture');
        return { methodResponses: args.methodCalls.map(([method, arg, id]: [string, any, string]) => [method, { accountId: arg.accountId, destroyed: arg.destroy }, id]) };
      }
      case 'desktop_kv_keys': return [...values.keys()].filter((key) => key.startsWith(args.prefix));
      case 'desktop_kv_get': return values.get(args.key) ?? null;
      case 'desktop_kv_put': values.set(args.key, args.value); return null;
      case 'desktop_kv_delete': values.delete(args.key); return null;
      case 'write_status': return { pending: pendingWrites.size, blocked: 0 };
      case 'pending_changes': return [...[...pendingWrites].map((accountId) => ({ accountId, sequence: 1,
        objectId: 'email-1', kind: 'mail', title: 'Offline-Nachricht aus redb', action: 'Mail ändern',
        transferable: true, requiresReview: heldWrites.has(accountId) })), ...(showLocalDraft ? [{ accountId: 'account',
          objectId: 'local-draft-1', kind: 'draft', title: 'Lokaler Testentwurf', action: 'Lokaler Entwurf', transferable: false, requiresReview: false }] : []), ...[...journalWrites.values()].map((row) => ({ ...row, requiresReview: row.status === 'held' }))];
      case 'queue_draft_sync': {
        expect(args).toEqual({ accountId: 'account', draftId: 'local-draft-1' });
        showLocalDraft = false;
        const journalId = ++nextJournalId;
        journalWrites.set(journalId, { journalId, accountId: 'account', objectId: args.draftId,
          kind: 'draft', title: 'Lokaler Testentwurf', action: 'Als Entwurf auf Stalwart speichern',
          status: 'held', transferable: true });
        return null;
      }
      case 'approve_pending_changes':
        for (const change of args.changes) heldWrites.delete(change.accountId);
        for (const change of args.changes) if (journalWrites.has(change.journalId)) journalWrites.get(change.journalId).status = 'ready';
        return null;
      case 'change_details': {
        const row = journalWrites.get(args.selection.journalId);
        if (!row || row.accountId !== args.selection.accountId) throw 'Auftrag wurde inzwischen geändert.';
        return { title: row.title, revision: `${row.journalId}:${row.status}`,
          fields: [{ label: row.kind === 'send' ? 'Betreff / Titel' : 'Name', current: row.kind === 'send' ? 'Versandstatus nicht automatisch geprüft' : 'Serverfassung.txt', desired: row.title }],
          warning: row.status === 'uncertain' ? 'Der Server kann diese Aktion bereits ausgeführt haben. Prüfe den Serverstand.' : 'Erneutes Vormerken wartet auf eine neue Freigabe.',
          canRetry: writable && row.status === 'blocked' && row.kind !== 'send',
          canDiscard: ['held', 'ready', 'blocked', 'uncertain'].includes(row.status), uncertain: row.status === 'uncertain' };
      }
      case 'resolve_change': {
        const row = journalWrites.get(args.selection.journalId);
        if (!row || args.expectedRevision !== `${row.journalId}:${row.status}`) throw 'Stand wurde inzwischen geändert.';
        if (args.action === 'retry') {
          if (!writable || row.status !== 'blocked' || row.kind === 'send') throw 'Wiederholung gesperrt.';
          journalWrites.delete(row.journalId);
          const journalId = ++nextJournalId;
          journalWrites.set(journalId, { ...row, journalId, status: 'held', blocked: undefined });
        } else {
          if (row.status === 'uncertain' && !args.acknowledgeUncertain) throw 'Serverstatus zuerst prüfen.';
          resolvedWrites.push(row);
          journalWrites.delete(row.journalId);
        }
        return null;
      }
      case 'refresh_desktop_session':
      case 'desktop_bootstrap': {
        if (command === 'refresh_desktop_session') {
          if (!serverAvailable) { for (const account of pendingWrites) heldWrites.add(account); throw new Error('Server offline'); }
          sharesDiscovered = true;
        }
        const info: any = { serverUrl: 'https://mail.example.test', username: 'reader@example.test', readOnly: !writable,
        session: { username: 'reader@example.test', state: 'fixture', apiUrl: 'https://mail.example.test/jmap', downloadUrl: 'https://mail.example.test/download/{accountId}/{blobId}/{name}?type={type}', uploadUrl: 'https://mail.example.test/upload/{accountId}',
          capabilities: { 'urn:ietf:params:jmap:core': { maxObjectsInGet: 500, maxCallsInRequest: 16 }, 'urn:ietf:params:jmap:mail': {}, 'urn:ietf:params:jmap:calendars': {}, 'urn:ietf:params:jmap:submission': {}, 'urn:ietf:params:jmap:filenode': {}, 'urn:ietf:params:jmap:contacts': {} },
          accounts: { account: { name: 'Desktop Test', isPersonal: true, isReadOnly: !writable, accountCapabilities: { 'urn:ietf:params:jmap:mail': {}, 'urn:ietf:params:jmap:calendars': {}, 'urn:ietf:params:jmap:submission': {}, 'urn:ietf:params:jmap:filenode': {}, 'urn:ietf:params:jmap:contacts': {} } } },
          primaryAccounts: { 'urn:ietf:params:jmap:mail': 'account', 'urn:ietf:params:jmap:calendars': 'account', 'urn:ietf:params:jmap:submission': 'account', 'urn:ietf:params:jmap:filenode': 'account' } } };
        if (sharesDiscovered) info.session.accounts.shared = { name: 'Teamfreigabe', isPersonal: false, isReadOnly: true, accountCapabilities: {} };
        return info;
      }
      case 'sync_contacts': if (!serverAvailable) throw new Error('Server offline'); syncedContacts.add(args.accountId); return null;
      case 'local_contacts': return { methodResponses: args.methodCalls.map(([method, argument, id]: [string, any, string]) => {
        const target = argument.accountId || args.accountId;
        const shared = target === 'shared';
        const contact = { id: 'contact-1', addressBookIds: { 'book-1': true }, name: { full: shared ? 'Team Kontakt' : 'Ada Lokal' },
          emails: { e: { address: shared ? 'team@example.test' : 'ada@example.test' } }, phones: { p: { number: '+49123456789' } } };
        if (method === 'AddressBook/get') return [method, { accountId: target, state: 'books-1', list: [{ id: 'book-1', name: shared ? 'Team-Adressbuch' : 'Lokales Adressbuch', isDefault: true, myRights: { mayRead: true, mayWrite: false, mayDelete: false, mayShare: false } }], notFound: [] }, id];
        if (method === 'ContactCard/query') return [method, { accountId: target, queryState: 'contacts-1', position: argument.position || 0, total: 1, canCalculateChanges: false, ids: (argument.position || 0) === 0 ? ['contact-1'] : [] }, id];
        if (method === 'ContactCard/get') return [method, { accountId: target, state: 'contacts-1', list: [contact], notFound: [] }, id];
        return ['error', { type: method.endsWith('/set') ? 'forbidden' : 'unknownMethod' }, id];
      }) };
      case 'local_calendar_snapshot': return {
        calendarState: 'calendar-fixture', eventState: 'event-fixture', identities: [],
        calendars: [{ id: 'calendar-1', name: args.accountId === 'shared' ? 'Team-Kalender' : 'Offline-Kalender', color: '#2563eb', sortOrder: 0,
          isSubscribed: true, isVisible: true, isDefault: true, timeZone: 'Europe/Belgrade',
          myRights: { mayReadItems: true, mayReadFreeBusy: true, mayWriteAll: true, mayWriteOwn: true, mayShare: true } }],
        events: { 'event-1': { id: 'event-1', '@type': 'Event', uid: invitationMode ? 'synthetic-invitation-1' : 'fixture-event', sequence: invitationStoredSequence, organizerCalendarAddress: 'mailto:sender@example.test', title: args.accountId === 'shared' ? 'Freigegebener Termin' : eventTitle,
          calendarIds: { 'calendar-1': true }, start: invitationApplied ? '2026-11-03T11:00:00' : '2026-11-03T12:00:00', duration: 'PT1H',
          timeZone: invitationApplied ? 'Etc/UTC' : 'Europe/Belgrade', utcStart: '2026-11-03T11:00:00Z', utcEnd: '2026-11-03T12:00:00Z',
          showWithoutTime: false, status: 'confirmed', recurrenceRule: null, recurrenceOverrides: null } },
      };
      case 'sync_calendar': if (!serverAvailable) throw new Error('Server offline'); syncedCalendars.add(args.accountId); return null;
      case 'sync_mailboxes': if (!serverAvailable) throw new Error('Server offline'); return null;
      case 'sync_mail': if (syncBarrier) await syncBarrier; if (failMailSync) throw new Error('Synthetic sync failure'); if (!serverAvailable) throw new Error('Server offline'); syncedMail.add(args.accountId); if (!heldWrites.has(args.accountId)) pendingWrites.delete(args.accountId); return null;
      case 'local_jmap': return { methodResponses: args.methodCalls.map(([method, argument, id]: [string, any, string]) => {
        const target = argument.accountId || args.accountId;
        const shared = target === 'shared';
        const scopedEmail = shared ? { ...email, subject: 'Nachricht aus Teamfreigabe', bodyValues: { '1': { value: 'Nur freigegebenes Postfach.', isTruncated: false } } } : { ...email, mailboxIds, keywords: { ...email.keywords, ...(flagged ? { $flagged: true } : {}) } };
        const scopedInbox = shared ? { ...inbox, name: 'Team-Postfach', role: null } : { ...inbox, myRights: { ...inbox.myRights, maySetSeen: writable, maySetKeywords: writable, mayAddItems: writable, mayRemoveItems: writable } };
        const base = { accountId: target, state: 'local:fixture' };
        const legacyDraftQuery = !shared && args.methodCalls.some(([name, value]: [string, any]) => name === 'Email/query' && value.filter?.inMailbox === 'drafts');
        const legacyDraft = { ...scopedEmail, id: 'local-draft-legacy', threadId: 'local-draft-legacy', subject: 'Alter Entwurf ohne Datum', mailboxIds: { drafts: true }, receivedAt: null, sentAt: null, keywords: { $draft: true } };
        switch (method) {
          case 'Email/set': {
            if (!writable || shared) return ['error', { type: 'forbidden' }, id];
            const patch = argument.update?.['email-1'];
            if (!patch) throw new Error('Unexpected write in fixture');
            if (Object.hasOwn(patch, 'keywords/$flagged')) flagged = patch['keywords/$flagged'] === true;
            else if (patch.mailboxIds) mailboxIds = patch.mailboxIds;
            else throw new Error('Unexpected patch in fixture');
            pendingWrites.add(target);
            if (!networkEnabled || !serverAvailable) heldWrites.add(target);
            return [method, { accountId: target, oldState: 'local:fixture', newState: 'local:edited', updated: { 'email-1': null }, notUpdated: {} }, id];
          }
          case 'Mailbox/query': return [method, { ...base, ids: ['inbox'], position: 0, total: 1, queryState: 'local:fixture', canCalculateChanges: false }, id];
          case 'Mailbox/get': return [method, { ...base, list: [scopedInbox, { ...scopedInbox, id: 'trash', name: 'Papierkorb', role: 'trash' }, { ...scopedInbox, id: 'drafts', name: 'Entwürfe', role: 'drafts' }, { ...scopedInbox, id: 'sent', name: 'Gesendet', role: 'sent' }], notFound: [] }, id];
          case 'Email/query': return [method, { ...base, ids: legacyDraftQuery ? ['local-draft-legacy'] : argument.filter?.inMailbox && !shared && !mailboxIds[argument.filter.inMailbox] ? [] : ['email-1'], position: 0, total: 1, queryState: 'local:fixture', canCalculateChanges: false }, id];
          case 'Email/get': return [method, { ...base, list: [legacyDraftQuery ? legacyDraft : argument.ids?.includes('local-draft-1') ?
            { ...scopedEmail, id: 'local-draft-1', subject: 'Lokaler Testentwurf', keywords: { $draft: true } } : scopedEmail], notFound: [] }, id];
          case 'Thread/get': return [method, { ...base, list: [{ id: 'thread-1', emailIds: ['email-1'] }], notFound: [] }, id];
          case 'Identity/get': return [method, { ...base, list: [], notFound: [] }, id];
          default: return ['error', { type: method.endsWith('/set') ? 'forbidden' : 'unknownMethod' }, id];
        }
      }) };
      default: throw new Error(`Unexpected native command: ${command}`);
    }
  });
  await page.addInitScript(() => {
    (window as any).isTauri = true;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    let nextId = 0;
    (window as any).emitNativeEvent = (event: string, payload: unknown) => {
      for (const [id, listener] of listeners) if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
    };
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event: string, id: number) => listeners.delete(id) };
    (window as any).__TAURI_INTERNALS__ = {
      transformCallback: (callback: (event: unknown) => void) => { const id = ++nextId; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: (command: string, args: any) => {
        if (command === 'plugin:event|listen') { const id = ++nextId; listeners.set(id, args); return Promise.resolve(id); }
        if (command === 'plugin:event|unlisten') return Promise.resolve();
        return (window as any).nativeFixture(command, args);
      },
      convertFileSrc: () => `${window.location.origin}/plugin-sandbox/index.html` };

  });
  await page.goto('/de/');
  await expect(page.getByText('Offline-Nachricht aus redb').first()).toBeVisible();
  await page.getByText('Entwürfe', { exact: true }).first().click();
  await expect(page.getByText('Alter Entwurf ohne Datum', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('E-Mails können nicht geladen werden', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Posteingang', exact: true }).first().click();
  await expect(page.getByText('Offline-Nachricht aus redb').first()).toBeVisible();
  await expect(page.getByText(writable ? 'Test-Schreibmodus' : 'Nur Lesen', { exact: false })).toBeVisible();
  // Adding an account is cancellable before any native account write, including on a small window.
  for (const dismiss of ['cancel', 'close', 'escape']) {
    await page.setViewportSize({ width: 640, height: 480 });
    await page.getByText('Profile & Konten', { exact: true }).click();
    await page.getByRole('button', { name: 'Konto hinzufügen', exact: true }).click();
    const setup = page.getByRole('dialog', { name: 'Konto hinzufügen', exact: true });
    await expect(setup).toBeVisible();
    await expect(setup.getByLabel('Benutzername', { exact: true })).toHaveValue('');
    await setup.getByLabel('Benutzername', { exact: true }).fill('unsaved@example.test');
    await setup.getByLabel('Passwort', { exact: true }).fill('synthetic-unsaved');
    if (dismiss === 'cancel') await setup.getByRole('button', { name: 'Abbrechen', exact: true }).click();
    else if (dismiss === 'close') await setup.getByRole('button', { name: 'Kontoeinrichtung schließen', exact: true }).click();
    else await page.keyboard.press('Escape');
    await expect(setup).toHaveCount(0);
    expect(calls.filter((command) => ['setup_account', 'add_profile_account'].includes(command))).toHaveLength(0);
  }
  await page.setViewportSize({ width: 1440, height: 960 });

  await page.getByText('Offline-Nachricht aus redb', { exact: true }).click();
  await expect(page.getByText('Lokal verfügbarer Testinhalt.', { exact: true })).toBeVisible();
  await expect(page.getByText('Synthetische Einladung', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Einladung konnte nicht gelesen werden', { exact: true })).toHaveCount(0);
  // The footer reserves its own row, including when the viewport is narrow.
  const footer = page.getByRole('contentinfo', { name: 'Desktop-Status' });
  const workspace = page.locator('.desktop-workspace');
  for (const size of [{ width: 1440, height: 960 }, { width: 640, height: 480 }]) {
    await page.setViewportSize(size);
    const footerBox = (await footer.boundingBox())!;
    const workspaceBox = (await workspace.boundingBox())!;
    expect(workspaceBox.y + workspaceBox.height).toBeLessThanOrEqual(footerBox.y);
  }
  await page.setViewportSize({ width: 1440, height: 960 });
  const networkButton = page.locator('.desktop-network-toggle');
  await expect(networkButton).toHaveText('Online · Verbunden');
  await page.clock.install();
  const beforeIdle = calls.filter((command) => command === 'sync_mail').length;
  await page.clock.runFor(65_000);
  await expect(networkButton).toHaveText('Online \u00b7 Verbunden');
  expect(calls.filter((command) => command === 'sync_mail').length).toBe(beforeIdle);
  await page.evaluate(() => {
    for (let index = 0; index < 5; index++) (window as any).emitNativeEvent('jmap-push', null);
  });
  await page.clock.runFor(800);
  await expect.poll(() => calls.filter((command) => command === 'sync_mail').length).toBe(beforeIdle + 2);
  await expect(networkButton).toHaveText('Online \u00b7 Verbunden');
  await page.evaluate(() => (window as any).emitNativeEvent('jmap-push-status', false));
  await page.clock.runFor(60_100);
  await expect.poll(() => calls.filter((command) => command === 'sync_mail').length).toBe(beforeIdle + 4);
  await expect(networkButton).toHaveText('Online \u00b7 Verbunden');
  await page.evaluate(() => (window as any).emitNativeEvent('jmap-push-status', true));
  await expect.poll(() => calls.filter((command) => command === 'sync_mail').length).toBe(beforeIdle + 6);
  await expect(networkButton).toHaveText('Online \u00b7 Verbunden');
  let releaseSync!: () => void;
  syncBarrier = new Promise<void>((resolve) => { releaseSync = resolve; });
  await page.evaluate(() => window.dispatchEvent(new Event('desktop-sync-requested')));
  await expect(networkButton).toHaveText('Sync · Nachrichten 1/2');
  await networkButton.click();
  await expect(networkButton).toHaveText('Offline · Nur lokal');
  const offlineSendError = await page.evaluate(async () => {
    try {
      await (globalThis as any).__bulwarkOfflineHooks.answer({ getAccountId: () => 'account' }, [['EmailSubmission/set', { accountId: 'account', create: { '1': { emailId: 'synthetic' } } }, 'send']]);
      return 'unexpected-success';
    } catch (error) { return error instanceof Error ? error.name : 'not-an-error'; }
  });
  expect(offlineSendError).toBe('Error'); // Raw submissions cannot bypass the durable desktop send boundary.

  syncBarrier = null;
  releaseSync();
  await expect(page.getByRole('status').first()).toContainText('Offline');
  await networkButton.click();
  await expect(networkButton).toHaveText('Online · Verbunden');
  const cleanupResult = await page.evaluate(async () => (globalThis as any).__bulwarkOfflineHooks.answer(
    { getAccountId: () => 'account' }, [['EmailSubmission/set', { accountId: 'account', destroy: ['synthetic-receipt'] }, 'cleanup']]));
  expect(cleanupResult.methodResponses[0]).toEqual(['EmailSubmission/set', { accountId: 'account', destroyed: [] }, 'cleanup']);
  failMailSync = true;
  await page.evaluate(() => window.dispatchEvent(new Event('desktop-sync-requested')));
  await expect(page.getByRole('status').first()).toContainText('Abgleich fehlgeschlagen');
  await expect(networkButton).toHaveText('Online · Verbunden');
  failMailSync = false;
  await page.evaluate(() => window.dispatchEvent(new Event('desktop-sync-requested')));
  await expect(page.getByRole('status').first()).toContainText('Abgleich abgeschlossen');
  await page.getByTitle('Antworten', { exact: true }).first().click();
  const sendButton = page.locator('[data-testid="composer-send"]:visible').first();
  await expect(sendButton).toBeVisible();
  const sendBox = (await sendButton.boundingBox())!;
  const statusBox = (await footer.boundingBox())!;
  expect(sendBox.y + sendBox.height).toBeLessThanOrEqual(statusBox.y);
  await page.screenshot({ path: `.cache/composer-statusbar-${writable ? 'write' : 'read'}.png` });
  await page.locator('[data-testid="composer-close"]:visible').first().click();
  if (writable) {
    showLocalDraft = true;
    await page.evaluate(() => window.dispatchEvent(new Event('desktop-changes-updated')));
    await expect(page.locator('.desktop-changes-toggle')).toHaveText('Keine ausstehenden Aktionen');
    await page.locator('.desktop-changes-toggle').click();
    const drafts = page.getByRole('dialog', { name: 'Lokale Änderungen übertragen?' });
    await expect(drafts.getByRole('checkbox')).toBeDisabled();
    await expect(drafts).toHaveCSS('background-color', 'rgb(255, 255, 255)');
    await drafts.getByRole('button', { name: 'Öffnen' }).click();
    await expect(sendButton).toBeVisible();
    await expect(page.locator('input[value="Lokaler Testentwurf"]')).toBeVisible();
    await page.locator('[data-testid="composer-close"]:visible').first().click();
    await page.locator('.desktop-changes-toggle').click();
    await drafts.getByRole('button', { name: 'Stalwart-Entwurf vormerken' }).click();
    const stagedDraft = drafts.getByRole('checkbox', { name: /Als Entwurf auf Stalwart speichern/ });
    await expect(stagedDraft).toBeEnabled();
    expect(calls).not.toContain('transport:draft');
    await stagedDraft.check();
    await drafts.getByRole('button', { name: 'Auswahl übertragen (1)' }).click();
    await expect.poll(() => calls.includes('transport:draft')).toBe(true);
    await drafts.getByRole('button', { name: 'Später' }).click();
    await page.getByText('Offline-Nachricht aus redb', { exact: true }).first().click();
  }
  if (writable) {
    serverAvailable = false;
    await page.getByRole('button', { name: 'Stern hinzufügen', exact: true }).first().click();
    await expect(page.getByRole('button', { name: 'Stern entfernen', exact: true }).first()).toBeVisible();
    await expect.poll(() => pendingWrites.size).toBe(1);
    await expect(page.locator('.desktop-status')).toContainText('1 ausstehend');
    serverAvailable = true;
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    const review = page.getByRole('dialog', { name: 'Lokale Änderungen übertragen?' });
    await expect(review).toBeVisible();
    expect(pendingWrites.size).toBe(1);
    await page.screenshot({ path: '.cache/pending-changes-review.png' });
    await review.getByRole('button', { name: 'Später' }).click();

    await page.locator('.desktop-changes-toggle').click();
    await review.getByRole('checkbox').check();
    await review.getByRole('button', { name: 'Auswahl übertragen (1)' }).click();
    await expect.poll(() => pendingWrites.size).toBe(0);
    await review.getByRole('button', { name: 'Später' }).click();
    expect(flagged).toBe(true);
    serverAvailable = false;
    await page.getByText('Offline-Nachricht aus redb', { exact: true }).first().click();
    await page.locator('button[title="L\u00f6schen"]:visible').first().click({ timeout: 5000 });
    await expect.poll(() => mailboxIds).toEqual({ trash: true });
    await expect(page.locator('.desktop-status')).toContainText('1 ausstehend');
    await page.reload();
    await page.getByText('Papierkorb', { exact: true }).first().click();
    await expect(page.getByText('Offline-Nachricht aus redb', { exact: true }).first()).toBeVisible();
    serverAvailable = true;
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    await expect(review).toBeVisible();
    expect(pendingWrites.size).toBe(1);
    await review.getByRole('checkbox').check();
    await review.getByRole('button', { name: 'Auswahl übertragen (1)' }).click();
    await expect.poll(() => pendingWrites.size).toBe(0);
    await review.getByRole('button', { name: 'Später' }).click();
  }
  await page.screenshot({ path: '.cache/bulwark-mail.png', fullPage: true });
  await expect(page.getByTestId('section-shared')).toBeVisible();
  await page.getByTestId('section-shared').click();
  await page.getByText('Teamfreigabe', { exact: true }).first().click();
  await page.getByText('Team-Postfach', { exact: true }).click();
  await expect(page.getByText('Nachricht aus Teamfreigabe', { exact: true }).first()).toBeVisible();
  await page.getByText('Nachricht aus Teamfreigabe', { exact: true }).first().click();
  await expect(page.getByText('Nur freigegebenes Postfach.', { exact: true })).toBeVisible();
  await expect(page.getByText('Lokal verfügbarer Testinhalt.', { exact: true })).not.toBeVisible();
  await expect(page.getByRole('link', { name: 'Kalender', exact: true })).toBeVisible();
  await page.getByRole('link', { name: 'Kalender', exact: true }).click();
  await expect(page.getByText('Offline-Kalender', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Offline-Kalendertermin', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Team-Kalender', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Freigegebener Termin', { exact: true }).first()).toBeVisible();
  await page.screenshot({ path: '.cache/bulwark-calendar.png', fullPage: true });
  await page.goto('/de/contacts');
  await expect(page.getByText('Ada Lokal', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Team Kontakt', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Lokales Adressbuch', { exact: true }).first()).toBeVisible();
  await expect(page.locator('.desktop-contacts-notice')).toContainText('Direkte Bearbeitung');
  await expect(page.getByRole('button', { name: 'Google-Kontakte übernehmen', exact: true })).toHaveCount(0);
  const contactSearchBox = page.getByRole('searchbox', { name: /Kontakte durchsuchen|Kontakte suchen/ }).first();
  await contactSearchBox.fill('ada');
  await expect(page.getByText('Ada Lokal', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Team Kontakt', { exact: true })).toHaveCount(0);
  await contactSearchBox.fill('');
  await page.getByText('Ada Lokal', { exact: true }).first().click();
  await expect(page.getByText('ada@example.test', { exact: true }).first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Kontakt bearbeiten', exact: true })).toBeDisabled();
  const contactWrite = await page.evaluate(async () => (globalThis as any).__bulwarkOfflineHooks.answer(
    { getAccountId: () => 'account' }, [['ContactCard/set', { accountId: 'account', destroy: ['contact-1'] }, 'blocked-contact-write']]));
  expect(contactWrite.methodResponses[0][1].type).toBe('forbidden');
  await page.screenshot({ path: `.cache/contacts-${writable ? 'write' : 'read'}.png` });
  await page.goto('/de/settings/plugins');
  await page.getByText('Offline-Speicher', { exact: false }).click();
  const personalCache = page.locator('.desktop-cache-account').filter({ hasText: 'Personal cache' });
  await expect(personalCache).toContainText('40 MiB von 256 MiB');
  await personalCache.getByLabel('Limit in MiB').fill('25');
  await expect(personalCache).toContainText('sofort entfernt');
  await personalCache.getByRole('button', { name: 'Limit speichern', exact: true }).click();
  await expect(personalCache.getByRole('status')).toHaveText('Limit gespeichert.');
  await page.reload();
  await page.getByText('Offline-Speicher', { exact: false }).click();
  await expect(personalCache.getByLabel('Limit in MiB')).toHaveValue('25');
  await expect(page.locator('.desktop-cache-account').filter({ hasText: 'Shared cache' }).getByLabel('Limit in MiB')).toHaveValue('256');
  await page.setViewportSize({ width: 640, height: 900 });
  await page.screenshot({ path: `.cache/cache-settings-${writable ? 'write' : 'read'}.png` });
  await page.setViewportSize({ width: 1440, height: 960 });
  await page.getByText('Offline-Speicher', { exact: false }).click();
  await page.goto('/de/settings/plugins');

  await expect(page.locator('input[type="file"]')).toBeAttached();
  const zip = new JSZip();
  zip.file('manifest.json', JSON.stringify({ id: 'desktop-test', name: 'Desktop Test Add-on', version: '1.0.0',
    author: 'Desktop Test', description: 'Lokaler Integrationstest', type: 'hook', permissions: [], entrypoint: 'index.js' }));
  zip.file('index.js', 'module.exports = { activate(api) { api.storage.set("started", true); }, deactivate() {} };');
  await page.locator('input[type="file"]').setInputFiles({ name: 'desktop-test.zip', mimeType: 'application/zip', buffer: await zip.generateAsync({ type: 'nodebuffer' }) });
  await expect(page.getByText('Desktop Test Add-on', { exact: true })).toBeVisible();
  expect(values.has('plugins/code/desktop-test')).toBeTruthy();
  await page.getByRole('switch', { name: /Desktop Test Add-on/ }).click();
  await page.getByRole('button', { name: /Desktop Test Add-on/ }).click();
  await expect.poll(() => [...values.entries()].some(([key, value]) => key.startsWith('ui/plugin:desktop-test:') && key.endsWith(':started') && value === 'true')).toBeTruthy();
  // Model the native close handshake: flush Bulwark's batched stores and wait for redb writes.
  await page.evaluate(() => window.dispatchEvent(new Event('pagehide')));
  await expect.poll(() => {
    const saved = values.get('ui/plugin-storage');
    return saved ? JSON.parse(saved).state.plugins.some((plugin: any) => plugin.id === 'desktop-test' && plugin.enabled) : false;
  }).toBeTruthy();
  serverAvailable = false;
  await page.reload();
  await expect(page.getByText('Desktop Test Add-on', { exact: true })).toBeVisible();
  await expect(page.getByRole('switch', { name: /Desktop Test Add-on/ })).toBeChecked();
  await page.screenshot({ path: '.cache/bulwark-plugins.png', fullPage: true });
  await page.goto('/de/calendar');
  await expect(page.getByText('Offline-Kalendertermin', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Freigegebener Termin', { exact: true }).first()).toBeVisible();
  await page.goto('/de/contacts');
  await expect(page.getByText('Ada Lokal', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Team Kontakt', { exact: true }).first()).toBeVisible();
  await page.reload();
  await expect(page.getByText('Ada Lokal', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Team Kontakt', { exact: true }).first()).toBeVisible();
  expect([...syncedContacts].sort()).toEqual(['account', 'shared']);
  expect([...syncedMail].sort()).toEqual(['account', 'shared']);
  expect([...syncedCalendars].sort()).toEqual(['account', 'shared']);
  if (writable) {
    // The desktop adapter accepts a real original-composer Send while offline.
    await page.goto('/de/mail');
    await page.getByText('Papierkorb', { exact: true }).first().click();
    await page.getByText('Offline-Nachricht aus redb', { exact: true }).first().click();
    await page.evaluate(() => window.dispatchEvent(new Event('offline')));
    await page.getByTitle('Antworten', { exact: true }).first().click();
    await page.locator('[contenteditable="true"]:visible').last().fill('Dauerhaft gespeicherter Offline-Versandtest.');
    await page.locator('input[type="file"]').last().setInputFiles({ name: 'lokaler-anhang.bin', mimeType: 'application/octet-stream', buffer: attachmentBytes });
    await expect.poll(() => stagedUploads.size).toBe(1);
    await expect(page.getByText('lokaler-anhang.bin', { exact: true }).last()).toBeVisible();
    await page.locator('[data-testid="composer-send"]:visible').first().click();
    await expect.poll(() => [...journalWrites.values()].filter((row) => row.kind === 'send').length).toBe(1);
    expect(calls.filter((call) => call === 'transport:send')).toHaveLength(0);
    const queuedMail = [...journalWrites.values()].find((row) => row.kind === 'send').methodCalls[0][1].create;
    expect(JSON.stringify(queuedMail)).toContain('local-upload-1');
    expect(Buffer.from([...stagedUploads.values()][0].base64, 'base64')).toEqual(attachmentBytes);
    const stagedRead = await page.evaluate(async () => {
      const blob = await (globalThis as any).__bulwarkOfflineHooks.blob({}, 'local-upload-1', 'account');
      return [...new Uint8Array(await blob.arrayBuffer())];
    });
    expect(stagedRead).toEqual([...attachmentBytes]);
    // Calendar and file writes use the same desktop journal, without a server call.
    await page.evaluate(async () => {
      const hooks = (globalThis as any).__bulwarkOfflineHooks;
      const client = { getAccountId: () => 'account' };
      const calendar = await hooks.answer(client, [['CalendarEvent/set', { accountId: 'account', update: { 'event-1': { title: 'Offline geänderter Termin' } } }, 'calendar-write']]);
      if (calendar.methodResponses[0][0] === 'error') throw new Error('Calendar offline write rejected');
      const file = await hooks.answer(client, [['FileNode/set', { accountId: 'account', update: { 'file-1': { name: 'Offline umbenannt.txt' } } }, 'file-write']]);
      if (file.methodResponses[0][0] === 'error') throw new Error('File offline write rejected');
    });
    expect(journalWrites.size).toBe(3);
    await page.reload();
    await expect(page.locator('.desktop-changes-toggle')).toHaveText('3 ausstehende Aktionen');
    expect(calls.filter((call) => call.startsWith('transport:') && call !== 'transport:draft')).toHaveLength(0);
    serverAvailable = true;
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    const review = page.getByRole('dialog', { name: 'Lokale Änderungen übertragen?' });
    await expect(review).toBeVisible();
    await expect(review.getByText('Offline geänderter Termin', { exact: true })).toBeVisible();
    expect(journalWrites.size).toBe(3);
    await page.screenshot({ path: '.cache/offline-actions-review.png' });
    const sendRow = review.locator('.desktop-change').filter({ hasText: 'Mail versenden' });
    await sendRow.getByRole('checkbox').check();
    await review.getByRole('button', { name: 'Auswahl übertragen (1)' }).click();
    await expect.poll(() => calls.filter((call) => call === 'transport:send').length).toBe(1);
    expect(journalWrites.size).toBe(2);
    for (const checkbox of await review.getByRole('checkbox').all()) await checkbox.check();
    await review.getByRole('button', { name: 'Auswahl übertragen (2)' }).click();
    await expect.poll(() => journalWrites.size).toBe(0);
    expect(calls.filter((call) => call === 'transport:calendar')).toHaveLength(1);
    expect(calls.filter((call) => call === 'transport:file')).toHaveLength(1);
    await review.getByRole('button', { name: 'Später' }).click();
    // Resolve a conflict locally, then close an explicitly checked uncertain
    // send. Neither action may start a transport or reuse a previous approval.
    const blockedId = ++nextJournalId;
    journalWrites.set(blockedId, { journalId: blockedId, accountId: 'account', objectId: 'file-1', kind: 'file', title: 'Konfliktdatei.txt', action: 'Umbenennen', status: 'blocked', blocked: 'stateMismatch', transferable: true });
    const uncertainId = ++nextJournalId;
    journalWrites.set(uncertainId, { journalId: uncertainId, accountId: 'account', objectId: 'local-send-uncertain', kind: 'send', title: 'Unklare Sendung', action: 'Mail versenden', status: 'uncertain', blocked: 'Serverantwort unklar', transferable: true });
    const transportsBeforeResolution = calls.filter((call) => call.startsWith('transport:')).length;
    await page.evaluate(() => window.dispatchEvent(new Event('desktop-changes-updated')));
    await page.locator('.desktop-changes-toggle').click();
    await review.locator('.desktop-change').filter({ hasText: 'Konfliktdatei.txt' }).getByRole('button', { name: 'Prüfen', exact: true }).click();
    const inspection = review.getByRole('region', { name: 'Aktion prüfen' });
    await expect(inspection.getByRole('cell', { name: 'Serverfassung.txt', exact: true })).toBeVisible();
    await expect(inspection.getByRole('cell', { name: 'Konfliktdatei.txt', exact: true })).toBeVisible();
    await page.screenshot({ path: '.cache/change-conflict-desktop.png' });
    await page.setViewportSize({ width: 640, height: 800 });
    await page.screenshot({ path: '.cache/change-conflict-narrow.png' });
    expect(await inspection.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.setViewportSize({ width: 1440, height: 960 });
    await inspection.getByRole('button', { name: 'Erneut vormerken', exact: true }).click();
    await expect.poll(() => [...journalWrites.values()].find((row) => row.kind === 'file')?.status).toBe('held');
    expect(calls.filter((call) => call.startsWith('transport:')).length).toBe(transportsBeforeResolution);
    await review.locator('.desktop-change').filter({ hasText: 'Konfliktdatei.txt' }).getByRole('button', { name: 'Prüfen', exact: true }).click();
    await expect(inspection.getByRole('button', { name: 'Lokale Absicht verwerfen', exact: true })).toBeDisabled();
    await inspection.getByRole('checkbox').check();
    await inspection.getByRole('button', { name: 'Lokale Absicht verwerfen', exact: true }).click();
    await expect.poll(() => journalWrites.size).toBe(1);
    await review.locator('.desktop-change').filter({ hasText: 'Unklare Sendung' }).getByRole('button', { name: 'Prüfen', exact: true }).click();
    await expect(inspection.getByRole('button', { name: 'Erneut vormerken', exact: true })).toHaveCount(0);
    await expect(inspection.getByRole('button', { name: 'Lokal abschließen', exact: true })).toBeDisabled();
    await page.screenshot({ path: '.cache/change-uncertain-send.png' });
    await inspection.getByRole('checkbox').check();
    await inspection.getByRole('button', { name: 'Lokal abschließen', exact: true }).click();
    await expect.poll(() => journalWrites.size).toBe(0);
    expect(resolvedWrites).toHaveLength(2);
    expect(calls.filter((call) => call.startsWith('transport:')).length).toBe(transportsBeforeResolution);
    await review.getByRole('button', { name: 'Später', exact: true }).click();
    await page.reload();
    await expect(page.locator('.desktop-changes-toggle')).toHaveText('Keine ausstehenden Aktionen');
  }
  // A real invitation banner, native snapshot and durable queue, with no server writes.
  invitationMode = true;
  await page.goto('/de/');
  await page.getByText('Offline-Nachricht aus redb', { exact: true }).click();
  await expect(page.locator('.desktop-invitation-update')).toContainText('neuere Version');
  const takeUpdate = page.getByRole('button', { name: 'Terminangaben übernehmen', exact: true });
  if (!writable) {
    await expect(takeUpdate).toHaveCount(0);
  } else {
    await page.locator('.desktop-network-toggle').click();
    await expect(page.locator('.desktop-network-toggle')).toHaveText('Offline · Nur lokal');
    const transports = calls.filter((call) => call.startsWith('transport:')).length;
    await takeUpdate.click();
    await expect(page.locator('.desktop-invitation-update')).toContainText('inzwischen geändert');
    expect(invitationWrites).toHaveLength(0);
    await takeUpdate.click();
    await expect(page.locator('.desktop-invitation-update')).toContainText('lokal vorgemerkt');
    await expect(page.locator('.desktop-invitation-update')).toContainText('entspricht dem lokalen Kalenderstand');
    await expect(page.locator('.desktop-invitation-update')).not.toContainText('widersprechen');
    expect(invitationWrites).toHaveLength(1);
    expect(invitationWrites[0].sendSchedulingMessages).toBe(false);
    expect(invitationWrites[0].update['event-1'].sequence).toBe(2);
    expect(invitationWrites[0].update['event-1']).not.toHaveProperty('participants');
    expect(calls.filter((call) => call.startsWith('transport:')).length).toBe(transports);
    await expect(takeUpdate).toHaveCount(0);
    await page.screenshot({ path: '.cache/invitation-update-queued.png', fullPage: true });
    invitationStoredSequence = 3;
    await page.evaluate(() => window.dispatchEvent(new Event('desktop-changes-updated')));
    await expect(page.locator('.desktop-invitation-update')).toContainText('Einladung ist veraltet');
  }
  expect(calls).toContain('local_jmap');
  expect(errors).toEqual([]);
});
}

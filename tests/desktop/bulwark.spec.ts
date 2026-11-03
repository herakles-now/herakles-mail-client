import { test, expect } from '@playwright/test';
import JSZip from 'jszip';

for (const writable of [false, true]) {
test(`original Bulwark mail and plugins use native adapters (${writable ? 'writable' : 'read-only'})`, async ({ page }) => {
  await page.clock.setFixedTime(new Date('2026-11-03T10:00:00Z'));
  const values = new Map<string, string>();
  let serverAvailable = true;
  let sharesDiscovered = false;
  const syncedMail = new Set<string>();
  const syncedCalendars = new Set<string>();
  let flagged = false;
  const pendingWrites = new Set<string>();
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
      keywords: { $seen: true }, mailboxIds: { inbox: true }, receivedAt: '2026-11-03T10:00:00Z', size: 100,
      preview: 'Diese Nachricht kommt aus dem lokalen Repository.', hasAttachment: false,
      textBody: [{ partId: '1', type: 'text/plain' }], htmlBody: [], attachments: [],
      bodyValues: { '1': { value: 'Lokal verfügbarer Testinhalt.', isTruncated: false } } };
    const inbox = { id: 'inbox', name: 'Inbox', role: 'inbox', parentId: null, sortOrder: 0,
      totalEmails: 1, unreadEmails: 0, totalThreads: 1, unreadThreads: 0, isSubscribed: true,
      myRights: { mayReadItems: true, mayAddItems: false, mayRemoveItems: false, maySetSeen: false,
        maySetKeywords: false, mayCreateChild: false, mayRename: false, mayDelete: false, maySubmit: false } };
    switch (command) {
      case 'desktop_kv_keys': return [...values.keys()].filter((key) => key.startsWith(args.prefix));
      case 'desktop_kv_get': return values.get(args.key) ?? null;
      case 'desktop_kv_put': values.set(args.key, args.value); return null;
      case 'desktop_kv_delete': values.delete(args.key); return null;
      case 'write_status': return { pending: pendingWrites.size, blocked: 0 };
      case 'refresh_desktop_session':
      case 'desktop_bootstrap': {
        if (command === 'refresh_desktop_session') {
          if (!serverAvailable) throw new Error('Server offline');
          sharesDiscovered = true;
        }
        const info: any = { serverUrl: 'https://mail.example.test', username: 'reader@example.test', readOnly: !writable,
        session: { username: 'reader@example.test', state: 'fixture', apiUrl: 'https://mail.example.test/jmap',
          capabilities: { 'urn:ietf:params:jmap:core': { maxObjectsInGet: 500, maxCallsInRequest: 16 }, 'urn:ietf:params:jmap:mail': {}, 'urn:ietf:params:jmap:calendars': {} },
          accounts: { account: { name: 'Desktop Test', isPersonal: true, isReadOnly: !writable, accountCapabilities: { 'urn:ietf:params:jmap:mail': {}, 'urn:ietf:params:jmap:calendars': {} } } },
          primaryAccounts: { 'urn:ietf:params:jmap:mail': 'account', 'urn:ietf:params:jmap:calendars': 'account' } } };
        if (sharesDiscovered) info.session.accounts.shared = { name: 'Teamfreigabe', isPersonal: false, isReadOnly: true, accountCapabilities: {} };
        return info;
      }
      case 'local_calendar_snapshot': return {
        calendarState: 'calendar-fixture', eventState: 'event-fixture', identities: [],
        calendars: [{ id: 'calendar-1', name: args.accountId === 'shared' ? 'Team-Kalender' : 'Offline-Kalender', color: '#2563eb', sortOrder: 0,
          isSubscribed: true, isVisible: true, isDefault: true, timeZone: 'Europe/Belgrade',
          myRights: { mayReadItems: true, mayReadFreeBusy: true, mayWriteAll: true, mayWriteOwn: true, mayShare: true } }],
        events: { 'event-1': { id: 'event-1', '@type': 'Event', uid: 'fixture-event', title: args.accountId === 'shared' ? 'Freigegebener Termin' : 'Offline-Kalendertermin',
          calendarIds: { 'calendar-1': true }, start: '2026-11-03T12:00:00', duration: 'PT1H',
          timeZone: 'Europe/Belgrade', utcStart: '2026-11-03T11:00:00Z', utcEnd: '2026-11-03T12:00:00Z',
          showWithoutTime: false, status: 'confirmed', recurrenceRule: null, recurrenceOverrides: null } },
      };
      case 'sync_calendar': if (!serverAvailable) throw new Error('Server offline'); syncedCalendars.add(args.accountId); return null;
      case 'sync_mailboxes': if (!serverAvailable) throw new Error('Server offline'); return null;
      case 'sync_mail': if (!serverAvailable) throw new Error('Server offline'); syncedMail.add(args.accountId); pendingWrites.delete(args.accountId); return null;
      case 'local_jmap': return { methodResponses: args.methodCalls.map(([method, argument, id]: [string, any, string]) => {
        const target = argument.accountId || args.accountId;
        const shared = target === 'shared';
        const scopedEmail = shared ? { ...email, subject: 'Nachricht aus Teamfreigabe', bodyValues: { '1': { value: 'Nur freigegebenes Postfach.', isTruncated: false } } } : { ...email, keywords: { ...email.keywords, ...(flagged ? { $flagged: true } : {}) } };
        const scopedInbox = shared ? { ...inbox, name: 'Team-Postfach', role: null } : { ...inbox, myRights: { ...inbox.myRights, maySetSeen: writable, maySetKeywords: writable } };
        const base = { accountId: target, state: 'local:fixture' };
        switch (method) {
          case 'Email/set': {
            if (!writable || shared) return ['error', { type: 'forbidden' }, id];
            const patch = argument.update?.['email-1'];
            if (!patch || !Object.hasOwn(patch, 'keywords/$flagged')) throw new Error('Unexpected write in fixture');
            flagged = patch['keywords/$flagged'] === true;
            pendingWrites.add(target);
            return [method, { accountId: target, oldState: 'local:fixture', newState: 'local:edited', updated: { 'email-1': null }, notUpdated: {} }, id];
          }
          case 'Mailbox/query': return [method, { ...base, ids: ['inbox'], position: 0, total: 1, queryState: 'local:fixture', canCalculateChanges: false }, id];
          case 'Mailbox/get': return [method, { ...base, list: [scopedInbox], notFound: [] }, id];
          case 'Email/query': return [method, { ...base, ids: ['email-1'], position: 0, total: 1, queryState: 'local:fixture', canCalculateChanges: false }, id];
          case 'Email/get': return [method, { ...base, list: [scopedEmail], notFound: [] }, id];
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
    (window as any).__TAURI_INTERNALS__ = { invoke: (command: string, args: object) => (window as any).nativeFixture(command, args),
      convertFileSrc: () => `${window.location.origin}/plugin-sandbox/index.html` };
  });
  await page.goto('/de/');
  await expect(page.getByText('Offline-Nachricht aus redb').first()).toBeVisible();
  await expect(page.getByText(writable ? 'Test-Schreibmodus' : 'Nur Lesen', { exact: false })).toBeVisible();
  await page.getByText('Offline-Nachricht aus redb', { exact: true }).click();
  await expect(page.getByText('Lokal verfügbarer Testinhalt.', { exact: true })).toBeVisible();
  if (writable) {
    serverAvailable = false;
    await page.getByRole('button', { name: 'Stern hinzufügen', exact: true }).first().click();
    await expect(page.getByRole('button', { name: 'Stern entfernen', exact: true }).first()).toBeVisible();
    await expect.poll(() => pendingWrites.size).toBe(1);
    await expect(page.getByText('1 ausstehend', { exact: false })).toBeVisible();
    serverAvailable = true;
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    await expect.poll(() => pendingWrites.size).toBe(0);
    expect(flagged).toBe(true);
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
  expect([...syncedMail].sort()).toEqual(['account', 'shared']);
  expect([...syncedCalendars].sort()).toEqual(['account', 'shared']);
  expect(calls).toContain('local_jmap');
  expect(errors).toEqual([]);
});
}

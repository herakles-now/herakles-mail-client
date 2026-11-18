import { test, expect } from '@playwright/test';
import { build } from 'esbuild';
import { readFile } from 'node:fs/promises';

let bundle = '';
test.beforeAll(async () => {
  const result = await build({
    stdin: { contents: `import React from 'react'; import { createRoot } from 'react-dom/client'; import { GoogleContacts } from './src/desktop/GoogleContacts'; createRoot(document.getElementById('root')).render(React.createElement(GoogleContacts));`, resolveDir: process.cwd(), loader: 'tsx' },
    bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic',
    define: { 'process.env.NODE_ENV': '"production"' },
    plugins: [{ name: 'native-boundary', setup(builder) {
      builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'ipc', namespace: 'mock' }));
      builder.onResolve({ filter: /^\.\/google-contact-hooks$/ }, () => ({ path: 'hooks', namespace: 'mock' }));
      builder.onResolve({ filter: /^\.\/bootstrap$/ }, () => ({ path: 'bootstrap', namespace: 'mock' }));
      builder.onLoad({ filter: /.*/, namespace: 'mock' }, ({ path }) => ({ contents: path === 'hooks' ? 'export const allowGoogleContact = async () => true; export const notifyGoogleContacts = async () => {};' : path === 'ipc' ? 'export const invoke = (...args) => window.nativeInvoke(...args);' : 'export const refreshLocalViews = async () => {};' }));
    } }],
  });
  bundle = result.outputFiles[0].text;
});
for (const readOnly of [false, true]) {
  test(`Google import previews duplicates and queues only explicit selection (${readOnly ? 'read-only' : 'writable'})`, async ({ page }) => {
    const calls: { command: string; args: any }[] = [];
    let staged = false;
    await page.exposeFunction('nativeInvoke', async (command: string, args: any) => {
      calls.push({ command, args });
      if (command === 'google_import_status') return { connected: true, email: 'reader@example.test', fetchedAt: 1794900000, count: 3, readOnly, books: [{ accountId: 'contacts', id: 'book', name: 'Persönliche Kontakte', accountName: 'Stalwart' }] };
      if (command === 'google_import_preview') return { revision: 'revision-1', readOnly, rows: [
        { resource: 'people/new', title: 'Alice Example', status: staged ? 'unchanged' : 'new', reason: 'Neuer Kontakt', current: null, incoming: { name: { full: 'Alice Example' }, emails: { a: { address: 'alice@example.test' } } } },
        { resource: 'people/duplicate', title: 'Bob Example', status: 'duplicate', reason: 'Mögliche Dublette', current: null, incoming: { name: { full: 'Bob Example' } } },
        { resource: 'people/conflict', title: 'Charlie Google', status: 'conflict', reason: 'Google und Stalwart geändert', current: { name: { full: 'Charlie Lokal' } }, incoming: { name: { full: 'Charlie Google' } } },
      ] };
      if (command === 'google_import_transfer') { if (readOnly) throw Error('read-only'); expect(args).toEqual({ accountId: 'contacts', bookId: 'book', revision: 'revision-1', resources: ['people/new'] }); staged = true; return { transferred: 1, waiting: 0, review: 0, warning: null, readbackPending: false, contacts: [] }; }
      throw Error(`Unexpected command ${command}`);
    });
    await page.setContent('<div id="root"></div>');
    await page.addStyleTag({ content: ':root { --color-background: #fff; --color-foreground: #172033; --color-border: #cdd4de; } body { font-family: system-ui; }' + await readFile('src/desktop/desktop.css', 'utf8') });
    await page.addScriptTag({ content: bundle });
    await page.getByRole('button', { name: 'Google-Kontakte übernehmen', exact: true }).click();
    await page.getByRole('button', { name: 'Kontaktliste aktualisieren', exact: true }).click();
    await page.getByText('Kontakte prüfen und Auswahl ändern', { exact: true }).click();
    await expect(page.getByLabel('Bob Example übernehmen')).toBeDisabled();
    await expect(page.getByLabel('Charlie Google übernehmen')).toBeDisabled();
    await page.getByRole('row').filter({ hasText: 'Charlie Google' }).getByRole('button', { name: 'Vergleichen' }).click();
    await expect(page.getByRole('cell', { name: 'Charlie Lokal', exact: true })).toBeVisible();
    if (readOnly) {
      await expect(page.getByLabel('Alice Example übernehmen')).toBeDisabled();
      await expect(page.getByText('Nur-Lese-Modus: Vorschau ist möglich, Übernahme nach Stalwart ist gesperrt.')).toBeVisible();
    } else {
      await page.getByLabel('Alice Example übernehmen').check();
      await page.screenshot({ path: '.cache/google-import-preview.png' });
      await page.getByRole('button', { name: 'Kontakte übertragen (1)', exact: true }).click();
      await expect(page.getByRole('status')).toContainText('1 Kontakte übertragen.');
      await page.getByRole('button', { name: 'Kontaktliste aktualisieren', exact: true }).click();
      await expect(page.getByLabel('Alice Example übernehmen')).toBeDisabled();
    }
    expect(calls.some((c) => ['sync_pending_changes', 'approve_pending_changes', 'google_import_apply', 'google_connect', 'google_fetch_contacts'].includes(c.command))).toBe(false);
  });
}
for (const configured of [true, false]) {
  test(`Google connection uses one native action without client fields (${configured ? 'configured app' : 'unconfigured app'})`, async ({ page }) => {
    const calls: { command: string; args: unknown }[] = [];
    let connected = false;
    await page.exposeFunction('nativeInvoke', async (command: string, args: unknown) => {
      calls.push({ command, args });
      if (command === 'google_import_status') return { connected, oauthConfigured: configured, email: connected ? 'reader@example.test' : '', fetchedAt: 0, count: 0, books: [], readOnly: true };
      if (command === 'google_connect') { expect(args).toBeUndefined(); connected = true; return; }
      throw Error(`Unexpected command ${command}`);
    });
    await page.setContent('<div id="root"></div>');
    await page.addScriptTag({ content: bundle });
    await page.getByRole('button', { name: 'Google-Kontakte übernehmen', exact: true }).click();
    await expect(page.getByLabel('Google OAuth-Client-ID', { exact: true })).toHaveCount(0);
    await expect(page.locator('input[type=password]')).toHaveCount(0);
    const connect = page.getByRole('button', { name: 'Mit Google verbinden', exact: true });
    if (configured) {
      await expect(connect).toBeEnabled();
      await connect.click();
      await expect(page.getByText('Google verbunden: reader@example.test')).toBeVisible();
      expect(calls.filter((c) => c.command === 'google_connect')).toHaveLength(1);
    } else {
      await expect(connect).toBeDisabled();
      await expect(page.getByText('Die Google-Anmeldung ist in dieser App-Version noch nicht eingerichtet.')).toBeVisible();
      expect(calls.some((c) => c.command === 'google_connect')).toBe(false);
    }
  });
}


import { test, expect } from '@playwright/test';

test('empty-profile setup can be dismissed and reopened without persisting credentials', async ({ page }) => {
  const calls: string[] = [];
  await page.exposeFunction('nativeFixture', async (command: string) => {
    calls.push(command);
    if (command === 'profile_inventory') return { activeProfileId: 'empty', profiles: [
      { id: 'empty', name: 'Neues Profil', accounts: [], activeAccountId: null },
      { id: 'existing', name: 'Vorhanden', accounts: [{ id: 'login', label: 'Vorhandenes Konto', username: 'reader@example.test' }], activeAccountId: 'login' },
    ] };
    if (command === 'has_account_configuration' || command === 'push_status') return false;
    if (command === 'desktop_kv_keys') return [];
    if (command === 'desktop_kv_put' || command === 'desktop_kv_delete') return null;
    if (command === 'desktop_kv_get' || command === 'log_frontend_event') return null;
    throw Error(`Unexpected command: ${command}`);
  });
  await page.addInitScript(() => {
    (window as any).isTauri = true;
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    (window as any).__TAURI_INTERNALS__ = {
      transformCallback: () => 1, unregisterCallback: () => {},
      invoke: (command: string, args: unknown) => command.startsWith('plugin:event|') ? Promise.resolve(1) : (window as any).nativeFixture(command, args),
      convertFileSrc: () => `${window.location.origin}/plugin-sandbox/index.html`,
    };
  });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Konto hinzufügen', exact: true })).toBeVisible();
  await page.getByLabel('Benutzername', { exact: true }).fill('unsaved@example.test');
  await page.getByLabel('Passwort', { exact: true }).fill('synthetic-unsaved');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('heading', { name: 'Kein Konto verbunden', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Profil „Vorhanden“ öffnen', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Konto hinzufügen', exact: true }).click();
  await expect(page.getByLabel('Benutzername', { exact: true })).toHaveValue('');
  await expect(page.getByLabel('Passwort', { exact: true })).toHaveValue('');
  await page.getByRole('button', { name: 'Kontoeinrichtung schließen', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Kein Konto verbunden', exact: true })).toBeVisible();
  expect(calls.some((command) => ['setup_account', 'add_profile_account', 'create_profile'].includes(command))).toBe(false);
});

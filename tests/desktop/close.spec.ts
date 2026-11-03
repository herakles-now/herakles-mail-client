import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { flushBeforeClose } from '../../src/desktop/close';

test('Tauri close handler can perform the SDK automatic destroy', async () => {
  const capability = JSON.parse(await readFile('src-tauri/capabilities/default.json', 'utf8'));
  expect(capability.permissions).toContain('core:window:allow-destroy');
  let saved = false;
  let failed = false;
  await flushBeforeClose(async () => { saved = true; }, () => { failed = true; });
  expect(saved).toBe(true);
  expect(failed).toBe(false);
});

test('failed or stalled storage never prevents closing indefinitely', async () => {
  let failures = 0;
  await flushBeforeClose(async () => { throw new Error('disk full'); }, () => { failures++; });
  await flushBeforeClose(() => new Promise<void>(() => {}), () => { failures++; }, 5);
  expect(failures).toBe(2);
});

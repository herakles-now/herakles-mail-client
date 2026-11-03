import { invoke, isTauri } from '@tauri-apps/api/core';
let writes: Promise<unknown> = Promise.resolve();
const failed = new Map<string, string | null>();
const write = (key: string, value: string | null) => value === null
  ? invoke('desktop_kv_delete', { key: `ui/${key}` }) : invoke('desktop_kv_put', { key: `ui/${key}`, value });
export async function installDesktopStorage() {
  if (!isTauri()) return;
  const keys = await invoke<string[]>('desktop_kv_keys', { prefix: 'ui/' });
  const values = await Promise.all(keys.map(async (key) => [key.slice(3), await invoke<string | null>('desktop_kv_get', { key })] as const));
  const cache = new Map(values.filter((entry): entry is readonly [string, string] => entry[1] !== null));
  const persist = (key: string, value: string | null) => {
    writes = writes.then(async () => {
      try { await write(key, value); failed.delete(key); }
      catch { failed.set(key, value); window.dispatchEvent(new Event('desktop-storage-error')); }
    });
  };
  const storage: Storage = {
    get length() { return cache.size; }, key: (index) => [...cache.keys()][index] ?? null,
    getItem: (key) => cache.get(key) ?? null,
    setItem(key, value) { cache.set(String(key), String(value)); persist(String(key), String(value)); },
    removeItem(key) { cache.delete(key); persist(key, null); },
    clear() { for (const key of [...cache.keys()]) this.removeItem(key); },
  };
  Object.defineProperty(window, 'localStorage', { value: storage, configurable: true });
}
export async function flushDesktopStorage() {
  await writes;
  for (const [key, value] of failed) { await write(key, value); failed.delete(key); }
}

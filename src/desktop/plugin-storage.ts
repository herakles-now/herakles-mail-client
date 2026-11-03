import { invoke } from '@tauri-apps/api/core';

const key = (kind: string, id: string) => `plugins/${kind}/${id}`;
const put = (kind: string, id: string, value: string) => invoke<void>('desktop_kv_put', { key: key(kind, id), value });
const get = (kind: string, id: string) => invoke<string | null>('desktop_kv_get', { key: key(kind, id) });
const remove = (kind: string, id: string) => invoke<void>('desktop_kv_delete', { key: key(kind, id) });

export const pluginStorage = {
  saveCode: (id: string, value: string) => put('code', id, value),
  getCode: (id: string) => get('code', id),
  deleteCode: (id: string) => remove('code', id),
  codeIds: async () => (await invoke<string[]>('desktop_kv_keys', { prefix: 'plugins/code/' })).map((id) => id.slice('plugins/code/'.length)),
  saveThemeCSS: (id: string, value: string) => put('theme', id, value),
  getThemeCSS: (id: string) => get('theme', id),
  deleteThemeCSS: (id: string) => remove('theme', id),
  saveThemeSkin: (id: string, value: string) => put('skin', id, value),
  getThemeSkin: (id: string) => get('skin', id),
  deleteThemeSkin: (id: string) => remove('skin', id),
  savePreview: (id: string, value: string) => put('preview', id, value),
  getPreview: (id: string) => get('preview', id),
  deletePreview: (id: string) => remove('preview', id),
};
export const fileStorage = {
  async saveFile(id: string, file: File) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    let encoded = '';
    for (let i = 0; i < bytes.length; i += 8192) encoded += String.fromCharCode(...bytes.subarray(i, i + 8192));
    await put('files', id, JSON.stringify({ name: file.name, type: file.type, lastModified: file.lastModified, bytes: btoa(encoded) }));
  },
  async getFile(id: string) {
    const saved = await get('files', id);
    if (!saved) return null;
    const row = JSON.parse(saved);
    return new File([Uint8Array.from(atob(row.bytes), (c) => c.charCodeAt(0))], row.name, { type: row.type, lastModified: row.lastModified });
  },
  deleteFile: (id: string) => remove('files', id),
  async clearFiles() {
    const keys = await invoke<string[]>('desktop_kv_keys', { prefix: 'plugins/files/' });
    for (const key of keys) await invoke('desktop_kv_delete', { key });
  },
};

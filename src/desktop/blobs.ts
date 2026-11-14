import { invoke } from '@tauri-apps/api/core';

interface CachedBlob { base64: string; mediaType: string; }
const downloads = new Map<string, Promise<void>>();

export async function readDesktopBlob(accountId: string, blobId: string): Promise<Blob> {
  let cached = await invoke<CachedBlob | null>('local_blob', { accountId, blobId });
  if (!cached) {
    const key = JSON.stringify([accountId, blobId]);
    let pending = downloads.get(key);
    if (!pending) {
      pending = invoke<void>('sync_blob', { accountId, blobId }).finally(() => downloads.delete(key));
      downloads.set(key, pending);
    }
    try { await pending; }
    catch (reason) { throw new Error(typeof reason === 'string' ? reason : 'Anhang konnte nicht heruntergeladen werden.'); }
    cached = await invoke<CachedBlob | null>('local_blob', { accountId, blobId });
  }
  if (!cached) throw new Error('Dieser Anhang ist noch nicht offline gespeichert.');
  const binary = atob(cached.base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return new Blob([bytes], { type: cached.mediaType });
}


export async function stageDesktopUpload(file: File, accountId: string) {
  if (file.size > 25 * 1024 * 1024) throw new Error('Datei überschreitet das Uploadlimit von 25 MiB.');
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 32768) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 32768));
  }
  try {
    return await invoke<{ blobId: string; size: number; type: string }>('stage_upload', {
      accountId, base64: btoa(binary), mediaType: file.type || 'application/octet-stream',
    });
  } catch (reason) {
    throw new Error(typeof reason === 'string' ? reason : 'Datei konnte nicht lokal gespeichert werden.');
  }
}

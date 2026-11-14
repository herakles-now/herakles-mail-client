import { invoke } from '@tauri-apps/api/core';
import type { OfflineMethodCall, OfflineResponse } from '@/lib/jmap/offline-hooks';

export async function answerFiles(calls: OfflineMethodCall[]): Promise<OfflineResponse> {
  const methodResponses: OfflineResponse['methodResponses'] = [];
  for (const [method, originalArgs, id] of calls) {
    const args = originalArgs as Record<string, any>;
    try {
      if (method === 'FileNode/set') {
        const result = await invoke<Record<string, any>>('local_object_set', { method, args });
        methodResponses.push([method, result, id]);
        window.dispatchEvent(new Event('desktop-write-queued'));
        continue;
      }
      const snapshot = await invoke<{ list: Record<string, any>[]; state: string | null; localRevision?: number }>('local_files', { accountId: args.accountId });
      if (!snapshot.state) throw new Error('serverUnavailable');
      const state = `local-files:${snapshot.state}:${snapshot.localRevision ?? 0}`;
      if (method === 'FileNode/get') {
        const list = snapshot.list.filter((file) => args.ids == null || args.ids.includes(file.id));
        methodResponses.push([method, { accountId: args.accountId, state, list, notFound: (args.ids || []).filter((id: string) => !list.some((file) => file.id === id)) }, id]);
      } else if (method === 'FileNode/query') {
        const filter = args.filter || {};
        if (Object.keys(filter).some((key) => !['parentId', 'name', 'text'].includes(key))) throw new Error('unsupportedFilter');
        const list = snapshot.list.filter((file) => (!Object.hasOwn(filter, 'parentId') || file.parentId === filter.parentId)
          && (!filter.name || file.name === filter.name) && (!filter.text || String(file.name).toLocaleLowerCase().includes(String(filter.text).toLocaleLowerCase())));
        const position = Math.max(0, Number(args.position) || 0);
        methodResponses.push([method, { accountId: args.accountId, queryState: state, position, total: list.length, canCalculateChanges: false, ids: list.slice(position, position + (args.limit ?? 1000)).map((file) => file.id) }, id]);
      } else throw new Error('unknownMethod');
    } catch (error) {
      const type = typeof error === 'string' ? error : error instanceof Error ? error.message : 'serverUnavailable';
      methodResponses.push(['error', { type: ['forbidden', 'notFound', 'invalidPatch', 'unsupportedFilter', 'unknownMethod'].includes(type) ? type : 'serverUnavailable' }, id]);
    }
  }
  return { methodResponses };
}

import { contactHooks } from '@/lib/plugin-hooks';
import { contactToView } from '@/lib/plugin-projection';
import type { ContactCard } from '@/lib/jmap/types';

export async function allowGoogleContact(row: { status: string; targetId: string | null; uid: string; incoming: Record<string, unknown>; patch: Record<string, unknown> }, bookId: string): Promise<boolean> {
  if (row.status === 'new') {
    return contactHooks.onBeforeContactCreate.intercept(contactToView({ ...row.incoming, id: '', uid: row.uid, addressBookIds: { [bookId]: true } } as unknown as ContactCard));
  }
  return contactHooks.onBeforeContactUpdate.intercept({ contactId: row.targetId, patch: row.patch });
}
export async function notifyGoogleContacts(contacts: ContactCard[], createdUids: Set<string>): Promise<void> {
  for (const contact of contacts) {
    const hook = createdUids.has(contact.uid ?? '') ? contactHooks.onAfterContactCreate : contactHooks.onAfterContactUpdate;
    if (hook.size) void hook.emit(contactToView(contact));
  }
}

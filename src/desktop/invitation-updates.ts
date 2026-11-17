export type InvitationRecord = Record<string, any>;
export type InvitationUpdatePlan = { kind: 'missing' | 'current' | 'outdated' | 'conflict' | 'unsupported' | 'untrusted' } | { kind: 'update'; id: string; patch: InvitationRecord };
const address = (value: unknown) => typeof value === 'string' ? value.trim().replace(/^mailto:/i, '').toLowerCase() : '';
const version = (event: InvitationRecord) => Number.isSafeInteger(event.sequence ?? 0) && (event.sequence ?? 0) >= 0 ? event.sequence ?? 0 : null;
const details = ['title', 'description', 'descriptionContentType', 'start', 'duration', 'timeZone', 'showWithoutTime', 'locations', 'virtualLocations', 'status'] as const;

const canonical = (value: any): any => {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
  return value;
};
function detail(event: InvitationRecord, key: typeof details[number]): any {
  const defaults: InvitationRecord = { showWithoutTime: false, status: 'confirmed', title: '', description: '', descriptionContentType: 'text/plain', locations: {}, virtualLocations: {} };
  return canonical(event[key] ?? defaults[key] ?? null);
}

// Deliberately limited to a single meeting. Series and participant changes use
// Bulwark/Stalwart scheduling, never a guessed replacement of a whole series.
export function planInvitationUpdate(incoming: InvitationRecord, stored: InvitationRecord[], trusted: boolean): InvitationUpdatePlan {
  const matching = stored.filter((event) => event.uid && event.uid === incoming.uid);
  if (!matching.length) return { kind: 'missing' };
  if (matching.length !== 1) return { kind: 'conflict' };
  const current = matching[0];
  const nextVersion = version(incoming), oldVersion = version(current);
  if (nextVersion === null || oldVersion === null) return { kind: 'conflict' };
  if (nextVersion < oldVersion) return { kind: 'outdated' };
  const different = details.some((key) => JSON.stringify(detail(incoming, key)) !== JSON.stringify(detail(current, key)));
  if (nextVersion === oldVersion) return { kind: different ? 'conflict' : 'current' };
  if (!trusted || !address(incoming.organizerCalendarAddress) || address(incoming.organizerCalendarAddress) !== address(current.organizerCalendarAddress)) return { kind: 'untrusted' };
  if ([incoming, current].some((event) => event.recurrenceId || event.recurrenceRule || event.recurrenceRules?.length || Object.keys(event.recurrenceOverrides ?? {}).length)) return { kind: 'unsupported' };
  if (typeof incoming.start !== 'string' || typeof incoming.duration !== 'string' || !current.id) return { kind: 'unsupported' };
  const patch: InvitationRecord = { sequence: nextVersion };
  for (const key of details) patch[key] = incoming[key] ?? null;
  // The user's alerts, calendar links and participation answer remain intact.
  return { kind: 'update', id: current.id, patch };
}

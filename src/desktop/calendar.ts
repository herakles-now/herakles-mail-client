import { invoke } from '@tauri-apps/api/core';
import { normalizeCalendarEventLike } from '@/lib/calendar-event-normalization';
import { expandRecurringEvents } from '@/lib/recurrence-expansion';
import { getEventStartDate, getEventEndDate } from '@/lib/calendar-utils';
import { parseLocalDateTime, fromWallClock, toDisplayDate } from '@/lib/timezone';
import type { CalendarEvent } from '@/lib/jmap/types';
import type { OfflineMethodCall, OfflineResponse } from '@/lib/jmap/offline-hooks';
import { format } from 'date-fns';

interface Snapshot { accessDenied?: boolean; calendars: Record<string, any>[]; events: Record<string, Record<string, any>>; identities: Record<string, any>[]; eventState: string | null; calendarState: string | null; }

function rangeBound(value: unknown, zone: string): Date | null {
  if (typeof value !== 'string') return null;
  const wall = parseLocalDateTime(value);
  const instant = wall ? fromWallClock(wall, zone) : new Date(value);
  if (!Number.isFinite(instant.getTime())) throw new Error('invalidArguments');
  return toDisplayDate(instant);
}

export function calendarMatches(raw: Record<string, any>, filter: Record<string, any> | null, zone: string): boolean {
  if (!filter) return true;
  if (filter.operator) {
    if (!Array.isArray(filter.conditions)) throw new Error('invalidArguments');
    const results = filter.conditions.map((condition: Record<string, any>) => calendarMatches(raw, condition, zone));
    switch (filter.operator) { case 'AND': return results.every(Boolean); case 'OR': return results.some(Boolean); case 'NOT': return !results.some(Boolean); default: throw new Error('unsupportedFilter'); }
  }
  for (const key of Object.keys(filter)) {
    if (!['inCalendar','inCalendars','after','before','text','uid','title'].includes(key)) throw new Error('unsupportedFilter');
  }
  if (filter.inCalendar && !raw.calendarIds?.[filter.inCalendar]) return false;
  if (filter.inCalendars && !filter.inCalendars.some((id: string) => raw.calendarIds?.[id])) return false;
  if (filter.uid && raw.uid !== filter.uid) return false;
  if (filter.title && !String(raw.title || '').toLocaleLowerCase().includes(String(filter.title).toLocaleLowerCase())) return false;
  if (filter.text && ![raw.title, raw.description].some((value) => String(value || '').toLocaleLowerCase().includes(String(filter.text).toLocaleLowerCase()))) return false;
  const after = rangeBound(filter.after, zone);
  const before = rangeBound(filter.before, zone);
  if (!after && !before) return true;
  const event = normalizeCalendarEventLike(raw as CalendarEvent);
  if (event.recurrenceRules?.length) {
    // Keep the master for Bulwark's original client-side recurrence expansion.
    if (!after || !before) return true;
    return expandRecurringEvents([event], format(after, "yyyy-MM-dd'T'HH:mm:ss"), format(before, "yyyy-MM-dd'T'HH:mm:ss")).some((occurrence) =>
      getEventStartDate(occurrence) < before && getEventEndDate(occurrence) >= after);
  }
  return (!before || getEventStartDate(event) < before) && (!after || getEventEndDate(event) > after || getEventStartDate(event).getTime() === after.getTime());
}

export async function answerCalendar(methodCalls: OfflineMethodCall[], defaultAccount: string, readOnly: boolean): Promise<OfflineResponse> {
  const snapshots = new Map<string, Snapshot>();
  const responses: OfflineResponse['methodResponses'] = [];
  const prior = new Map<string, Record<string, any>>();
  for (const [method, originalArgs, id] of methodCalls) {
    const args = { ...originalArgs } as Record<string, any>;
    try {
      for (const [key, value] of Object.entries(args)) if (key.startsWith('#')) {
        const reference = value as { resultOf: string; name: string; path: string };
        const earlier = prior.get(`${reference.resultOf}:${reference.name}`);
        if (!earlier || reference.path !== '/ids') throw new Error('invalidResultReference');
        args[key.slice(1)] = earlier.ids; delete args[key];
      }
      if (method.endsWith('/set') || method.endsWith('/parse')) throw new Error('forbidden');
      const account = String(args.accountId || defaultAccount);
      if (!snapshots.has(account)) snapshots.set(account, await invoke<Snapshot>('local_calendar_snapshot', { accountId: account }));
      const snapshot = snapshots.get(account)!;
      if (snapshot.accessDenied) throw new Error('accountNotSupportedByMethod');
      if (!snapshot.calendarState) throw new Error('serverUnavailable');
      const state = `local-calendar:${snapshot.eventState || 'empty'}:${snapshot.calendarState}`;
      let result: Record<string, any>;
      if (method === 'CalendarEvent/query') {
        if (!snapshot.eventState) throw new Error('serverUnavailable');
        let events = Object.values(snapshot.events).filter((event) => calendarMatches(event, args.filter ?? null, args.timeZone || 'UTC'));
        for (const sort of args.sort || []) if (!['start','uid','title','created','updated'].includes(sort.property)) throw new Error('unsupportedSort');
        events.sort((left, right) => {
          for (const sort of args.sort || [{ property: 'start', isAscending: true }]) {
            const comparison = String(left[sort.property] || '').localeCompare(String(right[sort.property] || ''));
            if (comparison) return sort.isAscending === false ? -comparison : comparison;
          }
          return String(left.id).localeCompare(String(right.id));
        });
        const position = Math.max(0, Number(args.position) || 0);
        result = { accountId: account, queryState: state, canCalculateChanges: false, position, total: events.length,
          ids: events.slice(position, position + (args.limit ?? 1000)).map((event) => event.id) };
      } else if (['Calendar/get','CalendarEvent/get','ParticipantIdentity/get'].includes(method)) {
        let objects = method === 'Calendar/get' ? snapshot.calendars : method === 'CalendarEvent/get' ? Object.values(snapshot.events) : snapshot.identities;
        if (method === 'Calendar/get' && readOnly) objects = objects.map((calendar) => ({ ...calendar,
          myRights: Object.fromEntries(Object.entries(calendar.myRights || {}).map(([key, value]) => [key, key.startsWith('mayRead') ? value : false])) }));
        const selected = args.ids == null ? objects : objects.filter((object) => args.ids.includes(object.id));
        const notFound = args.ids == null ? [] : args.ids.filter((wanted: string) => !selected.some((object) => object.id === wanted));
        const list = selected.map((object) => args.properties ? Object.fromEntries(['id', ...args.properties].filter((key) => key in object).map((key) => [key, object[key]])) : object);
        result = { accountId: account, state, list, notFound };
      } else { throw new Error(method.endsWith('/changes') ? 'cannotCalculateChanges' : 'unknownMethod'); }
      prior.set(`${id}:${method}`, result); responses.push([method, result, id]);
    } catch (error) {
      const message = error instanceof Error ? error.message : typeof error === 'string' ? error : '';
      const type = ['invalidArguments','unsupportedFilter','unsupportedSort','invalidResultReference','serverUnavailable','accountNotFound','accountNotSupportedByMethod','forbidden','cannotCalculateChanges','unknownMethod'].includes(message) ? message : 'serverUnavailable';
      responses.push(['error', { type }, id]);
    }
  }
  return { methodResponses: responses };
}

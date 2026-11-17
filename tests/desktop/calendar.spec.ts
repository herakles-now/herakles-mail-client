import { test, expect } from '@playwright/test';
import { build } from 'esbuild';
import path from 'node:path';
import { planInvitationUpdate } from '../../src/desktop/invitation-updates';

test('original Bulwark cancellation handles sequence and occurrence identity without dropping other overrides', async ({ page }) => {
  const bundle = await build({
    entryPoints: ['upstream/bulwark/lib/calendar-cancellation.ts'], bundle: true,
    write: false, format: 'iife', globalName: 'Cancellation', platform: 'browser',
    alias: { '@': path.resolve('upstream/bulwark') },
    define: { 'process.env.NODE_ENV': '"production"', 'process.env': '{}' },
  });
  await page.goto('/');
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  const result = await page.evaluate(() => {
    const { planCancellationRemoval: plan } = (window as any).Cancellation;
    const event = { id: 'series', uid: 'meeting', sequence: 4, timeZone: 'Europe/Berlin',
      recurrenceRules: [{ frequency: 'weekly' }], recurrenceOverrides: {
        '2026-11-25T10:00:00': { title: 'Previously moved occurrence', start: '2026-11-25T11:00:00' },
      } };
    return {
      stale: plan([event], { uid: 'meeting', sequence: 3 }),
      series: plan([event], { uid: 'meeting', sequence: 4 }),
      unrelated: plan([event], { uid: 'different', sequence: 5 }),
      occurrence: plan([event], { uid: 'meeting', sequence: 4,
        recurrenceId: '2026-11-18T09:00:00', recurrenceIdTimeZone: 'Etc/UTC' }),
      unread: plan([{ ...event, recurrenceOverrides: undefined }], {
        uid: 'meeting', sequence: 4, recurrenceId: '2026-11-18T10:00:00' }),
      newerOccurrence: plan([{ ...event, recurrenceOverrides: {
        '2026-11-18T10:00:00': { sequence: 6 },
      } }], { uid: 'meeting', sequence: 5, recurrenceId: '2026-11-18T10:00:00' }),
    };
  });
  expect(result.stale).toEqual({ kind: 'outdated' });
  expect(result.series).toEqual({ kind: 'delete', ids: ['series'] });
  expect(result.unrelated).toEqual({ kind: 'none' });
  expect(result.unread).toEqual({ kind: 'none' });
  expect(result.newerOccurrence).toEqual({ kind: 'outdated' });
  expect(result.occurrence).toEqual({ kind: 'exclude', id: 'series', patch: {
    'recurrenceOverrides/2026-11-18T10:00:00': { excluded: true },
  } });
});


test('invitation reconciliation refuses ambiguous versions, organizer changes and series replacements', () => {
  const stored = { id: 'e', uid: 'uid', sequence: 2, organizerCalendarAddress: 'mailto:organizer@example.test', start: '2026-11-18T10:00:00', duration: 'PT1H', title: 'Meeting', participants: { me: { participationStatus: 'accepted' } }, alerts: { a: {} } };
  const incoming = { ...stored, sequence: 3, start: '2026-11-18T11:00:00' };
  const plan = planInvitationUpdate(incoming, [stored], true);
  expect(plan.kind).toBe('update');
  if (plan.kind !== 'update') throw new Error('Missing update plan');
  expect(plan.patch.start).toBe(incoming.start);
  expect(plan.patch.sequence).toBe(3);
  for (const key of ['id', 'uid', 'participants', 'alerts', 'calendarIds', 'organizerCalendarAddress', 'recurrenceOverrides']) expect(plan.patch).not.toHaveProperty(key);
  expect(planInvitationUpdate(stored, [stored], true).kind).toBe('current');
  expect(planInvitationUpdate(stored, [{ ...stored, showWithoutTime: false, status: 'confirmed', locations: {}, descriptionContentType: 'text/plain' }], true).kind).toBe('current');
  expect(planInvitationUpdate({ ...incoming, sequence: 1 }, [stored], true).kind).toBe('outdated');
  expect(planInvitationUpdate({ ...incoming, sequence: 2 }, [stored], true).kind).toBe('conflict');
  expect(planInvitationUpdate(incoming, [stored, stored], true).kind).toBe('conflict');
  expect(planInvitationUpdate(incoming, [], true).kind).toBe('missing');
  expect(planInvitationUpdate(incoming, [stored], false).kind).toBe('untrusted');
  expect(planInvitationUpdate({ ...incoming, organizerCalendarAddress: 'mailto:other@example.test' }, [stored], true).kind).toBe('untrusted');
  expect(planInvitationUpdate({ ...incoming, recurrenceId: incoming.start }, [stored], true).kind).toBe('unsupported');
  expect(planInvitationUpdate({ ...incoming, recurrenceRule: { frequency: 'weekly' } }, [stored], true).kind).toBe('unsupported');
  expect(planInvitationUpdate({ ...incoming, sequence: -1 }, [stored], true).kind).toBe('conflict');
});

test('original invitation replies use the existing copy and server scheduling once', async ({ page }) => {
  const bundle = await build({
    stdin: { contents: "export { answerInvitation } from './upstream/bulwark/lib/calendar-rsvp'; export { getInvitationTrustAssessment } from './upstream/bulwark/lib/calendar-invitation';", resolveDir: process.cwd(), loader: 'ts' },
    bundle: true, write: false, format: 'iife', globalName: 'InvitationReply', platform: 'browser',
    alias: { '@': path.resolve('upstream/bulwark') },
    define: { 'process.env.NODE_ENV': '"production"', 'process.env': '{}' },
  });
  await page.goto('/');
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  const result = await page.evaluate(async () => {
    const { answerInvitation, getInvitationTrustAssessment: trust } = (window as any).InvitationReply;
    const existing = { id: 'stored-copy', uid: 'meeting', organizerCalendarAddress: 'mailto:organizer@example.test',
      participants: { me: { calendarAddress: 'mailto:reader@example.test', roles: { attendee: true }, participationStatus: 'needs-action' } } };
    const parsed = { ...existing, id: null };
    const calls: any[] = [];
    const store = { rsvpEvent: async (...args: any[]) => { calls.push(['rsvp', ...args.slice(1)]); return true; },
      importEvents: async () => { throw new Error('Must not duplicate an existing invitation'); },
      updateEvent: async (...args: any[]) => { calls.push(['repair', ...args.slice(1)]); return true; }, events: () => [existing] };
    const request = { client: {}, store, parsedEvent: parsed, existingEvent: existing,
      myParticipant: { id: 'me', participant: existing.participants.me }, actingEmails: ['reader@example.test'],
      calendarId: 'calendar', sharedMailAccount: null, resolveExisting: async () => existing };
    const outcomes = [];
    for (const status of ['accepted', 'tentative', 'declined']) outcomes.push(await answerInvitation(request, status));
    const repaired = await answerInvitation({ ...request, existingEvent: { ...existing, participants: null },
      resolveExisting: async () => ({ ...existing, participants: null }) }, 'accepted');
    const from = [{ email: 'sender@example.test' }];
    return { outcomes, repaired, calls,
      verifiedDifferentSender: trust(parsed, { from, authenticationResults: { dmarc: { result: 'pass', domain: 'example.test' } } }, 'request'),
      unverifiedDifferentSender: trust(parsed, { from }, 'request') };
  });
  expect(result.outcomes).toEqual(['answered', 'answered', 'answered']);
  expect(result.calls.slice(0, 3).map((row: any[]) => row.slice(0, 4))).toEqual([
    ['rsvp', 'stored-copy', 'me', 'accepted'], ['rsvp', 'stored-copy', 'me', 'tentative'], ['rsvp', 'stored-copy', 'me', 'declined'],
  ]);
  expect(result.repaired).toBe('answered');
  expect(result.calls[3][0]).toBe('repair');
  expect(result.calls[3][2].participants.me.participationStatus).toBe('accepted');
  expect(result.calls[3][3]).toBe(true); // Let Stalwart send the REPLY; no separate EmailSubmission.
  expect(result.verifiedDifferentSender.reason).toBe('sender_mismatch');
  expect(result.unverifiedDifferentSender.level).toBe('warning');
});

import { test, expect } from '@playwright/test';
import { accountsFor, accountSupports, MAIL_CAPABILITY, CALENDAR_CAPABILITY, sessionFingerprint } from '../../src/desktop/accounts';

test('sync plans include shared accounts, respect explicit capability restrictions and revocation', () => {
  const session = {
    primaryAccounts: { [MAIL_CAPABILITY]: 'personal', [CALENDAR_CAPABILITY]: 'personal' },
    capabilities: { [MAIL_CAPABILITY]: {}, [CALENDAR_CAPABILITY]: {} },
    accounts: {
      personal: { isPersonal: true, accountCapabilities: { [MAIL_CAPABILITY]: {}, [CALENDAR_CAPABILITY]: {} } },
      shared: { isPersonal: false, accountCapabilities: {} },
      calendarOnly: { isPersonal: false, accountCapabilities: { [CALENDAR_CAPABILITY]: {} } },
      unsupportedPersonal: { isPersonal: true, accountCapabilities: {} },
    },
  };
  expect(accountsFor(session, MAIL_CAPABILITY)).toEqual(['personal', 'shared']);
  expect(accountsFor(session, CALENDAR_CAPABILITY)).toEqual(['personal', 'calendarOnly', 'shared']);
  expect(accountSupports(session, 'calendarOnly', MAIL_CAPABILITY)).toBe(false);
  const before = sessionFingerprint(session);
  const refreshed = { ...session, accounts: { personal: session.accounts.personal } };
  expect(sessionFingerprint(refreshed)).not.toEqual(before);
  expect(accountsFor(refreshed, MAIL_CAPABILITY)).toEqual(['personal']);
});

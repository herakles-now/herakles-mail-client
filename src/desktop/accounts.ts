export const MAIL_CAPABILITY = 'urn:ietf:params:jmap:mail';
export const CALENDAR_CAPABILITY = 'urn:ietf:params:jmap:calendars';

export interface DesktopAccount {
  name?: string;
  isPersonal?: boolean;
  isReadOnly?: boolean;
  accountCapabilities?: Record<string, unknown>;
}
export interface DesktopSession {
  state?: string;
  primaryAccounts: Record<string, string>;
  accounts: Record<string, DesktopAccount>;
  capabilities?: Record<string, unknown>;
}

export function accountSupports(session: DesktopSession, accountId: string, capability: string) {
  const account = session.accounts[accountId];
  if (!account) return false;
  const caps = account.accountCapabilities ?? {};
  if (Object.hasOwn(caps, capability)) return true;
  return account.isPersonal === false && Object.keys(caps).length === 0 && Object.hasOwn(session.capabilities ?? {}, capability);
}

export function accountsFor(session: DesktopSession, capability: string) {
  const primary = session.primaryAccounts[capability];
  return Object.keys(session.accounts).filter((id) => accountSupports(session, id, capability))
    .sort((left, right) => left === primary ? -1 : right === primary ? 1 : left.localeCompare(right));
}

export function sessionFingerprint(session: DesktopSession) {
  return JSON.stringify([session.state, session.accounts, session.primaryAccounts, session.capabilities]);
}

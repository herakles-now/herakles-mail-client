import { invoke } from '@tauri-apps/api/core';

export interface Email {
  id: string;
  subject?: string;
  preview?: string;
  receivedAt?: string;
  from?: { name?: string; email: string }[];
  keywords: Record<string, boolean>;
  textBody?: { partId: string }[];
  bodyValues?: Record<string, { value: string; isTruncated: boolean }>;
}

export interface LocalMail {
  initialized: boolean;
  emails: Email[];
  mailboxes: { id: string; name: string }[];
  pending: { sequence: number; emailId: string; keyword: string; enabled: boolean; blocked: string | null }[];
}

// Every read and mutation goes to redb. Only synchronize may contact Stalwart.
export const mailRepository = {
  isReadOnly: () => invoke<boolean>('read_only_mode'),
  read: (accountId: string) => invoke<LocalMail>('local_mail', { accountId }),
  setKeyword: (accountId: string, emailId: string, keyword: '$seen' | '$flagged', enabled: boolean) =>
    invoke<void>('queue_keyword', { accountId, emailId, keyword, enabled }),
  synchronize: (accountId: string) => invoke<void>('sync_mail', { accountId }),
};

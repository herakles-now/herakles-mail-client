import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

type AccountCache = { accountId: string; name: string; usedBytes: number; limitBytes: number; count: number };
const mib = (bytes: number) => (bytes / 1024 / 1024).toLocaleString('de-DE', { maximumFractionDigits: 1 });

function AccountLimit({ account, refresh }: { account: AccountCache; refresh: () => Promise<void> }) {
  const [limit, setLimit] = useState(String(account.limitBytes / 1024 / 1024));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const value = Number(limit);
  const valid = Number.isInteger(value) && value >= 25 && value <= 4096;
  const changed = value * 1024 * 1024 !== account.limitBytes;
  return <form className="desktop-cache-account" onSubmit={(event) => {
    event.preventDefault();
    if (!valid || busy || !changed) return;
    setBusy(true); setError(''); setSaved(false);
    void invoke('set_download_cache_limit', { accountId: account.accountId, limitMib: value })
      .then(refresh).then(() => setSaved(true)).catch((reason) => setError(String(reason))).finally(() => setBusy(false));
  }}>
    <strong>{account.name}</strong>
    <span>{mib(account.usedBytes)} MiB von {mib(account.limitBytes)} MiB · {account.count} Downloads</span>
    <progress aria-label={`Cachebelegung ${account.name}`} value={account.usedBytes} max={account.limitBytes} />
    <div className="desktop-cache-controls">
      <label>Limit in MiB <input type="number" min="25" max="4096" step="1" value={limit} disabled={busy} onChange={(event) => { setLimit(event.target.value); setSaved(false); }} /></label>
      <button type="submit" disabled={busy || !valid || !changed}>{busy ? 'Speichern …' : 'Limit speichern'}</button>
    </div>
    {valid && value * 1024 * 1024 < account.usedBytes && <p>Beim Speichern werden die ältesten Downloads sofort entfernt, bis das neue Limit eingehalten wird. Diese Anhänge sind anschließend erst nach erneutem Download offline verfügbar.</p>}
    {error && <p role="alert">{error}</p>}
    {saved && <p role="status">Limit gespeichert.</p>}
  </form>;
}
export function CacheSettings() {
  const [accounts, setAccounts] = useState<AccountCache[] | null>(null);
  const [error, setError] = useState('');
  const refresh = async () => { setAccounts(await invoke<AccountCache[]>('download_cache_usage')); };
  useEffect(() => {
    let active = true;
    void invoke<AccountCache[]>('download_cache_usage').then((rows) => { if (active) setAccounts(rows); }).catch((reason) => { if (active) setError(String(reason)); });
    return () => { active = false; };
  }, []);
  return <details className="desktop-cache-settings">
    <summary>Offline-Speicher · Downloadcache</summary>
    <div className="desktop-cache-body">
      <p>Das Limit gilt je Konto für heruntergeladene Anhänge und Dateien. Bei Platzmangel werden die ältesten Downloads entfernt. Zum erneuten Laden brauchst du eine Verbindung. Maildaten, Entwürfe und ausstehende Uploads bleiben erhalten.</p>
      <p>Die Anzeige zählt Downloadinhalte, nicht die gesamte Datenbankdatei. Freier Platz in der Datenbank wird wiederverwendet.</p>
      {error && <p role="alert">{error}</p>}
      {!accounts && !error && <p>Lade Speicherbelegung …</p>}
      {accounts?.length === 0 && <p>Keine lokalen Konten verfügbar.</p>}
      {accounts?.map((account) => <AccountLimit key={account.accountId} account={account} refresh={refresh} />)}
      <button type="button" onClick={() => { setError(''); void refresh().catch((reason) => setError(String(reason))); }}>Belegung aktualisieren</button>
    </div>
  </details>;
}

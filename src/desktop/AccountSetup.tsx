import { useEffect, useRef, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface SetupProps { onComplete: () => void; onCancel?: () => void; onBusyChange?: (busy: boolean) => void; profileId?: string; initialProfileName?: string; }
export function AccountSetup({ onComplete, onCancel, onBusyChange, profileId, initialProfileName }: SetupProps) {
  const [profileName, setProfileName] = useState(initialProfileName ?? 'Privat');
  const [serverUrl, setServerUrl] = useState('https://');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [readOnly, setReadOnly] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    onBusyChange?.(true);
    setError('');
    try {
      // The native command authenticates and validates JMAP before storing any credentials.
      if (profileId) await invoke('add_profile_account', { profileId, serverUrl, username, password, readOnly });
      else await invoke('setup_account', { profileName, serverUrl, username, password, readOnly });
      onComplete();
    } catch (reason) {
      setError(typeof reason === 'string' ? reason : 'Verbindung konnte nicht geprüft werden. Zugangsdaten wurden nicht gespeichert.');
    } finally {
      setBusy(false);
      onBusyChange?.(false);
    }
  };

  return <main className="desktop-account-setup" onKeyDown={(event) => {
    if (event.key === 'Escape' && onCancel) { event.preventDefault(); event.stopPropagation(); if (!busy) onCancel(); }
  }}>
    <form className="desktop-account-card" onSubmit={(event) => { void submit(event); }}>
      <div className="desktop-account-heading"><h1 id="account-setup-title">{profileId ? 'Konto hinzufügen' : 'Herakles Mail einrichten'}</h1>{onCancel && <button type="button" className="desktop-account-close" disabled={busy} onClick={onCancel} aria-label="Kontoeinrichtung schließen">×</button>}</div>
      <p>Gib die JMAP-Adresse und deine Zugangsdaten ein. Herakles prüft zuerst die Verbindung und speichert den Zugang erst danach sicher auf diesem Gerät.</p>
      {!profileId && <label>Profilname<input disabled={busy} autoFocus required maxLength={64} value={profileName} onChange={(event) => setProfileName(event.target.value)} /></label>}
      <label>JMAP-Sitzungsadresse
        <input disabled={busy} autoFocus={!!profileId} autoComplete="url" type="url" required value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://mail.example/.well-known/jmap" />
      </label>
      <label>Benutzername
        <input disabled={busy} autoComplete="username" required value={username} onChange={(event) => setUsername(event.target.value)} />
      </label>
      <label>Passwort
        <input disabled={busy} autoComplete="current-password" type="password" required value={password} onChange={(event) => setPassword(event.target.value)} />
      </label>
      <label className="desktop-account-readonly"><input disabled={busy} type="checkbox" checked={readOnly} onChange={(event) => setReadOnly(event.target.checked)} /> Konto zunächst nur lesend verbinden</label>
      {error && <p className="desktop-account-error" role="alert">{error}</p>}
      <div className="desktop-account-actions">{onCancel && <button type="button" className="desktop-account-cancel" disabled={busy} onClick={onCancel}>Abbrechen</button>}<button type="submit" disabled={busy}>{busy ? 'Verbindung wird geprüft …' : 'Verbindung prüfen und Konto speichern'}</button></div>
    </form>
  </main>;
}

export function AccountSetupDialog({ onCancel, ...props }: SetupProps & { onCancel: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => { dialog.current?.showModal(); }, []);
  return <dialog ref={dialog} className="desktop-account-dialog" aria-labelledby="account-setup-title" onCancel={(event) => { event.preventDefault(); if (!busy) onCancel(); }}><AccountSetup {...props} onCancel={onCancel} onBusyChange={setBusy} /></dialog>;
}

import { useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';

export function AccountSetup({ onComplete, profileId, initialProfileName }: { onComplete: () => void; profileId?: string; initialProfileName?: string }) {
  const [profileName, setProfileName] = useState(initialProfileName ?? 'Privat');
  const [serverUrl, setServerUrl] = useState('https://');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [readOnly, setReadOnly] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setBusy(true);
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
    }
  };

  return <main className="desktop-account-setup">
    <form className="desktop-account-card" onSubmit={(event) => { void submit(event); }}>
      <h1>Herakles Mail einrichten</h1>
      <p>Gib die JMAP-Adresse und deine Zugangsdaten ein. Herakles prüft zuerst die Verbindung und speichert den Zugang erst danach sicher auf diesem Gerät.</p>
      {!profileId && <label>Profilname<input required maxLength={64} value={profileName} onChange={(event) => setProfileName(event.target.value)} /></label>}
      <label>JMAP-Sitzungsadresse
        <input autoComplete="url" type="url" required value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://mail.example/.well-known/jmap" />
      </label>
      <label>Benutzername
        <input autoComplete="username" required value={username} onChange={(event) => setUsername(event.target.value)} />
      </label>
      <label>Passwort
        <input autoComplete="current-password" type="password" required value={password} onChange={(event) => setPassword(event.target.value)} />
      </label>
      <label className="desktop-account-readonly"><input type="checkbox" checked={readOnly} onChange={(event) => setReadOnly(event.target.checked)} /> Konto zunächst nur lesend verbinden</label>
      {error && <p className="desktop-account-error" role="alert">{error}</p>}
      <button type="submit" disabled={busy}>{busy ? 'Verbindung wird geprüft …' : 'Verbindung prüfen und Konto speichern'}</button>
    </form>
  </main>;
}

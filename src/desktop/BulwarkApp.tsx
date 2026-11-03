import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { NextIntlClientProvider } from 'next-intl';
import messages from '../../upstream/bulwark/locales/de/common.json';
import { MailApp } from '@/components/mail/mail-app';
import { SettingsApp } from '@/components/settings/settings-app';
import { CalendarApp } from '@/components/calendar/calendar-app';
import { ContactsApp } from '@/components/contacts/contacts-app';
import { FilesApp } from '@/components/files/files-app';
import { ThemeProvider } from '@/components/providers/theme-provider';
import { TourProvider } from '@/components/tour/tour-provider';
import { RateLimitToastProvider } from '@/components/providers/rate-limit-toast-provider';
import { PluginDialogHost } from '@/components/plugins/plugin-dialog-host';
import { PluginConsentDialog } from '@/components/plugins/plugin-consent-dialog';
import { PluginConnectDialog } from '@/components/plugins/plugin-connect-dialog';
import { PluginAppEvents } from '@/components/providers/plugin-app-events';
import { PluginPageNavigator } from '@/components/plugins/plugin-page-navigator';
import { ExternalRecipientConfirmHost } from '@/components/email/external-recipient-confirm';
import { usePathname } from './navigation';
import { bootstrapBulwark, synchronizeBulwark } from './bootstrap';
import { AccountSetup } from './AccountSetup';
import type { Bootstrap } from './bootstrap';
import '@/app/globals.css';
import './desktop.css';

function Routes() {
  const segments = usePathname().split('/').filter(Boolean);
  const localized = ['de', 'en'].includes(segments[0]);
  const surface = segments[localized ? 1 : 0];
  const rest = segments.slice(localized ? 2 : 1);
  switch (surface) {
    case 'settings': return <SettingsApp linkSegments={rest} />;
    case 'calendar': return <CalendarApp linkSegments={rest} />;
    case 'contacts': return <ContactsApp linkSegments={rest} />;
    case 'files': return <FilesApp linkSegments={rest} />;
    default: return <MailApp />;
  }
}
export default function BulwarkApp() {
  type Profile = { id: string; name: string; accounts: { id: string; label: string; username: string }[]; activeAccountId: string | null };
  const [info, setInfo] = useState<Bootstrap | null>(null);
  const [needsSetup, setNeedsSetup] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeProfileId, setActiveProfileId] = useState<string | null>(null);
  const [setupProfileId, setSetupProfileId] = useState<string | undefined>();
  const [showAccountSetup, setShowAccountSetup] = useState(false);
  const [newProfileName, setNewProfileName] = useState('');
  const [error, setError] = useState('');
  const [status, setStatus] = useState('Lokale Sitzung laden …');
  const [writes, setWrites] = useState({ pending: 0, blocked: 0, pendingDeletes: 0, blockedDeletes: 0 });
  const [offlineMode, setOfflineMode] = useState(false);
  const [connectivity, setConnectivity] = useState<'checking' | 'online' | 'offline'>('checking');
  const offlineModeRef = useRef(false);
  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let syncing = false;
    let visibilityCleanup = () => {};
    let stopPush = () => {};
    let requestSync = () => {};
    const updateWrites = () => { void invoke<typeof writes>('write_status').then((result) => { if (active) setWrites(result); }).catch(() => {}); };
    const queued = () => { updateWrites(); requestSync(); };
    const storageError = () => {
      void invoke('log_frontend_event', { level: 'error', area: 'storage', event: 'ui_storage_write_failed' }).catch(() => {});
      setStatus('Lokales Speichern fehlgeschlagen. Bitte Speicherplatz prüfen.');
    };
    const onOffline = () => {
      if (!offlineModeRef.current) {
        setConnectivity('offline');
        void invoke('set_network_enabled', { enabled: false }).catch(() => {});
      }
    };
    const onOnline = () => {
      if (!offlineModeRef.current) {
        setConnectivity('checking');
        void invoke('set_network_enabled', { enabled: true }).then(() => {
          window.dispatchEvent(new Event('desktop-sync-requested'));
        }).catch(() => {});
      }
    };
    const onSyncRequested = () => requestSync();
    window.addEventListener('desktop-storage-error', storageError);
    window.addEventListener('desktop-write-queued', queued);
    window.addEventListener('online', onOnline);
    window.addEventListener('offline', onOffline);
    window.addEventListener('desktop-sync-requested', onSyncRequested);
    void listen('jmap-push', () => requestSync()).then((unlisten) => {
      if (active) stopPush = unlisten;
      else unlisten();
    }).catch(() => {});
    const start = async () => {
      const registry = await invoke<{ profiles: Profile[]; activeProfileId: string | null }>('profile_inventory');
      setProfiles(registry.profiles);
      setActiveProfileId(registry.activeProfileId);
      const hasAccount = await invoke<boolean>('has_account_configuration');
      if (!hasAccount) {
        const active = registry.profiles.find((profile) => profile.id === registry.activeProfileId);
        setSetupProfileId(active?.id);
        setNeedsSetup(true);
        return null;
      }
      const preference = await invoke<string | null>('desktop_kv_get', { key: 'ui/network-mode' });
      const forcedOffline = preference === 'offline';
      offlineModeRef.current = forcedOffline;
      setOfflineMode(forcedOffline);
      const enabled = !forcedOffline;
      setConnectivity(enabled && navigator.onLine ? 'checking' : 'offline');
      await invoke('set_network_enabled', { enabled });
      return bootstrapBulwark();
    };
    start().then((result) => {
      if (!result) return;
      if (!active) return;
      setInfo(result);
      const sync = () => {
        if (offlineModeRef.current) {
          setConnectivity('offline');
          setStatus('Offline · lokale Daten verfügbar');
          void invoke('set_network_enabled', { enabled: false }).catch(() => {});
          return;
        }
        if (syncing) return;
        syncing = true;
        setStatus('Synchronisierung …');
        synchronizeBulwark().then(() => { if (active) { setConnectivity('online'); setStatus('Lokal verfügbar · Abgleich abgeschlossen'); } })
          .catch((reason) => {
            void invoke('log_frontend_event', { level: 'error', area: 'sync', event: 'ui_sync_failed' }).catch(() => {});
            if (active) { setConnectivity('offline'); setStatus(reason instanceof Error ? `Abgleich ausstehend · ${reason.message}` : 'Abgleich ausstehend · lokaler Datenbestand verfügbar'); }
          }).finally(() => { syncing = false; updateWrites(); });
      };
      requestSync = sync;
      const onVisibilityChange = () => { if (document.visibilityState === 'visible') sync(); };
      document.addEventListener('visibilitychange', onVisibilityChange);
      visibilityCleanup = () => document.removeEventListener('visibilitychange', onVisibilityChange);
      updateWrites(); sync(); timer = window.setInterval(sync, 15_000);
    }).catch((reason) => {
      void invoke('log_frontend_event', { level: 'error', area: 'ui', event: 'bootstrap_failed' }).catch(() => {});
      if (active) setError(typeof reason === 'string' ? reason : 'Desktop-Start fehlgeschlagen.');
    });
    return () => { active = false; window.clearInterval(timer); visibilityCleanup(); stopPush(); window.removeEventListener('desktop-storage-error', storageError); window.removeEventListener('desktop-write-queued', queued); window.removeEventListener('online', onOnline); window.removeEventListener('offline', onOffline); window.removeEventListener('desktop-sync-requested', onSyncRequested); };
  }, []);
  const toggleOfflineMode = async () => {
    if (!offlineMode && connectivity === 'offline') {
      await invoke('set_network_enabled', { enabled: true });
      setConnectivity('checking');
      window.dispatchEvent(new Event('desktop-sync-requested'));
      return;
    }
    const next = !offlineMode;
    offlineModeRef.current = next;
    setOfflineMode(next);
    await invoke('desktop_kv_put', { key: 'ui/network-mode', value: next ? 'offline' : 'auto' });
    const enabled = !next;
    setConnectivity(enabled && navigator.onLine ? 'checking' : 'offline');
    await invoke('set_network_enabled', { enabled });
    if (enabled) window.dispatchEvent(new Event('desktop-sync-requested'));
  };
  if (needsSetup) return <AccountSetup profileId={setupProfileId} onComplete={() => window.location.reload()} />;
  if (!info) return <div className="desktop-start"><h1>Herakles Mail</h1><p role="status">{error || status}</p></div>;
  return <NextIntlClientProvider locale="de" messages={messages} timeZone="Europe/Belgrade">
    <ThemeProvider><RateLimitToastProvider><TourProvider>
      <div className="desktop-status" role="status" title="Im Test-Schreibmodus sind Gelesen/Ungelesen, Markierungen und Entwurfsaktionen angebunden.">{info.readOnly ? 'Nur Lesen' : 'Test-Schreibmodus'} · {status}{writes.pending > 0 && ` · ${writes.pending} ausstehend`}{writes.blocked > 0 && ` · ${writes.blocked} abgelehnt`}{writes.pendingDeletes > 0 && ` · ${writes.pendingDeletes} Entwurfslöschung(en) ausstehend`}{writes.blockedDeletes > 0 && ` · ${writes.blockedDeletes} Entwurfslöschung(en) blockiert`}</div>
      <div className="desktop-profile-controls">
        <label>Profil <select value={activeProfileId ?? ''} onChange={(event) => { void invoke('switch_profile', { profileId: event.target.value }).then(() => window.location.reload()).catch((reason) => setError(String(reason))); }}>
          {profiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
        </select></label>
        <label>Konto <select value={profiles.find((profile) => profile.id === activeProfileId)?.activeAccountId ?? ''} onChange={(event) => { if (activeProfileId) void invoke('switch_profile_account', { profileId: activeProfileId, accountId: event.target.value }).then(() => window.location.reload()).catch((reason) => setError(String(reason))); }}>
          {(profiles.find((profile) => profile.id === activeProfileId)?.accounts ?? []).map((account) => <option key={account.id} value={account.id}>{account.label}</option>)}
        </select></label>
        <button type="button" onClick={() => { setSetupProfileId(activeProfileId ?? undefined); setShowAccountSetup(true); }}>Konto hinzufügen</button>
        <form onSubmit={(event) => { event.preventDefault(); void invoke<{ profiles: Profile[]; activeProfileId: string }>('create_profile', { name: newProfileName }).then((registry) => { setProfiles(registry.profiles); setActiveProfileId(registry.activeProfileId); setSetupProfileId(registry.activeProfileId); setShowAccountSetup(true); setNewProfileName(''); }).catch((reason) => setError(String(reason))); }}>
          <input aria-label="Neues Profil" placeholder="Neues Profil" maxLength={64} value={newProfileName} onChange={(event) => setNewProfileName(event.target.value)} />
          <button type="submit" disabled={!newProfileName.trim()}>Profil anlegen</button>
        </form>
      </div>
      {error && <div className="desktop-inline-error" role="alert">{error}</div>}
      {showAccountSetup && <div className="desktop-setup-overlay"><AccountSetup profileId={setupProfileId} onComplete={() => window.location.reload()} /><button type="button" className="desktop-setup-cancel" onClick={() => setShowAccountSetup(false)}>Abbrechen</button></div>}
      <button className={`desktop-network-toggle is-${connectivity}`} type="button" aria-pressed={offlineMode} onClick={() => { void toggleOfflineMode().catch(() => setConnectivity('offline')); }} title={offlineMode ? 'Offline-Modus beenden und sofort verbinden' : connectivity === 'offline' ? 'Verbindung erneut versuchen' : 'Offline-Modus einschalten'}>
        <span className="desktop-network-indicator" aria-hidden="true" />
        {offlineMode ? 'Offline · Nur lokal' : connectivity === 'online' ? 'Online · Verbunden' : connectivity === 'checking' ? 'Verbinde …' : 'Offline · Verbindung getrennt'}
      </button>
      <Routes /><PluginDialogHost /><PluginConsentDialog /><PluginConnectDialog />
      <PluginAppEvents /><PluginPageNavigator /><ExternalRecipientConfirmHost />
    </TourProvider></RateLimitToastProvider></ThemeProvider>
  </NextIntlClientProvider>;
}

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
  const profileMenuRef = useRef<HTMLDetailsElement>(null);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('Lokale Sitzung laden …');
  const [writes, setWrites] = useState({ pending: 0, blocked: 0, pendingDeletes: 0, blockedDeletes: 0 });
  const [offlineMode, setOfflineMode] = useState(false);
  const [connectivity, setConnectivity] = useState<'syncing' | 'online' | 'offline'>('syncing');
  const [syncPhase, setSyncPhase] = useState('Sitzung');
  const offlineModeRef = useRef(false);
  useEffect(() => {
    const closeProfileMenu = (event: PointerEvent) => {
      if (event.target instanceof Node && !profileMenuRef.current?.contains(event.target)) {
        profileMenuRef.current?.removeAttribute('open');
      }
    };
    const closeProfileMenuOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && profileMenuRef.current?.open) {
        profileMenuRef.current.open = false;
      }
    };
    document.addEventListener('pointerdown', closeProfileMenu);
    document.addEventListener('keydown', closeProfileMenuOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeProfileMenu);
      document.removeEventListener('keydown', closeProfileMenuOnEscape);
    };
  }, []);
  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let syncing = false;
    let syncPending = false;
    let pushTimer: number | undefined;
    let lastFinished = -Infinity;
    let pushConnected = false;
    let pushStatusVersion = 0;
    let stopPushStatus = () => {};
    let armSafety = () => {};
    let serverReached = false;
    let networkAvailable = navigator.onLine;
    const onServerReachable = () => { serverReached = true; };
    const onSyncProgress = (event: Event) => {
      if (active && !offlineModeRef.current && networkAvailable) {
        setConnectivity('syncing');
        setSyncPhase((event as CustomEvent<string>).detail);
      }
    };
    let visibilityCleanup = () => {};
    let stopPush = () => {};
    let requestSync = (_reason = 'manual') => {};
    const updateWrites = () => { void invoke<typeof writes>('write_status').then((result) => { if (active) setWrites(result); }).catch(() => {}); };
    const queued = () => { updateWrites(); requestSync('write'); };
    const storageError = () => {
      void invoke('log_frontend_event', { level: 'error', area: 'storage', event: 'ui_storage_write_failed' }).catch(() => {});
      setStatus('Lokales Speichern fehlgeschlagen. Bitte Speicherplatz prüfen.');
    };
    const onOffline = () => {
      networkAvailable = false;
      if (!offlineModeRef.current) {
        setConnectivity('offline');
        setStatus('Offline · lokale Daten verfügbar');
        void invoke('set_network_enabled', { enabled: false }).catch(() => {});
      }
    };
    const onOnline = () => {
      networkAvailable = true;
      if (!offlineModeRef.current) {
        setConnectivity('syncing');
        void invoke('set_network_enabled', { enabled: true }).then(() => {
          window.dispatchEvent(new Event('desktop-sync-requested'));
        }).catch(() => {});
      }
    };
    const onSyncRequested = () => requestSync('manual');
    window.addEventListener('desktop-storage-error', storageError);
    window.addEventListener('desktop-write-queued', queued);
    window.addEventListener('online', onOnline);
    window.addEventListener('offline', onOffline);
    window.addEventListener('desktop-sync-requested', onSyncRequested);
    window.addEventListener('desktop-server-reachable', onServerReachable);
    window.addEventListener('desktop-sync-progress', onSyncProgress);
    void listen('jmap-push', () => {
      window.clearTimeout(pushTimer);
      pushTimer = window.setTimeout(() => requestSync('push'), 750);
    }).then((unlisten) => {
      if (active) stopPush = unlisten;
      else unlisten();
    }).catch(() => {});
    const applyPushStatus = (connected: boolean) => {
      const reconnect = connected && !pushConnected;
      pushConnected = connected;
      if (connected) {
        if (!syncPending) window.clearTimeout(timer);
        if (reconnect && !offlineModeRef.current && networkAvailable) requestSync('push_reconnect');
      } else armSafety();
    };
    void listen<boolean>('jmap-push-status', (event) => {
      pushStatusVersion++;
      applyPushStatus(event.payload);
    }).then(async (unlisten) => {
      if (!active) { unlisten(); return; }
      stopPushStatus = unlisten;
      const version = pushStatusVersion;
      const connected = await invoke<boolean>('push_status').catch(() => false);
      if (active && version === pushStatusVersion) applyPushStatus(connected);
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
      setConnectivity(enabled && navigator.onLine ? 'syncing' : 'offline');
      await invoke('set_network_enabled', { enabled });
      return bootstrapBulwark();
    };
    start().then((result) => {
      if (!result) return;
      if (!active) return;
      setInfo(result);
      const sync = (reason = 'startup') => {
        if (offlineModeRef.current || !networkAvailable) {
          setConnectivity('offline');
          setStatus('Offline · lokale Daten verfügbar');
          void invoke('set_network_enabled', { enabled: false }).catch(() => {});
          return;
        }
        void invoke('log_frontend_event', { level: 'info', area: 'sync', event: syncing ? 'sync_requested_while_running' : 'sync_started', code: reason }).catch(() => {});
        if (syncing) { syncPending = true; return; }
        window.clearTimeout(timer);
        syncing = true;
        syncPending = false;
        serverReached = false;
        setSyncPhase('Sitzung');
        setConnectivity('syncing');
        setStatus('Synchronisierung …');
        synchronizeBulwark().then(() => { if (active && !offlineModeRef.current && networkAvailable) { setConnectivity('online'); setStatus('Lokal verfügbar · Abgleich abgeschlossen'); } })
          .catch((reason) => {
            void invoke('log_frontend_event', { level: 'error', area: 'sync', event: 'ui_sync_failed' }).catch(() => {});
            if (active && !offlineModeRef.current && networkAvailable) {
              setConnectivity(serverReached ? 'online' : 'offline');
              const message = reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : 'Lokaler Datenbestand bleibt verfügbar';
              setStatus(`Abgleich fehlgeschlagen · ${message}`);
            }
          }).finally(() => {
            void invoke('log_frontend_event', { level: 'info', area: 'sync', event: 'sync_finished' }).catch(() => {});
            syncing = false;
            lastFinished = performance.now();
            updateWrites();
            if (active && !offlineModeRef.current && networkAvailable) {
              if (syncPending) timer = window.setTimeout(() => sync('pending'), 750);
              else armSafety();
            }
          });
      };
      requestSync = sync;
      armSafety = () => {
        if (!pushConnected && !syncing && !offlineModeRef.current && networkAvailable) {
          window.clearTimeout(timer);
          timer = window.setTimeout(() => sync('fallback'), 60_000);
        }
      };
      const onVisibilityChange = () => {
        if (document.visibilityState === 'visible' && !pushConnected && !syncing && performance.now() - lastFinished >= 60_000) sync('visibility');
      };
      document.addEventListener('visibilitychange', onVisibilityChange);
      visibilityCleanup = () => document.removeEventListener('visibilitychange', onVisibilityChange);
      updateWrites(); sync();
    }).catch((reason) => {
      void invoke('log_frontend_event', { level: 'error', area: 'ui', event: 'bootstrap_failed' }).catch(() => {});
      if (active) setError(typeof reason === 'string' ? reason : 'Desktop-Start fehlgeschlagen.');
    });
    return () => { active = false; window.clearTimeout(timer); window.clearTimeout(pushTimer); visibilityCleanup(); stopPush(); stopPushStatus(); window.removeEventListener('desktop-storage-error', storageError); window.removeEventListener('desktop-write-queued', queued); window.removeEventListener('online', onOnline); window.removeEventListener('offline', onOffline); window.removeEventListener('desktop-sync-requested', onSyncRequested); window.removeEventListener('desktop-server-reachable', onServerReachable); window.removeEventListener('desktop-sync-progress', onSyncProgress); };
  }, []);
  const toggleOfflineMode = async () => {
    if (!offlineMode && connectivity === 'offline') {
      await invoke('set_network_enabled', { enabled: true });
      setConnectivity('syncing');
      window.dispatchEvent(new Event('desktop-sync-requested'));
      return;
    }
    const next = !offlineMode;
    offlineModeRef.current = next;
    setOfflineMode(next);
    if (next) setStatus('Offline · lokale Daten verfügbar');
    await invoke('desktop_kv_put', { key: 'ui/network-mode', value: next ? 'offline' : 'auto' });
    const enabled = !next;
    setConnectivity(enabled && navigator.onLine ? 'syncing' : 'offline');
    await invoke('set_network_enabled', { enabled });
    if (enabled) window.dispatchEvent(new Event('desktop-sync-requested'));
  };
  if (needsSetup) return <AccountSetup profileId={setupProfileId} onComplete={() => window.location.reload()} />;
  if (!info) return <div className="desktop-start"><h1>Herakles Mail</h1><p role="status">{error || status}</p></div>;
  return <NextIntlClientProvider locale="de" messages={messages} timeZone="Europe/Belgrade">
    <ThemeProvider><RateLimitToastProvider><TourProvider>
      <footer className="desktop-statusbar" aria-label="Desktop-Status">
      <div className="desktop-status" role="status" title="Im Test-Schreibmodus sind Gelesen/Ungelesen, Markierungen, Verschieben, Papierkorb und Entwurfsaktionen angebunden.">{info.readOnly ? 'Nur Lesen' : 'Test-Schreibmodus'} · {status}{writes.pending > 0 && ` · ${writes.pending} ausstehend`}{writes.blocked > 0 && ` · ${writes.blocked} abgelehnt`}{writes.pendingDeletes > 0 && ` · ${writes.pendingDeletes} Entwurfslöschung(en) ausstehend`}{writes.blockedDeletes > 0 && ` · ${writes.blockedDeletes} Entwurfslöschung(en) blockiert`}</div>
      <details className="desktop-profile-menu" ref={profileMenuRef}>
        <summary>Profile &amp; Konten</summary>
        <div className="desktop-profile-panel">
        <label>Profil <select value={activeProfileId ?? ''} onChange={(event) => { void invoke('switch_profile', { profileId: event.target.value }).then(() => window.location.reload()).catch((reason) => setError(String(reason))); }}>
          {profiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
        </select></label>
        <label>Konto <select value={profiles.find((profile) => profile.id === activeProfileId)?.activeAccountId ?? ''} onChange={(event) => { if (activeProfileId) void invoke('switch_profile_account', { profileId: activeProfileId, accountId: event.target.value }).then(() => window.location.reload()).catch((reason) => setError(String(reason))); }}>
          {(profiles.find((profile) => profile.id === activeProfileId)?.accounts ?? []).map((account) => <option key={account.id} value={account.id}>{account.label}</option>)}
        </select></label>
        <button type="button" onClick={() => { setSetupProfileId(activeProfileId ?? undefined); setShowAccountSetup(true); profileMenuRef.current?.removeAttribute('open'); }}>Konto hinzufügen</button>
        <form onSubmit={(event) => { event.preventDefault(); void invoke<{ profiles: Profile[]; activeProfileId: string }>('create_profile', { name: newProfileName }).then((registry) => { setProfiles(registry.profiles); setActiveProfileId(registry.activeProfileId); setSetupProfileId(registry.activeProfileId); setShowAccountSetup(true); setNewProfileName(''); profileMenuRef.current?.removeAttribute('open'); }).catch((reason) => setError(String(reason))); }}>
          <input aria-label="Neues Profil" placeholder="Neues Profil" maxLength={64} value={newProfileName} onChange={(event) => setNewProfileName(event.target.value)} />
          <button type="submit" disabled={!newProfileName.trim()}>Neues Profil anlegen</button>
        </form>
        {error && <p className="desktop-profile-error" role="alert">{error}</p>}
        </div>
      </details>

      <button className={`desktop-network-toggle is-${connectivity}`} type="button" aria-pressed={offlineMode} onClick={() => { void toggleOfflineMode().catch(() => setConnectivity('offline')); }} title={offlineMode ? 'Offline-Modus beenden und sofort verbinden' : connectivity === 'offline' ? 'Verbindung erneut versuchen' : 'Offline-Modus einschalten'}>
        <span className="desktop-network-indicator" aria-hidden="true" />
        {offlineMode ? 'Offline · Nur lokal' : connectivity === 'online' ? 'Online · Verbunden' : connectivity === 'syncing' ? `Sync · ${syncPhase}` : 'Offline · Verbindung getrennt'}
      </button>
      </footer>
      {showAccountSetup && <div className="desktop-setup-overlay"><AccountSetup profileId={setupProfileId} onComplete={() => window.location.reload()} /><button type="button" className="desktop-setup-cancel" onClick={() => setShowAccountSetup(false)}>Abbrechen</button></div>}
      <div className="desktop-workspace">
      <Routes /><PluginDialogHost /><PluginConsentDialog /><PluginConnectDialog />
      <PluginAppEvents /><PluginPageNavigator /><ExternalRecipientConfirmHost />
      </div>
    </TourProvider></RateLimitToastProvider></ThemeProvider>
  </NextIntlClientProvider>;
}

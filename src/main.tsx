import { createRoot } from 'react-dom/client';
import { isTauri } from '@tauri-apps/api/core';
import { installDesktopStorage } from './desktop/storage';

const root = createRoot(document.getElementById('root')!);
if (!isTauri()) {
  root.render(<main style={{ padding: 48, fontFamily: 'system-ui' }}><h1>Herakles Mail</h1>
    <p>Bitte mit <code>npm run tauri dev</code> starten. Die Oberfläche verwendet den nativen redb-Speicher.</p></main>);
} else {
  installDesktopStorage().then(async () => {
    const { default: BulwarkApp } = await import('./desktop/BulwarkApp');
    root.render(<BulwarkApp />);
  }).catch(() => root.render(<p>Der lokale Desktop-Speicher konnte nicht geladen werden.</p>));
}

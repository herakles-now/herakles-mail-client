import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

export function googleBuildConfiguration(document) {
  const client = document?.installed;
  if (!client || typeof client.client_id !== 'string' || !/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(client.client_id)
    || client.client_id.length > 512 || (client.client_secret !== undefined && (typeof client.client_secret !== 'string' || client.client_secret.length > 1024 || /[\x00-\x1f\x7f]/.test(client.client_secret)))) {
    throw new Error('Bitte die heruntergeladene JSON-Datei eines Google-OAuth-Clients vom Typ Desktop-App verwenden.');
  }
  return { HERAKLES_GOOGLE_CLIENT_ID: client.client_id, HERAKLES_GOOGLE_CLIENT_SECRET: client.client_secret ?? '' };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [mode, path] = process.argv.slice(2);
    if (!['dev', 'build', 'check'].includes(mode) || !path || process.argv.length !== 4) throw new Error('Aufruf: npm run tauri:google -- <dev|build|check> <OAuth-JSON-Datei>');
    let document;
    try { document = JSON.parse(readFileSync(resolve(path), 'utf8').replace(/^\uFEFF/, '')); }
    catch { throw new Error('Google-OAuth-Datei konnte nicht gelesen werden. Pfad und JSON-Format prüfen.'); }
    const environment = googleBuildConfiguration(document);
    if (mode === 'check') console.log('Google-Desktop-Konfiguration ist gültig. Keine Anmeldung oder Übertragung ausgeführt.');
    else {
      const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
      const result = spawnSync(process.execPath, [resolve(root, 'node_modules/@tauri-apps/cli/tauri.js'), mode], { cwd: root, env: { ...process.env, ...environment }, stdio: 'inherit', shell: false });
      if (result.error) throw new Error('Tauri konnte nicht gestartet werden. Abhängigkeiten und Toolchain prüfen.');
      process.exitCode = result.status ?? 1;
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { googleBuildConfiguration } from './tauri-google.mjs';

test('Google desktop JSON supplies only native build configuration', () => {
  assert.deepEqual(googleBuildConfiguration({ installed: { client_id: 'synthetic.apps.googleusercontent.com', client_secret: 'desktop-client', auth_uri: 'https://untrusted.example', token_uri: 'https://untrusted.example' } }), {
    HERAKLES_GOOGLE_CLIENT_ID: 'synthetic.apps.googleusercontent.com', HERAKLES_GOOGLE_CLIENT_SECRET: 'desktop-client',
  });
  assert.equal(googleBuildConfiguration({ installed: { client_id: 'synthetic.apps.googleusercontent.com' } }).HERAKLES_GOOGLE_CLIENT_SECRET, '');
});
test('Google helper refuses web clients and malformed configuration without exposing input', () => {
  for (const document of [{ web: { client_id: 'synthetic.apps.googleusercontent.com' } }, {}, { installed: { client_id: 'https://example.test' } }, { installed: { client_id: 'synthetic.apps.googleusercontent.com', client_secret: 'synthetic\nsecret' } }]) {
    assert.throws(() => googleBuildConfiguration(document), { message: 'Bitte die heruntergeladene JSON-Datei eines Google-OAuth-Clients vom Typ Desktop-App verwenden.' });
  }
});

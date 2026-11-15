import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const script = fileURLToPath(new URL('./stage-release.mjs', import.meta.url));
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'herakles-release-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'src-tauri'), { recursive: true });
  writeFileSync(join(root, 'package.json'), '{"version":"0.1.0"}');
  writeFileSync(join(root, 'src-tauri/tauri.conf.json'), '{"version":"0.1.0"}');
  writeFileSync(join(root, 'src-tauri/Cargo.toml'), '[package]\nversion = "0.1.0"\n');
  const run = (...args) => spawnSync(process.execPath, [script, ...args], {
    cwd: root, encoding: 'utf8',
    env: { ...process.env, GITHUB_REF_TYPE: 'tag', GITHUB_REF_NAME: 'v0.1.0' },
  });
  return { root, run };
}

test('release checks reject version mismatch before staging', (t) => {
  const { root, run } = fixture(t);
  assert.equal(run('validate').status, 0);
  writeFileSync(join(root, 'package.json'), '{"version":"0.2.0"}');
  assert.notEqual(run('validate').status, 0);
});

test('staging rejects unknown targets and ambiguous installers', (t) => {
  const { root, run } = fixture(t);
  assert.notEqual(run('stage', 'wrong-target', 'windows-x64').status, 0);
  const bundle = join(root, 'src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis');
  mkdirSync(bundle, { recursive: true });
  writeFileSync(join(bundle, 'installer.exe'), 'test package');
  assert.equal(run('stage', 'x86_64-pc-windows-msvc', 'windows-x64').status, 0);
  assert.equal(readFileSync(join(root, 'release-files/Herakles-Mail-windows-x64-setup.exe'), 'utf8'), 'test package');
  writeFileSync(join(bundle, 'other.exe'), 'other package');
  assert.notEqual(run('stage', 'x86_64-pc-windows-msvc', 'windows-x64').status, 0);
});

test('checksums require every platform and source archive', (t) => {
  const { root, run } = fixture(t);
  const output = join(root, 'release-files');
  mkdirSync(output);
  assert.notEqual(run('checksums').status, 0);
  const names = [
    'Herakles-Mail-windows-x64-setup.exe', 'Herakles-Mail-linux-x64.AppImage',
    'Herakles-Mail-linux-x64.deb', 'Herakles-Mail-macos-arm64.dmg',
    'Herakles-Mail-macos-x64.dmg', 'Herakles-Mail-source.tar.gz',
  ];
  for (const name of names) writeFileSync(join(output, name), name);
  assert.equal(run('checksums').status, 0);
  const hashes = readFileSync(join(output, 'SHA256SUMS'), 'utf8').trim().split('\n');
  assert.equal(hashes.length, 6);
  for (const name of names) {
    assert.ok(hashes.includes(`${createHash('sha256').update(name).digest('hex')}  ${name}`));
  }
});

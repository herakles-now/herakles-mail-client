import { readFileSync, readdirSync, mkdirSync, copyFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

const platforms = {
  'windows-x64': ['x86_64-pc-windows-msvc', [['nsis', '.exe', '-setup.exe']]],
  'linux-x64': ['x86_64-unknown-linux-gnu', [['appimage', '.AppImage', '.AppImage'], ['deb', '.deb', '.deb']]],
  'macos-arm64': ['aarch64-apple-darwin', [['dmg', '.dmg', '.dmg']]],
  'macos-x64': ['x86_64-apple-darwin', [['dmg', '.dmg', '.dmg']]],
};
const output = 'release-files';
const [command, target, platform] = process.argv.slice(2);
const json = (path) => JSON.parse(readFileSync(path, 'utf8'));
const version = json('package.json').version;
if (json('src-tauri/tauri.conf.json').version !== version ||
    !readFileSync('src-tauri/Cargo.toml', 'utf8').includes(`version = "${version}"`)) {
  throw new Error('Package, Tauri and Cargo versions must match');
}
if (process.env.GITHUB_REF_TYPE === 'tag' && process.env.GITHUB_REF_NAME !== `v${version}`) {
  throw new Error('Release tag must match the package version');
}
const names = Object.entries(platforms).flatMap(([name, [, bundles]]) =>
  bundles.map(([, , suffix]) => `Herakles-Mail-${name}${suffix}`));
if (command === 'stage') {
  const config = platforms[platform];
  if (!config || config[0] !== target) throw new Error('Unknown release target');
  mkdirSync(output, { recursive: true });
  for (const [directory, extension, suffix] of config[1]) {
    const folder = join('src-tauri', 'target', target, 'release', 'bundle', directory);
    const matches = readdirSync(folder).filter((file) => file.endsWith(extension));
    if (matches.length !== 1) throw new Error(`Expected one ${extension} package in ${folder}`);
    copyFileSync(join(folder, matches[0]), join(output, `Herakles-Mail-${platform}${suffix}`));
  }
} else if (command === 'checksums') {
  const expected = [...names, 'Herakles-Mail-source.tar.gz'].sort();
  const actual = readdirSync(output).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error('Incomplete or unexpected release downloads');
  const lines = expected.map((name) => {
    const bytes = readFileSync(join(output, name));
    if (!bytes.length) throw new Error(`Empty download: ${name}`);
    return `${createHash('sha256').update(bytes).digest('hex')}  ${name}`;
  });
  writeFileSync(join(output, 'SHA256SUMS'), `${lines.join('\n')}\n`);
} else if (command !== 'validate') {
  throw new Error('Expected validate, stage or checksums');
}

// Build Bulwark's original plugin runtime without changing the upstream checkout.
import { build } from 'esbuild';
import postcss from 'postcss';
import tailwind from '@tailwindcss/postcss';
import { readFile, writeFile, mkdir, cp } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const upstream = path.join(root, 'upstream/bulwark');
const outdir = path.join(root, 'public/plugin-sandbox-assets');
await mkdir(outdir, { recursive: true });
const result = await build({
  entryPoints: { runtime: path.join(upstream, 'lib/plugin-sandbox/sandbox-entry.ts') },
  bundle: true, splitting: true, format: 'esm', platform: 'browser', target: 'es2022',
  outdir, entryNames: '[name]', chunkNames: 'chunk-[hash]', minify: true,
  legalComments: 'none', jsx: 'automatic', alias: { '@': upstream },
  define: { 'process.env.NODE_ENV': '"production"', 'process.env': '{}' },
  loader: { '.css': 'empty' }, metafile: true,
});
const entry = Object.entries(result.metafile.outputs).find(([, value]) => value.entryPoint)?.[0];
if (!entry) throw new Error('No Bulwark sandbox entry emitted');
const cssPath = path.join(upstream, 'app/(sandbox)/plugin-kit.css');
const css = await postcss([tailwind({ optimize: { minify: true } })]).process(await readFile(cssPath, 'utf8'), { from: cssPath });
await writeFile(path.join(outdir, 'kit.css'), css.css);
const script = path.basename(entry);
await writeFile(path.join(outdir, 'manifest.json'), JSON.stringify({ script, style: 'kit.css' }));
await mkdir(path.join(root, 'public/plugin-sandbox'), { recursive: true });
await writeFile(path.join(root, 'public/plugin-sandbox/index.html'), `<!doctype html><html><head><meta charset="utf-8"><title>Bulwark plugin sandbox</title><link rel="stylesheet" href="/plugin-sandbox-assets/kit.css"><script type="module" src="/plugin-sandbox-assets/${script}"></script></head><body style="margin:0;background:transparent"><div id="plugin-sandbox-root"></div></body></html>`);
for (const name of ['branding', 'notification', 'apple-touch-icon.png', 'icon-192x192.png', 'icon-512x512.png']) {
  await cp(path.join(upstream, 'public', name), path.join(root, 'public', name), { recursive: true });
}
console.log('Bulwark sandbox and public assets prepared.');

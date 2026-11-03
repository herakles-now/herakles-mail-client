import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
import tailwind from '@tailwindcss/postcss';

const file = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig(({ mode }) => ({
  server: { watch: { ignored: ['**/src-tauri/target/**'] } },
  plugins: [{
    name: 'bulwark-desktop-sandbox',
    transform(code, id) {
      if (!id.replaceAll('\\', '/').endsWith('/lib/plugin-sandbox/host-bridge.ts')) return;
      const needle = 'withBasePath(this.privileged ? SANDBOX_PRIVILEGED_PATH : SANDBOX_PATH)';
      if (!code.includes(needle)) throw new Error('Bulwark sandbox integration changed upstream');
      // Opaque plugin frames run on a separate native protocol, with no IPC access.
      return code
        .replace(needle, 'this.privileged ? (() => { throw new Error("Privileged Add-ons sind im Desktop-Client noch nicht unterstützt."); })() : globalThis.__DESKTOP_PLUGIN_URL__')
        .replace(/process\.env\.NODE_ENV\s*===\s*["']development["']/g, 'false');
    },
  }],
  resolve: {
    alias: [
      { find: '@/i18n/navigation', replacement: file('./src/desktop/navigation.tsx') },
      { find: /^(?:@\/lib\/|\.\.?\/)plugin-storage$/, replacement: file('./src/desktop/plugin-storage.ts') },
      { find: 'next/navigation', replacement: file('./src/desktop/navigation.tsx') },
      { find: 'next/dynamic', replacement: file('./src/desktop/dynamic.tsx') },
      { find: '@', replacement: file('./upstream/bulwark') },
    ],
    dedupe: ['react', 'react-dom', 'zustand', 'next-intl'],
  },
  define: {
    'process.env.NODE_ENV': JSON.stringify(mode === 'production' ? 'production' : 'development'),
    'process.env.NEXT_PUBLIC_APP_VERSION': JSON.stringify('1.20.0'),
    'process.env.NEXT_PUBLIC_BULWARK_LITE': JSON.stringify('0'),
    'process.env.NEXT_PUBLIC_BASE_PATH': JSON.stringify(''),
    'process.env': '{}',
  },
  css: { postcss: { plugins: [tailwind()] } },
  build: {
    chunkSizeWarningLimit: 1500,
    rollupOptions: { onwarn(warning, warn) {
      if (warning.code === 'MODULE_LEVEL_DIRECTIVE' || warning.code === 'SOURCEMAP_ERROR') return;
      warn(warning);
    } },
  },
}));

import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
import tailwind from '@tailwindcss/postcss';

const file = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig(({ mode }) => ({
  server: { watch: { ignored: ['**/src-tauri/target/**'] } },
  plugins: [{
    name: 'bulwark-desktop-sandbox',
    enforce: 'pre',
    transform(code, id) {
      if (id.replaceAll('\\', '/').endsWith('/components/email/calendar-invitation-banner.tsx')) {
        const needle = '      {/* Content column */}';
        const content = '      <div className="flex-1 min-w-0 space-y-2">';
        if (code.split(needle).length !== 2 || code.split(content).length !== 2) throw new Error('Bulwark invitation integration changed upstream');
        const method = "  const method = detectedMethod !== 'unknown' ? detectedMethod : rawIcsMethod;";
        if (code.split(method).length !== 2) throw new Error('Bulwark invitation method detection changed upstream');
        code = code.replace(method, "  const method = rawIcsMethod !== 'unknown' ? rawIcsMethod : detectedMethod;");
        return `import { InvitationUpdate } from ${JSON.stringify(file('./src/desktop/InvitationUpdate.tsx').replaceAll('\\', '/'))};\n` + code.replace(content, `${content}
        <InvitationUpdate incoming={parsedEvent} method={method} client={client} accountId={sharedMailAccount || client?.getCalendarsAccountId()} trusted={trustAssessment?.level === 'trusted' || trustAssessment?.reason === 'sender_mismatch'} />`);
      }
      if (id.replaceAll('\\', '/').endsWith('/components/contacts/contact-detail.tsx')) {
        const needle = 'onClick={onEdit} className="gap-1.5 touch-manipulation"';
        if (code.split(needle).length !== 2) throw new Error('Bulwark contact detail integration changed upstream');
        return code.replace(needle, 'onClick={onEdit} disabled title="Kontakte sind derzeit nur lesend verfügbar" className="gap-1.5 touch-manipulation"');
      }
      if (id.replaceAll('\\', '/').endsWith('/components/contacts/contacts-sidebar.tsx')) {
        const needle = 'label={t("new")}';
        if (code.split(needle).length !== 2) throw new Error('Bulwark contacts sidebar integration changed upstream');
        return code.replace(needle, `${needle} disabled`);
      }
      if (id.replaceAll('\\', '/').endsWith('/components/calendar/calendar-app.tsx')) {
        const needle = '  const openEditModal = useCallback((event: CalendarEvent, startInForm = false) =>';
        if (!code.includes(needle)) throw new Error('Bulwark calendar integration changed upstream');
        return code.replace(needle, `  useEffect(() => {
    const open = (event: Event) => { openEditModal((event as CustomEvent<CalendarEvent>).detail, true); };
    window.addEventListener('desktop-open-calendar-event', open);
    return () => window.removeEventListener('desktop-open-calendar-event', open);
  });
${needle}`);
      }
      if (id.replaceAll('\\', '/').endsWith('/lib/jmap/client.ts')) {
        const needle = '    let response = await this.submissionRequest(methodCalls);';
        if (code.split(needle).length !== 2) throw new Error('Bulwark send integration changed upstream');
        return code.replace(needle, `    const desktopResult = await (globalThis as unknown as { __DESKTOP_QUEUE_SUBMISSION__: (calls: JMAPMethodCall[], draftId?: string, draftAccountId?: string) => Promise<unknown> }).__DESKTOP_QUEUE_SUBMISSION__(methodCalls, draftId, options?.draftAccountId);\n    if ((desktopResult as SendEmailResult).queued) return desktopResult as SendEmailResult;\n    let response = desktopResult as JMAPResponse;`);
      }
      if (id.replaceAll('\\', '/').endsWith('/components/mail/mail-app.tsx')) {
        const needle = '  const handleEditDraft = async (email?: Email, source?: DraftSource) => {';
        if (!code.includes(needle)) throw new Error('Bulwark draft integration changed upstream');
        return code.replace(needle, `  useEffect(() => {
    const open = (event: Event) => { void handleEditDraft((event as CustomEvent<Email>).detail); };
    window.addEventListener('desktop-open-draft', open);
    return () => window.removeEventListener('desktop-open-draft', open);
  });
${needle}`);
      }
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

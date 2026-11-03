import { useSyncExternalStore, useMemo } from 'react';
import type { AnchorHTMLAttributes, MouseEvent } from 'react';

const listeners = new Set<() => void>();
const notify = () => { for (const listener of listeners) listener(); };
window.addEventListener('popstate', notify);
const subscribe = (callback: () => void) => { listeners.add(callback); return () => { listeners.delete(callback); }; };
const snapshot = () => window.location.pathname + window.location.search;
export function navigate(href: string, replace = false) {
  const url = new URL(href, window.location.origin);
  if (url.origin !== window.location.origin) throw new Error('External navigation is not supported by the desktop router.');
  window.history[replace ? 'replaceState' : 'pushState']({}, '', url.pathname + url.search + url.hash);
  notify();
}
export function usePathname() {
  return useSyncExternalStore(subscribe, snapshot).split('?')[0] || '/';
}
export function useSearchParams() {
  const location = useSyncExternalStore(subscribe, snapshot);
  return useMemo(() => new URLSearchParams(location.split('?')[1] || ''), [location]);
}
export function useParams() { return { locale: 'de', segments: usePathname().split('/').filter(Boolean).slice(2) }; }
const router = {
  push: (href: string) => navigate(href), replace: (href: string) => navigate(href, true),
  back: () => history.back(), forward: () => history.forward(), refresh: notify,
  prefetch: async () => {},
};
export function useRouter() { return router; }
export function redirect(href: string): never { navigate(href, true); throw new Error('Desktop redirect'); }
export function notFound(): never { throw new Error('Diese Seite ist im Desktop-Client nicht verfügbar.'); }
export function useSelectedLayoutSegment() { return useParams().segments[0] ?? null; }
export function useSelectedLayoutSegments() { return useParams().segments; }
export function Link({ href, onClick, ...props }: Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { href: string }) {
  return <a {...props} href={href} onClick={(event: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);
    if (!event.defaultPrevented && !event.ctrlKey && !event.metaKey && !event.shiftKey && event.button === 0) {
      event.preventDefault(); navigate(href);
    }
  }} />;
}

import { lazy, Suspense } from 'react';
import type { ComponentType } from 'react';

export default function dynamic<P extends object>(
  loader: () => Promise<ComponentType<P> | { default: ComponentType<P> }>,
  options?: { ssr?: boolean; loading?: ComponentType },
) {
  const Lazy = lazy(async () => {
    const component = await loader();
    return typeof component === 'function' ? { default: component } : component as { default: ComponentType<P> };
  });
  const Loading = options?.loading;
  return function DesktopDynamic(props: P) {
    return <Suspense fallback={Loading ? <Loading /> : null}><Lazy {...props} /></Suspense>;
  };
}

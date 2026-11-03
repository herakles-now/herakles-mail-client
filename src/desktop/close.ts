// Tauri awaits this handler, then destroys the window itself. Do not recursively call close().
export async function flushBeforeClose(
  flush: () => Promise<void>,
  reportFailure: () => void,
  timeoutMs = 3000,
) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      flush(),
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Storage flush timed out')), timeoutMs); }),
    ]);
  } catch {
    // A storage failure must not trap the user in an unclosable window.
    reportFailure();
  } finally { clearTimeout(timer); }
}

type PollingTask = () => Promise<unknown> | unknown;

/**
 * Runs `task` serially: the next run is scheduled `intervalMs` after the previous one settled, so a slow
 * request never overlaps the next. Errors are logged and do not stop the polling. The returned function
 * stops it for good, also while a run is in flight.
 */
export function startPolling(task: PollingTask, intervalMs: number, runImmediately = false): () => void {
  let stopped = false;
  let timeout: ReturnType<typeof setTimeout> | undefined;

  const run = async () => {
    try {
      await task();
    } catch (error) {
      console.error('Polling task failed.', error);
    } finally {
      if (!stopped) timeout = setTimeout(run, intervalMs);
    }
  };

  if (runImmediately) void run();
  else timeout = setTimeout(run, intervalMs);

  return () => {
    stopped = true;
    if (timeout !== undefined) clearTimeout(timeout);
  };
}

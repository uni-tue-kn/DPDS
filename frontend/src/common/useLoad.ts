import { useCallback, useEffect, useState } from 'react';
import { errorMessage } from './format';

export interface Loaded<T> {
  data?: T;
  error?: string;
  /** Fetch again, keeping the current data until the new response arrives. */
  reload: () => void;
}

/** Fetches once on mount (and on `reload()`); ignores responses that arrive after unmount. */
export function useLoad<T>(load: () => Promise<T>): Loaded<T> {
  const [state, setState] = useState<{ data?: T; error?: string }>({});
  const [round, setRound] = useState(0);
  const reload = useCallback(() => setRound((n) => n + 1), []);

  useEffect(() => {
    let alive = true;
    load().then(
      (data) => alive && setState({ data }),
      (e: unknown) => alive && setState({ error: errorMessage(e) }),
    );
    return () => {
      alive = false;
    };
    // Only `round` re-triggers: callers pass an inline closure that must not restart the fetch every render.
  }, [round]);

  return { ...state, reload };
}

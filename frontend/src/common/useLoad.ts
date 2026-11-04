import { useCallback, useEffect, useRef, useState } from 'react';
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
  // Callers pass an inline closure; the latest one is used without restarting the fetch every render.
  const loadRef = useRef(load);
  loadRef.current = load;
  // Only the newest request may set the state; unmounting invalidates the one in flight.
  const latest = useRef(0);

  const reload = useCallback(() => {
    const mine = ++latest.current;
    loadRef.current().then(
      (data) => mine === latest.current && setState({ data }),
      (e: unknown) => mine === latest.current && setState({ error: errorMessage(e) }),
    );
  }, []);

  useEffect(() => {
    reload();
    return () => {
      latest.current++;
    };
  }, [reload]);

  return { ...state, reload };
}

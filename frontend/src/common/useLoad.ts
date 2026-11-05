import { useCallback, useEffect, useRef, useState } from 'react';
import { errorMessage } from './format';

export interface Loaded<T> {
  data?: T;
  error?: string;
  /** A request is in flight: the first one, or a reload (the previous data stays meanwhile). */
  loading: boolean;
  /** Fetch again, keeping the current data until the new response arrives. */
  reload: () => void;
}

/** Fetches once on mount (and on `reload()`); ignores responses that arrive after unmount. */
export function useLoad<T>(load: () => Promise<T>): Loaded<T> {
  const [state, setState] = useState<{ data?: T; error?: string }>({});
  const [loading, setLoading] = useState(true);
  // Callers pass an inline closure; the latest one is used without restarting the fetch every render.
  const loadRef = useRef(load);
  loadRef.current = load;
  // Only the newest request may set the state; unmounting invalidates the one in flight.
  const latest = useRef(0);

  const reload = useCallback(() => {
    const mine = ++latest.current;
    setLoading(true);
    const settle = (next: { data?: T; error?: string }) => {
      if (mine !== latest.current) return;
      setState(next);
      setLoading(false);
    };
    loadRef.current().then(
      (data) => settle({ data }),
      (e: unknown) => settle({ error: errorMessage(e) }),
    );
  }, []);

  useEffect(() => {
    reload();
    return () => {
      latest.current++;
    };
  }, [reload]);

  return { ...state, loading, reload };
}

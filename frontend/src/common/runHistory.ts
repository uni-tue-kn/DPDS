import { useEffect, useState } from 'react';
import * as api from '../api/client';
import type { HistoryLink, HistorySample } from '../api/types';
import { errorMessage } from './format';
import { startPolling } from './polling';

const POLL_MS = 1000;
/** The controller keeps at most this many samples per link (one a second, a day); so does the page. */
export const MAX_SAMPLES = 86400;

/** The series per link index, and the same empty list for every link without samples (stable for the charts' memos). */
export type History = Record<string, HistorySample[]>;
export const NO_SAMPLES: HistorySample[] = [];

/** The newest `MAX_SAMPLES` of `list`. */
const capped = (list: HistorySample[]) => (list.length > MAX_SAMPLES ? list.slice(-MAX_SAMPLES) : list);

/** `old` plus the new samples of `add`; `old` itself when there is nothing new, so unchanged links keep their lists. */
function merge(old: History, add: History): History {
  let out = old;
  for (const [link, list] of Object.entries(add)) {
    if (list.length === 0 && link in old) continue;
    if (out === old) out = { ...old };
    out[link] = capped([...(old[link] ?? NO_SAMPLES), ...list]);
  }
  return out;
}

/** The newest `t_ms` over all links, undefined without samples. */
function newestOf(h: History): number | undefined {
  let newest: number | undefined;
  for (const list of Object.values(h)) {
    const t = list.at(-1)?.t_ms;
    if (t !== undefined && (newest === undefined || t > newest)) newest = t;
  }
  return newest;
}

/**
 * The series of the run the controller holds: everything once, then only what is newer than the newest sample held.
 * Another run (`run_id` changed) starts at 0 again, so its first answer must be a full one. `error` is the message of
 * the last request while it failed, undefined after a success.
 */
export function useRunHistory(): { history: History; links: HistoryLink[]; error?: string } {
  const [history, setHistory] = useState<History>({});
  const [links, setLinks] = useState<HistoryLink[]>([]);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let alive = true;
    let run: number | null | undefined; // undefined: nothing received yet
    let data: History = {};
    const stop = startPolling(
      async () => {
        try {
          const since = newestOf(data);
          let h = await api.getStatsHistory(since);
          if (!alive) return;
          const newRun = h.run_id !== run;
          if (newRun && since !== undefined) {
            h = await api.getStatsHistory();
            if (!alive) return;
          }
          run = h.run_id;
          const next = newRun
            ? Object.fromEntries(Object.entries(h.samples).map(([k, l]) => [k, capped(l)]))
            : merge(data, h.samples);
          if (next !== data) {
            data = next;
            setHistory(next);
          }
          setLinks(h.links);
          setError(undefined);
        } catch (err) {
          if (alive) setError(errorMessage(err));
          throw err; // startPolling logs it and keeps going
        }
      },
      POLL_MS,
      true,
    );
    return () => {
      alive = false;
      stop();
    };
  }, []);

  return { history, links, error };
}

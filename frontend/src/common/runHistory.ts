import { useEffect, useState } from 'react';
import * as api from '../api/client';
import type { HistoryLink, HistorySample, HistoryTotals } from '../api/types';
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
 * the last request while it failed, undefined after a success. `totals` are the run's newest counters per link, from the
 * latest answer.
 */
export function useRunHistory(): {
  history: History;
  links: HistoryLink[];
  totals: Record<string, HistoryTotals>;
  error?: string;
} {
  const [history, setHistory] = useState<History>({});
  const [links, setLinks] = useState<HistoryLink[]>([]);
  const [totals, setTotals] = useState<Record<string, HistoryTotals>>({});
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
          setTotals(h.totals);
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

  return { history, links, totals, error };
}

/**
 * The aggregate of several links' series, at the times every link has a sample: rates are summed, and each drop share is
 * averaged weighted by the link's received rate. The four overload shares are one, in `limiter_pct`. The result has the shape
 * of a link's series, so it is drawn by the same charts. Empty without links or when one link has no sample.
 */
export function sumHistories(lists: HistorySample[][]): HistorySample[] {
  const sums = new Map<number, { n: number; s: HistorySample; lossW: number; overW: number }>();
  for (const list of lists) {
    for (const x of list) {
      let acc = sums.get(x.t_ms);
      if (!acc) {
        const zero = { rx_gbps: 0, tx_gbps: 0, rx_mpps: 0, tx_mpps: 0, loss_pct: 0, limiter_pct: 0 };
        acc = {
          n: 0,
          s: { t_ms: x.t_ms, ...zero, buffer_full_pct: 0, nic_tx_pct: 0, nic_rx_pct: 0 },
          lossW: 0,
          overW: 0,
        };
        sums.set(x.t_ms, acc);
      }
      acc.n += 1;
      acc.s.rx_gbps += x.rx_gbps;
      acc.s.tx_gbps += x.tx_gbps;
      acc.s.rx_mpps += x.rx_mpps;
      acc.s.tx_mpps += x.tx_mpps;
      acc.lossW += x.loss_pct * x.rx_mpps;
      acc.overW += (x.limiter_pct + x.buffer_full_pct + x.nic_tx_pct + x.nic_rx_pct) * x.rx_mpps;
    }
  }
  return [...sums.values()]
    .filter((a) => a.n === lists.length)
    .map(({ s, lossW, overW }) => ({
      ...s,
      loss_pct: s.rx_mpps > 0 ? lossW / s.rx_mpps : 0,
      limiter_pct: s.rx_mpps > 0 ? overW / s.rx_mpps : 0,
    }))
    .sort((a, b) => a.t_ms - b.t_ms);
}

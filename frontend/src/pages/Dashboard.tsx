import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Badge, Button, Card, Spinner } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage, formatElapsed, formatPct, formatRatePill, linkLabel } from '../common/format';
import { startPolling } from '../common/polling';
import { usePortLabeler } from '../common/portLabel';
import { readConfig, startRequestOf, useLocalConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import LoadRunning from './LoadRunning';
import DirectionCard from './DirectionCard';
import PortsTable from './PortsTable';
import type { DirectionStats, EmulationState, EmulationView, HistorySample, StatsView } from '../api/types';

const POLL_MS = 1000;

/** The series per link index, and the same empty list for every link without samples (stable for the charts' memos). */
type History = Record<string, HistorySample[]>;
const NO_SAMPLES: HistorySample[] = [];

const STATE_BG: Record<EmulationState, string> = {
  stopped: 'secondary',
  starting: 'warning',
  running: 'success',
  stopping: 'warning',
  failed: 'danger',
};

/** The log lines with a React key each: the line plus its occurrence number, since a line may repeat. */
function keyedLines(lines: string[]): { key: string; line: string }[] {
  const seen = new Map<string, number>();
  return lines.map((line) => {
    const n = seen.get(line) ?? 0;
    seen.set(line, n + 1);
    return { key: `${n}:${line}`, line };
  });
}

/** `old` plus the new samples of `add`; `old` itself when there is nothing new, so unchanged links keep their lists. */
function merge(old: History, add: History): History {
  let out = old;
  for (const [link, list] of Object.entries(add)) {
    if (list.length === 0 && link in old) continue;
    if (out === old) out = { ...old };
    out[link] = [...(old[link] ?? NO_SAMPLES), ...list];
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

/** The start request for the configuration as it is stored now, so an edit made in another tab is included. */
function storedRequest() {
  const stored = readConfig();
  if (stored.corrupt) throw new Error('The local configuration cannot be read; reset it first.');
  if (stored.doc.links.length === 0) throw new Error('No links configured: nothing to start. Add a link first.');
  return startRequestOf(stored.doc);
}

/** Total RX and TX rate over all links, and the share of dropped packets weighted by each link's received rate. */
function totalsOf(directions: StatsView['directions']) {
  let rxBps = 0;
  let rxPps = 0;
  let txBps = 0;
  let txPps = 0;
  let dropped = 0;
  for (const d of directions) {
    rxBps += d.rx_bps;
    rxPps += d.rx_pps;
    txBps += d.tx_bps;
    txPps += d.tx_pps;
    dropped += d.rx_pps * (d.loss_pct + d.limiter_pct + d.buffer_full_pct + d.nic_tx_pct + d.nic_rx_pct);
  }
  return { rxBps, rxPps, txBps, txPps, droppedPct: rxPps > 0 ? dropped / rxPps : 0 };
}

export default function Dashboard() {
  const [emulation, setEmulation] = useState<EmulationView>();
  const [stats, setStats] = useState<StatsView>();
  const [history, setHistory] = useState<History>({});
  const [pollError, setPollError] = useState<string>();
  const [statsError, setStatsError] = useState<string>();
  /** The stats error the user closed; the warning returns only for a different message. */
  const [dismissed, setDismissed] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [busy, setBusy] = useState<'start' | 'stop' | 'restart'>();
  const label = usePortLabeler();
  const { doc, corrupt } = useLocalConfig();
  /** A restart has sent its stop and waits for a poll that reports the end, then starts. */
  const [waiting, setWaiting] = useState(false);
  /** The last live stats of every link, to name its card after a stop. */
  const lastStats = useRef<Record<number, DirectionStats>>({});

  const start = useCallback(async () => {
    try {
      setEmulation(await api.startEmulation(storedRequest()));
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setBusy(undefined);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    const stopEmulationPoll = startPolling(
      async () => {
        try {
          const e = await api.getEmulation();
          if (!alive) return;
          setEmulation(e);
          setPollError(undefined);
        } catch (err) {
          if (alive) setPollError(errorMessage(err));
        }
      },
      POLL_MS,
      true,
    );
    const stopStatsPoll = startPolling(
      async () => {
        let s: StatsView;
        try {
          s = await api.getStats();
        } catch (err) {
          if (alive) setStatsError(errorMessage(err));
          throw err; // startPolling logs it and keeps going
        }
        if (!alive) return;
        setStatsError(undefined);
        setDismissed(undefined);
        for (const d of s.directions) lastStats.current[d.dir] = d;
        setStats(s);
      },
      POLL_MS,
      true,
    );
    // The series of the run: everything once, then only what is newer than the newest sample held. Another run
    // (`started_at` changed) starts at 0 again, so its first answer must be a full one.
    let run: number | null | undefined; // undefined: nothing received yet
    let data: History = {};
    const stopHistoryPoll = startPolling(
      async () => {
        const since = newestOf(data);
        let h = await api.getStatsHistory(since);
        if (!alive) return;
        const newRun = h.started_at !== run;
        if (newRun && since !== undefined) {
          h = await api.getStatsHistory();
          if (!alive) return;
        }
        run = h.started_at;
        const next = newRun ? h.samples : merge(data, h.samples);
        if (next !== data) {
          data = next;
          setHistory(next);
        }
      },
      POLL_MS,
      true,
    );
    return () => {
      alive = false;
      stopEmulationPoll();
      stopStatsPoll();
      stopHistoryPoll();
    };
  }, []);

  const state = emulation?.state;
  // An action's error is old news once the emulation has changed its state.
  const [errorState, setErrorState] = useState(state);
  if (errorState !== state) {
    setErrorState(state);
    setActionError(undefined);
  }

  // The controller refuses a start (409) until a poll reports that the stop has finished.
  useEffect(() => {
    if (waiting && (state === 'stopped' || state === 'failed')) {
      setWaiting(false);
      void start();
    }
  }, [waiting, state, start]);

  const act = async (which: 'start' | 'stop' | 'restart') => {
    if (busy) return;
    setBusy(which);
    setActionError(undefined);
    if (which === 'start') return start();
    try {
      if (which === 'restart') storedRequest(); // a run is never stopped for a start that cannot happen
      setEmulation(await api.stopEmulation());
      if (which === 'restart') setWaiting(true);
      else setBusy(undefined);
    } catch (err) {
      setActionError(errorMessage(err));
      setBusy(undefined);
    }
  };

  const live = stats && stats.directions.length > 0 ? stats.directions : undefined;
  const totals = live && totalsOf(live);
  const plan = (dir: number) => emulation?.directions.find((p) => p.dir === dir);
  // The controller reports stale: true with no age_ms for a stopped emulation; that is not "old data".
  const showStale = state === 'running' && stats?.stale === true && stats.age_ms != null;
  // After a stop the controller keeps the run's series until the next start: its final charts stay on the page.
  const finished = state === 'stopped' && Object.values(history).some((list) => list.length > 0);
  const shown: { dir: number; stats?: DirectionStats }[] = finished
    ? Object.keys(history)
        .map(Number)
        .filter(Number.isInteger)
        .sort((a, b) => a - b)
        .map((dir) => ({ dir }))
    : (live ?? []).map((d) => ({ dir: d.dir, stats: d }));
  const titleOf = (dir: number, s?: DirectionStats) => {
    const named = s ?? lastStats.current[dir];
    const p = plan(dir);
    if (!named && !p) return `Link ${dir}`;
    return linkLabel(named?.link ?? p?.link, named?.rx ?? p?.rx, named?.tx ?? p?.tx, label);
  };

  return (
    <>
      <h1 className="mb-3">Dashboard</h1>
      <ErrorAlert message={actionError} />
      <ErrorAlert message={pollError} />
      {statsError && statsError !== dismissed && (
        <Alert
          variant="warning"
          role="status"
          dismissible
          onClose={() => setDismissed(statsError)}
          data-testid="stats-warning"
        >
          Statistics are not updating: {statsError}
        </Alert>
      )}
      <Card className="mb-3">
        <Card.Body className="d-flex flex-wrap align-items-center gap-3">
          <span aria-live="polite">
            <Badge
              pill
              bg={state ? STATE_BG[state] : 'secondary'}
              className="state-badge"
              data-testid="state-badge"
              data-state={state ?? 'unknown'}
            >
              {state ?? 'unknown'}
            </Badge>
          </span>
          {totals && (
            <>
              <span className={`status-pill ${state === 'running' ? 'status-pill-active' : ''}`} data-testid="pill-rx">
                <i className="bi bi-arrow-down-circle-fill" aria-hidden="true" />
                RX {formatRatePill(totals.rxBps, totals.rxPps)}
              </span>
              <span className={`status-pill ${state === 'running' ? 'status-pill-active' : ''}`} data-testid="pill-tx">
                <i className="bi bi-arrow-up-circle-fill" aria-hidden="true" />
                TX {formatRatePill(totals.txBps, totals.txPps)}
              </span>
            </>
          )}
          {stats?.elapsed_ms != null && (
            <span className="status-pill" data-testid="pill-elapsed">
              {formatElapsed(stats.elapsed_ms)}
            </span>
          )}
          {totals && totals.droppedPct > 0 && (
            <span className="status-pill" data-testid="pill-dropped">
              dropped {formatPct(totals.droppedPct)}
            </span>
          )}
          {busy === 'restart' ? (
            <Button variant="primary" disabled>
              <Spinner size="sm" aria-hidden="true" /> Restarting…
            </Button>
          ) : state === 'stopping' ? (
            <Button variant="primary" disabled>
              <Spinner size="sm" aria-hidden="true" /> Stopping…
            </Button>
          ) : state === 'starting' || state === 'running' ? (
            <Button variant="primary" disabled={busy !== undefined} onClick={() => act('stop')}>
              <i className="bi bi-stop-fill" aria-hidden="true" /> Stop
            </Button>
          ) : (
            <Button
              variant="success"
              disabled={state === undefined || corrupt || busy !== undefined}
              onClick={() => act('start')}
            >
              <i className="bi bi-play-fill" aria-hidden="true" /> Start
            </Button>
          )}
          {state === 'running' && busy !== 'restart' && (
            <Button variant="outline-primary" disabled={corrupt || busy !== undefined} onClick={() => act('restart')}>
              <i className="bi bi-arrow-clockwise" aria-hidden="true" /> Restart
            </Button>
          )}
          {pendingRestart(doc, emulation).any && (
            <Badge pill bg="warning" text="dark" data-testid="pending-restart">
              Restart needed: links or settings changed
            </Badge>
          )}
          {emulation?.pid != null && (
            <span className="text-body-secondary">
              pid {emulation.pid}, main lcore {emulation.main_lcore ?? 'n/a'}
            </span>
          )}
          {showStale && (
            <Badge pill bg="warning" text="dark" data-testid="stale-indicator">
              Stale data ({((stats?.age_ms ?? 0) / 1000).toFixed(1)} s old)
            </Badge>
          )}
        </Card.Body>
      </Card>
      <LoadRunning emulation={emulation} />
      {emulation?.error && (
        <div className="alert alert-danger" role="alert">
          {emulation.error}
        </div>
      )}
      {emulation && emulation.log_tail.length > 0 && (
        <pre className="log-tail" data-testid="log-tail">
          {keyedLines(emulation.log_tail).map(({ key, line }) => (
            <div key={key}>{line}</div>
          ))}
        </pre>
      )}
      {shown.map(({ dir, stats: s }) => (
        <DirectionCard
          key={dir}
          dir={dir}
          title={titleOf(dir, s)}
          stats={s}
          plan={plan(dir)}
          history={history[String(dir)] ?? NO_SAMPLES}
        />
      ))}
      {stats && stats.ports.length > 0 && <PortsTable ports={stats.ports} label={label} />}
    </>
  );
}

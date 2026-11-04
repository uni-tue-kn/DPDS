import { useCallback, useEffect, useState } from 'react';
import { Alert, Badge, Button, Card, Spinner } from 'react-bootstrap';
import * as api from '../api/client';
import type { DirectionStats, EmulationState, EmulationView, StatsView } from '../api/types';
import { errorMessage, formatElapsed, formatPct, formatRatePill, linkLabel } from '../common/format';
import { readConfig, startRequestOf, useLocalConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import { startPolling } from '../common/polling';
import { usePortLabeler } from '../common/portLabel';
import { NO_SAMPLES, useRunHistory } from '../common/runHistory';
import ErrorAlert from '../components/ErrorAlert';
import DirectionCard from './DirectionCard';
import LoadRunning from './LoadRunning';
import PortsTable from './PortsTable';

const POLL_MS = 1000;

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
  const { history, links, error: historyError } = useRunHistory();
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
        setStats(s);
      },
      POLL_MS,
      true,
    );
    return () => {
      alive = false;
      stopEmulationPoll();
      stopStatsPoll();
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
  // After the run ends (or while it ends) the controller keeps its series until the next start: the final charts stay.
  const ended = state === 'stopping' || state === 'stopped' || state === 'failed' ? state : undefined;
  const finished = ended !== undefined && Object.values(history).some((list) => list.length > 0);
  const shown: { dir: number; stats?: DirectionStats }[] = finished
    ? Object.keys(history)
        .map(Number)
        .filter(Number.isInteger)
        .sort((a, b) => a - b)
        .map((dir) => ({ dir }))
    : (live ?? []).map((d) => ({ dir: d.dir, stats: d }));
  // A finished run is titled by the links of its own history, a live one by its stats or the plan.
  const titleOf = (dir: number, s?: DirectionStats) => {
    if (finished) {
      const l = links[dir];
      return l ? linkLabel(l.name, l.ingress, l.egress, label) : `Link ${dir}`;
    }
    const p = plan(dir);
    if (!s && !p) return `Link ${dir}`;
    return linkLabel(s?.link ?? p?.link, s?.rx ?? p?.rx, s?.tx ?? p?.tx, label);
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
      {historyError && (
        <Alert variant="warning" role="status" data-testid="history-warning">
          The history of the charts is not updating: {historyError}
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
          history={state === 'starting' ? NO_SAMPLES : (history[String(dir)] ?? NO_SAMPLES)}
          ended={finished ? ended : undefined}
        />
      ))}
      {stats && stats.ports.length > 0 && <PortsTable ports={stats.ports} label={label} />}
    </>
  );
}

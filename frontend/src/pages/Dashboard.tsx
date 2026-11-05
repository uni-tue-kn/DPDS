import { useCallback, useEffect, useMemo, useState } from 'react';
import { Alert, Badge, Button, ButtonGroup, Col, Row, Spinner, Tab, Tabs } from 'react-bootstrap';
import * as api from '../api/client';
import type { EmulationState, EmulationView, StatsView } from '../api/types';
import { errorMessage, formatElapsed, formatPct, formatRatePill, linkLabel } from '../common/format';
import { readConfig, startRequestOf, useLocalConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import { startPolling } from '../common/polling';
import { usePortLabeler } from '../common/portLabel';
import { NO_SAMPLES, sumHistories, useRunHistory } from '../common/runHistory';
import ErrorAlert from '../components/ErrorAlert';
import LinkCharts from './LinkCharts';
import LoadRunning from './LoadRunning';
import { type DropCounts, dropsOfCounters, dropsOfTotals } from './LossDonut';

const POLL_MS = 1000;

const NO_DROPS: DropCounts = { loss: 0, limiter: 0, bufferFull: 0, nicTx: 0, imissed: 0, nombuf: 0 };

/** The key of the Summary tab; the keys of the link tabs are their indexes. */
const SUMMARY_TAB = 'summary';

const ENDED_TEXT = { stopping: 'Stopping', stopped: 'Stopped', failed: 'Failed' } as const;

/** Every button of the group has this width, so that Start, Stop, "Stopping…" and "Restarting…" never move the others. */
const BUTTON_STYLE = { minWidth: '8rem' };

const STATE_BG: Record<EmulationState, string> = {
  stopped: 'secondary',
  starting: 'warning',
  running: 'success',
  stopping: 'warning',
  failed: 'danger',
};

const DASH = '–';

const ZERO_PILLS = { rxBps: 0, rxPps: 0, txBps: 0, txPps: 0, droppedPct: 0 };

const addDrops = (a: DropCounts, b: DropCounts): DropCounts => ({
  loss: a.loss + b.loss,
  limiter: a.limiter + b.limiter,
  bufferFull: a.bufferFull + b.bufferFull,
  nicTx: a.nicTx + b.nicTx,
  imissed: a.imissed + b.imissed,
  nombuf: a.nombuf + b.nombuf,
});

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
  const { history, links, totals: runTotalsByLink, error: historyError } = useRunHistory();
  /** The tab the user chose (the Summary or a link index); the Summary while that link is gone. */
  const [selected, setSelected] = useState<string>();
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
  const plan = (dir: number) => emulation?.directions.find((p) => p.dir === dir);
  // The controller reports stale: true with no age_ms for a stopped emulation; that is not "old data".
  const showStale = state === 'running' && stats?.stale === true && stats.age_ms != null;
  // After the run ends (or while it ends) the controller keeps its series until the next start: the final charts stay.
  const ended = state === 'stopping' || state === 'stopped' || state === 'failed' ? state : undefined;
  const finished = ended !== undefined && Object.values(history).some((list) => list.length > 0);
  // Like live statistics: once the run has ended the rates and the loss are 0 (the charts and donuts keep the run).
  const pills = finished ? ZERO_PILLS : live && totalsOf(live);
  const restartNeeded = pendingRestart(doc, emulation).any;
  const shown = useMemo(
    () =>
      finished
        ? Object.keys(history)
            .map(Number)
            .filter(Number.isInteger)
            .sort((a, b) => a - b)
        : (live ?? []).map((d) => d.dir),
    [finished, history, live],
  );
  const starting = state === 'starting';
  const summaryHistory = useMemo(
    () => sumHistories(shown.map((dir) => (starting ? NO_SAMPLES : (history[String(dir)] ?? NO_SAMPLES)))),
    [shown, history, starting],
  );
  const dropsOf = (dir: number): DropCounts => {
    if (finished) {
      const t = runTotalsByLink[String(dir)];
      return t ? dropsOfTotals(t) : NO_DROPS;
    }
    const s = live?.find((d) => d.dir === dir);
    return s ? dropsOfCounters(s.counters) : NO_DROPS;
  };
  // A finished run is titled by the links of its own history, a live one by its stats or the plan.
  const titleOf = (dir: number) => {
    if (finished) {
      const l = links[dir];
      return l ? linkLabel(l.name, l.ingress, l.egress, label) : `Link ${dir}`;
    }
    const s = live?.find((d) => d.dir === dir);
    const p = plan(dir);
    if (!s && !p) return `Link ${dir}`;
    return linkLabel(s?.link ?? p?.link, s?.rx ?? p?.rx, s?.tx ?? p?.tx, label);
  };
  const activeTab = selected !== undefined && shown.includes(Number(selected)) ? selected : SUMMARY_TAB;
  const pillClass = `status-pill ${state === 'running' ? 'status-pill-active' : ''}`;

  return (
    <>
      <h1 className="mb-3">Dashboard</h1>
      <Row className="align-items-center mb-3" data-testid="top-row">
        <Col xs={8} className="text-start d-flex flex-wrap align-items-center gap-3" data-testid="top-pills">
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
          <span className={pillClass} data-testid="pill-rx">
            <i className="bi bi-arrow-down-circle-fill" aria-hidden="true" />
            RX {pills ? formatRatePill(pills.rxBps, pills.rxPps) : DASH}
          </span>
          <span className={pillClass} data-testid="pill-tx">
            <i className="bi bi-arrow-up-circle-fill" aria-hidden="true" />
            TX {pills ? formatRatePill(pills.txBps, pills.txPps) : DASH}
          </span>
          {stats?.elapsed_ms != null && (
            <span className="status-pill" data-testid="pill-elapsed">
              {formatElapsed(stats.elapsed_ms)}
            </span>
          )}
          <span className="status-pill" data-testid="pill-dropped">
            dropped {pills ? formatPct(pills.droppedPct) : DASH}
          </span>
        </Col>
        <Col xs={4} className="text-end">
          <ButtonGroup data-testid="run-buttons">
            <Button
              variant="outline-primary"
              style={BUTTON_STYLE}
              disabled={state !== 'running' || corrupt || busy !== undefined}
              onClick={() => act('restart')}
            >
              <i className="bi bi-arrow-clockwise" aria-hidden="true" /> Restart
            </Button>
            {busy === 'restart' ? (
              <Button variant="primary" style={BUTTON_STYLE} disabled>
                <Spinner size="sm" aria-hidden="true" /> Restarting…
              </Button>
            ) : state === 'stopping' ? (
              <Button variant="primary" style={BUTTON_STYLE} disabled>
                <Spinner size="sm" aria-hidden="true" /> Stopping…
              </Button>
            ) : state === 'starting' || state === 'running' ? (
              <Button variant="primary" style={BUTTON_STYLE} disabled={busy !== undefined} onClick={() => act('stop')}>
                <i className="bi bi-stop-fill" aria-hidden="true" /> Stop
              </Button>
            ) : (
              <Button
                variant="success"
                style={BUTTON_STYLE}
                disabled={state === undefined || corrupt || busy !== undefined}
                onClick={() => act('start')}
              >
                <i className="bi bi-play-fill" aria-hidden="true" /> Start
              </Button>
            )}
          </ButtonGroup>
        </Col>
      </Row>
      <div data-testid="notices">
        <ErrorAlert message={actionError} />
        <ErrorAlert message={pollError} />
        <ErrorAlert message={emulation?.error} />
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
        {(restartNeeded || showStale) && (
          <div className="d-flex flex-wrap gap-3 mb-3">
            {restartNeeded && (
              <Badge pill bg="warning" text="dark" data-testid="pending-restart">
                Restart needed: links or settings changed
              </Badge>
            )}
            {showStale && (
              <Badge pill bg="warning" text="dark" data-testid="stale-indicator">
                Stale data ({((stats?.age_ms ?? 0) / 1000).toFixed(1)} s old)
              </Badge>
            )}
          </div>
        )}
      </div>
      <LoadRunning emulation={emulation} />
      {state === 'failed' && emulation && emulation.log_tail.length > 0 && (
        <details className="mb-3">
          <summary>Show log</summary>
          <pre className="log-tail" data-testid="log-tail">
            {keyedLines(emulation.log_tail).map(({ key, line }) => (
              <div key={key}>{line}</div>
            ))}
          </pre>
        </details>
      )}
      {shown.length > 0 && (
        <Tabs id="link-tabs" transition={false} activeKey={activeTab} onSelect={(k) => k && setSelected(k)}>
          <Tab eventKey={SUMMARY_TAB} title="Summary">
            <div className="pt-3">
              <LinkCharts
                active={activeTab === SUMMARY_TAB}
                history={summaryHistory}
                drops={shown.map(dropsOf).reduce(addDrops, NO_DROPS)}
              />
            </div>
          </Tab>
          {shown.map((dir) => {
            const s = live?.find((d) => d.dir === dir);
            return (
              <Tab
                key={dir}
                eventKey={String(dir)}
                title={
                  <>
                    {titleOf(dir)}
                    {finished && (
                      <Badge pill bg={ended === 'failed' ? 'danger' : 'secondary'} className="ms-2">
                        {ENDED_TEXT[ended]}
                      </Badge>
                    )}
                  </>
                }
              >
                <div className="pt-3">
                  <LinkCharts
                    dir={dir}
                    active={String(dir) === activeTab}
                    history={state === 'starting' ? NO_SAMPLES : (history[String(dir)] ?? NO_SAMPLES)}
                    dirLabel={`${label(s?.rx) || '?'} → ${label(s?.tx) || '?'}`}
                    drops={dropsOf(dir)}
                    live={
                      finished || !s
                        ? undefined
                        : { delayStats: s.delay_stats === true, meanNs: s.delay_mean_ns, stdNs: s.delay_std_ns }
                    }
                  />
                </div>
              </Tab>
            );
          })}
        </Tabs>
      )}
    </>
  );
}

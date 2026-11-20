import { type CSSProperties, useCallback, useEffect, useMemo, useState } from 'react';
import { Alert, Badge, Button, Col, OverlayTrigger, Row, Tab, Tabs, Tooltip } from 'react-bootstrap';
import * as api from '../api/client';
import type { EmulationView, HistoryLink, HistoryTotals, StatsView } from '../api/types';
import {
  errorMessage,
  formatBits,
  formatFrameCount,
  formatPacketRate,
  l1Bps,
  linkLabel,
  secondsToTime,
} from '../common/format';
import { readConfig, startRequestOf, useLocalConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import { startPolling } from '../common/polling';
import { usePortLabeler } from '../common/portLabel';
import { type History, NO_SAMPLES, sumHistories, useRunHistory } from '../common/runHistory';
import ErrorAlert from '../components/ErrorAlert';
import PageTitle from '../components/PageTitle';
import Loader from '../components/Loader';
import LinkCharts from './LinkCharts';
import LoadRunning from './LoadRunning';
import { totalDrops } from './LossDonut';
import { addStats, type LinkStats, NO_STATS, statsOfLive, statsOfTotals } from './linkStats';

const POLL_MS = 1000;

/** The key of the Summary tab; the keys of the link tabs are their indexes. */
const SUMMARY_TAB = 'summary';

const DASH = '–';

const NO_HISTORY: History = {};
const NO_LINKS: HistoryLink[] = [];
const NO_TOTALS: Record<string, HistoryTotals> = {};

// The pills' look (P4TG's SendReceiveMonitor) as inline styles.
const BOX: CSSProperties = {
  color: '#fff',
  borderRadius: '10px',
  display: 'inline-block',
  marginRight: '10px',
};
const STAT_STYLE: CSSProperties = {
  ...BOX,
  background: 'var(--color-secondary)',
  padding: '10px 15px',
  minWidth: '250px',
  textAlign: 'center',
  marginBottom: '10px',
};
const TIME_STYLE: CSSProperties = {
  ...BOX,
  background: 'var(--color-secondary)',
  padding: '10px 5px',
  minWidth: '140px',
  maxWidth: '140px',
  textAlign: 'center',
  marginBottom: '10px',
};
const ARROW_STYLE: CSSProperties = { paddingRight: '5px' };

const ZERO_PILLS = { rxBps: 0, rxPps: 0, txBps: 0, txPps: 0, rxL2: 0, txL2: 0 };

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

/** Total RX and TX rates over all links: L1 for the pills, L2 for the loss share. */
function totalsOf(directions: StatsView['directions']) {
  let rxBps = 0;
  let rxPps = 0;
  let txBps = 0;
  let txPps = 0;
  let rxL2 = 0;
  let txL2 = 0;
  for (const d of directions) {
    rxBps += l1Bps(d.rx_bps, d.rx_pps);
    rxPps += d.rx_pps;
    txBps += l1Bps(d.tx_bps, d.tx_pps);
    txPps += d.tx_pps;
    rxL2 += d.rx_bps;
    txL2 += d.tx_bps;
  }
  return { rxBps, rxPps, txBps, txPps, rxL2, txL2 };
}

export default function Dashboard() {
  const [emulation, setEmulation] = useState<EmulationView>();
  const [stats, setStats] = useState<StatsView>();
  const {
    history: polledHistory,
    links: polledLinks,
    totals: polledTotals,
    runId,
    error: historyError,
  } = useRunHistory();
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
  /** The run the controller held when Start/Restart was clicked: while it still holds that run, the page shows no run. */
  const [clearedRun, setClearedRun] = useState<{ id: number | null | undefined }>();

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
    if (which !== 'stop') setClearedRun({ id: runId });
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

  // From the click until the controller's history belongs to another run (a new run_id), the page shows no run, neither the old
  // series nor live statistics. The controller serves the old series until the new run exists, so they stay hidden while it starts
  // too. The charts themselves stay mounted.
  const cleared = clearedRun !== undefined && clearedRun.id === runId;
  const noSeries = cleared || state === 'starting';
  const history = noSeries ? NO_HISTORY : polledHistory;
  const links = noSeries ? NO_LINKS : polledLinks;
  const runTotalsByLink = noSeries ? NO_TOTALS : polledTotals;
  // The Start/Restart click until the run is up: the loader's overlay covers the page.
  const covered = busy === 'start' || busy === 'restart' || state === 'starting';
  const shownStats = cleared ? undefined : stats;

  const live = shownStats && shownStats.directions.length > 0 ? shownStats.directions : undefined;
  const plan = (dir: number) => emulation?.directions.find((p) => p.dir === dir);
  // The controller reports stale: true with no age_ms for a stopped emulation; that is not "old data".
  const showStale = state === 'running' && shownStats?.stale === true && shownStats.age_ms != null;
  // After the run ends (or while it ends) the controller keeps its series until the next start: the final charts stay.
  const ended = state === 'stopping' || state === 'stopped' || state === 'failed' ? state : undefined;
  const hasHistory = Object.values(history).some((list) => list.length > 0);
  const finished = ended !== undefined && hasHistory;

  // Like live statistics: once the run has ended the rates and the loss are 0 (the charts and donuts keep the run).
  const pills = finished ? ZERO_PILLS : live && totalsOf(live);
  const restartNeeded = pendingRestart(doc, emulation).any;
  // A dir is the link's index in the start request, so without reported directions or series the stored links give the
  // (empty) tabs.
  const noRun = !finished && !live && !hasHistory;
  const storedLinks = doc.links;
  const shown = useMemo(
    () =>
      live
        ? live.map((d) => d.dir)
        : hasHistory
          ? Object.keys(history)
              .map(Number)
              .filter(Number.isInteger)
              .sort((a, b) => a - b)
          : storedLinks.map((_, i) => i),
    [live, hasHistory, history, storedLinks],
  );
  const summaryHistory = useMemo(
    () => sumHistories(shown.map((dir) => history[String(dir)] ?? NO_SAMPLES)),
    [shown, history],
  );
  const statsOf = (dir: number): LinkStats => {
    if (finished) {
      const t = runTotalsByLink[String(dir)];
      return t ? statsOfTotals(t) : NO_STATS;
    }
    const s = live?.find((d) => d.dir === dir);
    return s ? statsOfLive(s) : NO_STATS;
  };
  // no zero start: a count that is null on every link stays null
  const perLink = shown.map(statsOf);
  const summaryStats = perLink.length > 0 ? perLink.reduce(addStats) : NO_STATS;
  const lost = totalDrops(summaryStats.drops);
  // A run with series is titled by the links of its own history, a live one by its stats or the plan, no run by the stored links.
  const titleOf = (dir: number) => {
    const stored = storedLinks[dir];
    if (finished || (!live && hasHistory)) {
      const l = links[dir];
      return l ? linkLabel(l.ingress, l.egress, label) : `Link ${dir}`;
    }
    if (!live && stored) return linkLabel(stored.ingress, stored.egress, label);
    const s = live?.find((d) => d.dir === dir);
    const p = plan(dir);
    if (!s && !p) return `Link ${dir}`;
    return linkLabel(s?.rx ?? p?.rx, s?.tx ?? p?.tx, label);
  };
  const activeTab = selected !== undefined && shown.includes(Number(selected)) ? selected : SUMMARY_TAB;
  const stopping = state === 'stopping';

  return (
    <>
      <PageTitle className="mb-3" title="Dashboard" />
      {emulation?.network === 'virtual' && (
        <div className="alert alert-info">
          Virtual network mode: for functional testing only, delays and rates are not measurements.
        </div>
      )}
      <Row className="align-items-center mb-3" data-testid="top-row">
        <Col className="col-8 text-start" data-testid="top-pills">
          <span style={STAT_STYLE} data-testid="pill-tx">
            <i
              className="bi bi-arrow-up-circle-fill"
              style={{ ...ARROW_STYLE, color: 'var(--color-okay)' }}
              aria-hidden="true"
            />
            {pills ? `${formatBits(pills.txBps)} (${formatPacketRate(pills.txPps)})` : DASH}
          </span>
          <span style={STAT_STYLE} data-testid="pill-rx">
            <i
              className="bi bi-arrow-down-circle-fill"
              style={{ ...ARROW_STYLE, color: 'var(--color-yellow)' }}
              aria-hidden="true"
            />
            {pills ? `${formatBits(pills.rxBps)} (${formatPacketRate(pills.rxPps)})` : DASH}
          </span>
          {state === 'running' && (
            <span
              style={{
                ...BOX,
                background: lost > 0 ? 'var(--color-primary)' : 'var(--color-okay)',
                padding: '10px 15px',
              }}
              data-testid="pill-status"
            >
              {lost > 0 ? (
                <OverlayTrigger placement="top" overlay={<Tooltip id="lost-frames">{lost}</Tooltip>}>
                  <span>Status: Error. Lost {formatFrameCount(lost)} frames.</span>
                </OverlayTrigger>
              ) : (
                'Status: Ok'
              )}
            </span>
          )}
          {shownStats?.elapsed_ms != null &&
            shownStats.elapsed_ms >= 1000 &&
            state !== 'stopped' &&
            state !== 'failed' && (
              <span style={TIME_STYLE} data-testid="pill-elapsed">
                {secondsToTime(Math.floor(shownStats.elapsed_ms / 1000))}
              </span>
            )}
          {pills && pills.rxL2 > 0 && 1 - pills.txL2 / pills.rxL2 > 0.001 && (
            <span data-testid="loss-pct">
              <i className="bi bi-exclamation-circle-fill text-danger" aria-hidden="true" />{' '}
              {(100 * (1 - pills.txL2 / pills.rxL2)).toFixed(2)} %
            </span>
          )}
        </Col>
        <Col xs={4} className="text-end" data-testid="run-buttons">
          {state === 'starting' || state === 'running' || stopping ? (
            <>
              <Button
                className="mb-1"
                variant="danger"
                disabled={stopping || busy !== undefined}
                onClick={() => act('stop')}
              >
                <i className="bi bi-stop-fill" aria-hidden="true" /> {stopping ? 'Draining…' : 'Stop'}
              </Button>{' '}
              <Button
                className="mb-1"
                variant="primary"
                disabled={stopping || corrupt || busy !== undefined}
                onClick={() => act('restart')}
              >
                <i className="bi bi-arrow-clockwise" aria-hidden="true" /> Restart
              </Button>
            </>
          ) : (
            <Button
              className="mb-1"
              variant="primary"
              disabled={state === undefined || corrupt || busy !== undefined}
              onClick={() => act('start')}
            >
              <i className="bi bi-play-circle-fill" aria-hidden="true" /> Start
            </Button>
          )}
        </Col>
      </Row>
      {/* Before the first answer the loader replaces the page; a failed first poll shows its error instead. */}
      <Loader loaded={emulation !== undefined || pollError !== undefined} overlay={covered}>
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
                  Restart needed: links changed
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
        <Tabs id="link-tabs" transition={false} activeKey={activeTab} onSelect={(k) => k && setSelected(k)}>
          <Tab eventKey={SUMMARY_TAB} title="Summary">
            <div className="pt-3">
              <LinkCharts
                active={activeTab === SUMMARY_TAB}
                history={summaryHistory}
                stats={summaryStats}
                empty={noRun}
              />
            </div>
          </Tab>
          {shown.map((dir) => {
            const s = live?.find((d) => d.dir === dir);
            return (
              <Tab key={dir} eventKey={String(dir)} title={titleOf(dir)}>
                <div className="pt-3">
                  <LinkCharts
                    dir={dir}
                    active={String(dir) === activeTab}
                    history={history[String(dir)] ?? NO_SAMPLES}
                    stats={statsOf(dir)}
                    empty={noRun}
                    // The link's stored config says whether its run had delay statistics (default: on).
                    endedStats={finished && storedLinks[dir]?.config.delay_stats !== false}
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
      </Loader>
    </>
  );
}

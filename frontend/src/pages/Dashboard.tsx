import { useEffect, useState } from 'react';
import { Alert, Badge, Button, Card } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { startPolling } from '../common/polling';
import { usePortLabeler } from '../common/portLabel';
import { readConfig, startRequestOf, useLocalConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import LoadRunning from './LoadRunning';
import DirectionCard from './DirectionCard';
import PortsTable from './PortsTable';
import type { Sample } from './DirectionCard';
import type { EmulationState, EmulationView, StatsView } from '../api/types';

const POLL_MS = 1000;
const HISTORY = 120;

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

function sampleOf(d: StatsView['directions'][number]): Sample {
  return {
    t: Date.now(),
    rx_bps: d.rx_bps,
    tx_bps: d.tx_bps,
    loss_pct: d.loss_pct,
    limiter_pct: d.limiter_pct,
    buffer_full_pct: d.buffer_full_pct,
    nic_tx_pct: d.nic_tx_pct,
    nic_rx_pct: d.nic_rx_pct,
  };
}

export default function Dashboard() {
  const [emulation, setEmulation] = useState<EmulationView>();
  const [stats, setStats] = useState<StatsView>();
  const [history, setHistory] = useState<Record<number, Sample[]>>({});
  const [pollError, setPollError] = useState<string>();
  const [statsError, setStatsError] = useState<string>();
  /** The stats error the user closed; the warning returns only for a different message. */
  const [dismissed, setDismissed] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [busy, setBusy] = useState<'start' | 'stop'>();
  const label = usePortLabeler();
  const { doc, corrupt } = useLocalConfig();

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
        setHistory((h) => {
          if (s.directions.length === 0) return {};
          const next: Record<number, Sample[]> = {};
          for (const d of s.directions) next[d.dir] = [...(h[d.dir] ?? []), sampleOf(d)].slice(-HISTORY);
          return next;
        });
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

  const act = async (which: 'start' | 'stop') => {
    if (busy) return;
    setBusy(which);
    setActionError(undefined);
    try {
      if (which === 'stop') {
        setEmulation(await api.stopEmulation());
      } else {
        // The configuration as it is stored now, so an edit made in another tab is included.
        const stored = readConfig();
        if (stored.corrupt) throw new Error('The local configuration cannot be read; reset it first.');
        if (stored.doc.links.length === 0) throw new Error('No links configured: nothing to start. Add a link first.');
        setEmulation(await api.startEmulation(startRequestOf(stored.doc)));
      }
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setBusy(undefined);
    }
  };

  const state = emulation?.state;
  const canStart = (state === 'stopped' || state === 'failed') && !corrupt;
  const canStop = state === 'starting' || state === 'running';
  const plan = (dir: number) => emulation?.directions.find((p) => p.dir === dir);
  // The controller reports stale: true with no age_ms for a stopped emulation; that is not "old data".
  const showStale = state === 'running' && stats?.stale === true && stats.age_ms != null;

  return (
    <>
      <h1 className="mb-3">Dashboard</h1>
      <ErrorAlert message={actionError ?? pollError} />
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
          <Button variant="success" disabled={!canStart || busy !== undefined} onClick={() => act('start')}>
            <i className="bi bi-play-fill" aria-hidden="true" /> Start
          </Button>
          <Button variant="primary" disabled={!canStop || busy !== undefined} onClick={() => act('stop')}>
            <i className="bi bi-stop-fill" aria-hidden="true" /> Stop
          </Button>
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
      {stats?.directions.map((d) => (
        <DirectionCard key={d.dir} stats={d} plan={plan(d.dir)} label={label} history={history[d.dir] ?? []} />
      ))}
      {stats && stats.ports.length > 0 && <PortsTable ports={stats.ports} label={label} />}
    </>
  );
}

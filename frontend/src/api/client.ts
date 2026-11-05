// REST client of the dpds-controller. Base URL `/api` (same origin: the controller serves the frontend,
// and the Vite dev proxy forwards it).
// Every function returns the parsed JSON body. Any non-2xx response rejects with `ApiError`; a network
// failure rejects with the plain fetch error. Every request carries `X-Session-Id`; every non-GET request carries `Content-Type: application/json`
// (the controller answers 415 otherwise), also the body-less stop.
import type {
  EmulationView,
  Hint,
  HistogramView,
  HistoryView,
  Interface,
  OnlineView,
  StartRequest,
  StatsView,
} from './types';

export class ApiError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

const SESSION_KEY = 'dpds-session-id';
let sessionId: string | undefined;

/** The id of this browser tab (spec 48), like P4TG's getSessionId: kept in sessionStorage, in memory if that throws. */
function getSessionId(): string {
  if (sessionId) return sessionId;
  try {
    sessionId = sessionStorage.getItem(SESSION_KEY) || undefined;
  } catch {
    // sessionStorage is not available: the id stays in memory
  }
  if (!sessionId) {
    sessionId =
      typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
        ? crypto.randomUUID()
        : Math.random().toString(36).slice(2) + Date.now().toString(36);
    try {
      sessionStorage.setItem(SESSION_KEY, sessionId);
    } catch {
      // see above
    }
  }
  return sessionId;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { 'X-Session-Id': getSessionId() };
  const init: RequestInit = { method, headers };
  if (method !== 'GET') {
    headers['Content-Type'] = 'application/json';
    if (body !== undefined) init.body = JSON.stringify(body);
  }
  const res = await fetch(`/api${path}`, init);
  if (!res.ok) {
    let message = `HTTP ${res.status}`;
    try {
      const err = (await res.json()) as { message?: unknown };
      if (typeof err.message === 'string') message = err.message;
    } catch {
      // not a JSON error body: keep the status text
    }
    throw new ApiError(res.status, message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

export const getOnline = () => request<OnlineView>('GET', '/online');
export const getInterfaces = () => request<Interface[]>('GET', '/interfaces');
export const getHostHints = () => request<Hint[]>('GET', '/host/hints');
/** Host-only hints (GET) plus the ones that depend on the links (POST with the configuration, nothing is started).
 * `recheck` makes the controller read the NIC firmware again instead of answering from its cache. */
export const getHostHintsFor = (req: StartRequest, opts?: { recheck?: boolean }) =>
  request<Hint[]>('POST', `/host/hints${opts?.recheck ? '?recheck=true' : ''}`, req);
export const getEmulation = () => request<EmulationView>('GET', '/emulation');
/** The whole configuration travels with the start: the controller stores nothing. */
export const startEmulation = (req: StartRequest) => request<EmulationView>('POST', '/emulation/start', req);
export const stopEmulation = () => request<EmulationView>('POST', '/emulation/stop');
export const getStats = () => request<StatsView>('GET', '/stats');
/** `windowS` 10 | 60 | 300 = only the last n seconds; absent = since start. */
export const getDelayHistogram = (dir: number, windowS?: number) =>
  request<HistogramView>('GET', `/stats/${dir}/delay_histogram${windowS ? `?window_s=${windowS}` : ''}`);
/** The departure-gap histogram of the run (delay histogram's shape, no `expected*`). */
export const getIatHistogram = (dir: number) => request<HistogramView>('GET', `/stats/${dir}/iat_histogram`);
/** The run's per-second series; `sinceMs` = only samples newer than that (0 is a real time), absent = everything. */
export const getStatsHistory = (sinceMs?: number) =>
  request<HistoryView>('GET', `/stats/history${sinceMs === undefined ? '' : `?since_ms=${sinceMs}`}`);

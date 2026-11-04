// REST client of the dpds-controller. Base URL `/api` (same origin: the controller serves the frontend,
// and the Vite dev proxy forwards it).
// Every function returns the parsed JSON body. Any non-2xx response rejects with `ApiError`; a network
// failure rejects with the plain fetch error. Every non-GET request carries `Content-Type: application/json`
// (the controller answers 415 otherwise), also the body-less stop.
import type {
  EmulationView, Hint, HistogramView, Interface, OnlineView, StartRequest, StatsView,
} from './types';

export class ApiError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method };
  if (method !== 'GET') {
    init.headers = { 'Content-Type': 'application/json' };
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
/** Host-only hints (GET) plus the ones that depend on the links (POST with the configuration, nothing is started). */
export const getHostHintsFor = (req: StartRequest) => request<Hint[]>('POST', '/host/hints', req);
export const getEmulation = () => request<EmulationView>('GET', '/emulation');
/** The whole configuration travels with the start: the controller stores nothing. */
export const startEmulation = (req: StartRequest) => request<EmulationView>('POST', '/emulation/start', req);
export const stopEmulation = () => request<EmulationView>('POST', '/emulation/stop');
export const getStats = () => request<StatsView>('GET', '/stats');
/** `windowS` 10 | 60 | 300 = only the last n seconds; absent = since start. */
export const getDelayHistogram = (dir: number, windowS?: number) => request<HistogramView>(
  'GET',
  `/stats/${dir}/delay_histogram${windowS ? `?window_s=${windowS}` : ''}`,
);

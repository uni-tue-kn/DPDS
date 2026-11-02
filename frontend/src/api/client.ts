// REST client of the dpds-controller. Base URL `/api` (same origin: nginx / the Vite dev proxy forward it).
// Every function returns the parsed JSON body. Any non-2xx response rejects with `ApiError`; a network
// failure rejects with the plain fetch error. Every non-GET request carries `Content-Type: application/json`
// (the controller answers 415 otherwise), also the body-less start/stop/delete.
import type {
  EmulationView, Hint, HistogramView, Interface, Link, LinkInput, OnlineView, Settings, StatsView,
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
export const getLinks = () => request<Link[]>('GET', '/links');
export const getLink = (id: number) => request<Link>('GET', `/links/${id}`);
export const createLink = (input: LinkInput) => request<Link>('POST', '/links', input);
export const updateLink = (id: number, input: LinkInput) => request<Link>('PUT', `/links/${id}`, input);
export const deleteLink = (id: number) => request<void>('DELETE', `/links/${id}`);
export const getSettings = () => request<Settings>('GET', '/settings');
export const updateSettings = (s: Settings) => request<Settings>('PUT', '/settings', s);
export const getEmulation = () => request<EmulationView>('GET', '/emulation');
export const startEmulation = () => request<EmulationView>('POST', '/emulation/start');
export const stopEmulation = () => request<EmulationView>('POST', '/emulation/stop');
export const getStats = () => request<StatsView>('GET', '/stats');
export const getDelayHistogram = (dir: number) => request<HistogramView>('GET', `/stats/${dir}/delay_histogram`);

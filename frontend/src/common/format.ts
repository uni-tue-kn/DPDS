// Display formats shared by the Dashboard and the Interfaces page. Null/undefined print "n/a".
import type { PortLabeler } from './portLabel';

const NA = 'n/a';

export function formatPps(v?: number | null): string {
  if (v == null) return NA;
  if (v >= 1e6) return `${(v / 1e6).toFixed(2)} Mpps`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(2)} kpps`;
  return `${v.toFixed(0)} pps`;
}

export function formatBps(v?: number | null): string {
  if (v == null) return NA;
  if (v >= 1e9) return `${(v / 1e9).toFixed(2)} Gb/s`;
  if (v >= 1e6) return `${(v / 1e6).toFixed(2)} Mb/s`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(2)} kb/s`;
  return `${v.toFixed(0)} b/s`;
}

/** Nanoseconds -> ns / µs / ms / s, each quantity picks its own unit. */
export function formatDuration(ns?: number | null): string {
  if (ns == null) return NA;
  const a = Math.abs(ns);
  if (a < 1e3) return `${ns.toFixed(2)} ns`;
  if (a < 1e6) return `${(ns / 1e3).toFixed(2)} µs`;
  if (a < 1e9) return `${(ns / 1e6).toFixed(2)} ms`;
  return `${(ns / 1e9).toFixed(2)} s`;
}

export function formatInt(v?: number | null): string {
  return v == null ? NA : String(v);
}

/** Link speed in Mb/s -> "100 Gb/s". */
export function formatSpeedMbps(mbps?: number | null): string {
  if (mbps == null) return NA;
  return mbps >= 1000 ? `${mbps / 1000} Gb/s` : `${mbps} Mb/s`;
}

export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Counter with thousands separators ("98,765,432,100"). */
export function formatCount(v?: number | null): string {
  return v == null ? NA : v.toLocaleString('en-US');
}

/** "name: ingress → egress": the one label the dashboard and the links list share. */
export function linkLabel(
  name?: string | null,
  ingress?: string | null,
  egress?: string | null,
  label: PortLabeler = (p) => p ?? undefined,
): string {
  return `${name || '?'}: ${label(ingress) || '?'} → ${label(egress) || '?'}`;
}

/**
 * The first of `units` (each 1000 x the one before) in which `v` shows as at least 1, with two decimals ("9.25 Gbit/s";
 * none for the base unit). The unit is chosen after rounding, so 999 996 bit/s is "1.00 Mbit/s", not "1000.00 kbit/s".
 */
function scaled(v: number, units: string[]): string {
  const show = (x: number, i: number) => x.toFixed(i === 0 ? 0 : 2);
  let i = 0;
  let x = v;
  while (i < units.length - 1 && Number(show(x, i)) >= 1000) {
    x /= 1000;
    i += 1;
  }
  return `${show(x, i)} ${units[i]}`;
}

/** The rate pills of the dashboard: "9.25 Gbit/s (7.31 Mpps)". */
export function formatRatePill(bps: number, pps: number): string {
  return `${scaled(bps, ['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s'])} (${scaled(pps, ['pps', 'kpps', 'Mpps'])})`;
}

/** Milliseconds (a run's relative time) as "1h 2m 5s". */
export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m ${s % 60}s`;
}

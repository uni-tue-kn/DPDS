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

// The four formatters below are ported exactly from P4TG's SendReceiveMonitor.tsx and Helper.ts.
const K = 1000;

function unitScaled(v: number, sizes: string[], decimals: number): [number, string] {
  const i = Math.floor(Math.log(v) / Math.log(K));
  return [Number.parseFloat((v / K ** i).toFixed(decimals)), sizes[i] ?? ''];
}

/** "9.25 Gbit/s"; 1000-based, trailing zeros dropped. */
export function formatBits(bits: number, decimals = 2): string {
  if (bits <= 0) return '0 Bit/s';
  const [x, unit] = unitScaled(bits, ['Bit/s', 'Kbit/s', 'Mbit/s', 'Gbit/s', 'Tbit/s'], decimals);
  return `${x} ${unit}`;
}

/** "7.31 Mpps". */
export function formatPacketRate(packets: number, decimals = 2): string {
  if (packets <= 0) return '0 Pps';
  const [x, unit] = unitScaled(packets, ['Pps', 'Kpps', 'Mpps', 'Gpps', 'Tpps'], decimals);
  return `${x} ${unit}`;
}

/** "7.77 K"; always the decimals, an empty unit below 1000 ("5.00 "). */
export function formatFrameCount(packets: number, decimals = 2): string {
  if (packets === 0) return '0';
  const [x, unit] = unitScaled(packets, ['', 'K', 'M', 'B', 'T'], decimals);
  return `${x.toFixed(decimals)} ${unit}`;
}

/** Seconds as "1h 2m 5s"; the hours do not wrap at a day. */
export function secondsToTime(seconds: number): string {
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m ${Math.floor((seconds % 3600) % 60)}s`;
}

/**
 * Preamble and inter-frame gap per frame, in bits: P4TG's L1 is L2 plus 20 B per frame. Whether DPDK's byte counters include the
 * 4 B FCS is unverified; it is not added here.
 */
export const L1_OVERHEAD_BITS = 20 * 8;

/** The L1 bit rate of an L2 bit rate `bps` at `pps` frames per second. */
export const l1Bps = (bps: number, pps: number) => bps + pps * L1_OVERHEAD_BITS;

/** The L1 rate in Gbit/s of a chart sample's L2 `gbps` at `mpps` million frames per second (P4TG's charts plot L1). */
export const l1Gbps = (gbps: number, mpps: number) => gbps + (mpps * 1e6 * L1_OVERHEAD_BITS) / 1e9;

// "Pending restart": the local configuration differs from the one the running emulation was started with.
import type { EmulationView, LocalConfig, StartLink } from '../api/types';

/** Canonical JSON: keys sorted, null and undefined members dropped (null and absent are the same optional value). */
function canon(v: unknown): string {
  const strip = (x: unknown): unknown => {
    if (Array.isArray(x)) return x.map(strip);
    if (x !== null && typeof x === 'object') {
      return Object.fromEntries(
        Object.entries(x as Record<string, unknown>)
          .filter(([, y]) => y !== null && y !== undefined)
          .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
          .map(([k, y]) => [k, strip(y)]),
      );
    }
    return x;
  };
  return JSON.stringify(strip(v));
}

const linkKey = (l: StartLink) =>
  canon({
    ingress: l.ingress,
    egress: l.egress,
    config: l.config,
  });

export interface Pending {
  /** Anything differs: a link changed, was added or removed. */
  any: boolean;
  /** Ingress ports of the local links that differ from the running one or are new. */
  links: Set<string>;
}

const NONE: Pending = { any: false, links: new Set() };

/**
 * Compares only while the emulation is starting or running (a stopping one goes away anyway). Without any local link
 * the page offers "Load running configuration" instead.
 */
export function pendingRestart(local: LocalConfig, emulation: EmulationView | undefined): Pending {
  const running = emulation?.config;
  if (!running || local.links.length === 0 || (emulation?.state !== 'starting' && emulation?.state !== 'running'))
    return NONE;
  const byIngress = new Map(running.links.map((l) => [l.ingress, linkKey(l)]));
  const links = new Set(local.links.filter((l) => byIngress.get(l.ingress) !== linkKey(l)).map((l) => l.ingress));
  const removed = running.links.some((r) => !local.links.some((l) => l.ingress === r.ingress));
  return { any: links.size > 0 || removed, links };
}

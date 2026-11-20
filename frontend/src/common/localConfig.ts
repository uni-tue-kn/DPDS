// The configuration lives only in this browser: one JSON document in localStorage (key `dpds.config.v1`),
// `{version: 1, links}`. The controller stores nothing; Start sends the document's links.
//
// Reading never writes. Writes always start from a fresh read of the store, so two pages never overwrite each other.
// A value that cannot be read is reported (`corrupt`) and left untouched until the user resets it.
import { useMemo, useSyncExternalStore } from 'react';
import { linkProblem } from '../pages/linkForm';
import type { Link, LocalConfig, StartRequest } from '../api/types';

export const STORAGE_KEY = 'dpds.config.v1';

export const emptyConfig = (): LocalConfig => ({ version: 1, links: [] });

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
/**
 * The members of a stored or imported link that are kept; any other member is dropped
 * and never written back.
 */
const cleanLink = (l: Link): Link => ({
  id: l.id,
  ingress: l.ingress,
  egress: l.egress,
  config: l.config,
  ...(l.avg_frame_size_byte === undefined ? {} : { avg_frame_size_byte: l.avg_frame_size_byte }),
});

/**
 * What is wrong with a list of links, or null when it is fine: the first link the form would not accept unchanged
 * (named by position), or the first repeated id.
 */
export function linksProblem(links: unknown[]): string | null {
  const ids = new Map<unknown, number>();
  for (const [i, l] of links.entries()) {
    const problem = linkProblem(l);
    if (problem !== null) return `Link ${i + 1}: ${problem}`;
    const { id } = l as Link;
    if (ids.has(id)) return `Duplicate link id ${id}: links ${ids.get(id)} and ${i + 1}`;
    ids.set(id, i + 1);
  }
  return null;
}

/** True for a version 1 document with valid links (unknown extra members, such as old settings, are fine). */
export function isLocalConfig(v: unknown): v is LocalConfig {
  return isObject(v) && v.version === 1 && Array.isArray(v.links) && linksProblem(v.links) === null;
}

export interface Stored {
  corrupt: boolean;
  doc: LocalConfig;
}

/** True for a version 1 document with a list of links: the shape of an export file (check the links with `linksProblem`). */
export function isLinksFile(v: unknown): v is { version: 1; links: unknown[] } {
  return isObject(v) && v.version === 1 && Array.isArray(v.links);
}

/** Parses a stored value: absent = the empty document, anything unreadable = corrupt (with the empty document to show). */
export function parseStored(raw: string | null): Stored {
  if (raw === null) return { corrupt: false, doc: emptyConfig() };
  try {
    const v: unknown = JSON.parse(raw);
    if (isLocalConfig(v))
      return {
        corrupt: false,
        doc: { version: 1, links: v.links.map(cleanLink) },
      };
  } catch {
    // not JSON: corrupt below
  }
  return { corrupt: true, doc: emptyConfig() };
}

const listeners = new Set<() => void>();
const notify = () => {
  for (const l of listeners) l();
};

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  window.addEventListener('storage', listener); // another tab changed it
  return () => {
    listeners.delete(listener);
    window.removeEventListener('storage', listener);
  };
}

export const readRaw = () => localStorage.getItem(STORAGE_KEY);
export const readConfig = () => parseStored(readRaw());

/** The stored document (or the empty one), and whether the stored value is unreadable. Re-renders on every change. */
export function useLocalConfig(): Stored {
  const raw = useSyncExternalStore(subscribe, readRaw);
  return useMemo(() => parseStored(raw), [raw]);
}

/** Replaces the stored document; only `version` and `links` are written. */
export function writeConfig({ version, links }: LocalConfig): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify({ version, links: links.map(cleanLink) }));
  notify();
}

/** Removes the stored value (the "Reset" of a corrupt one). */
export function resetConfig(): void {
  localStorage.removeItem(STORAGE_KEY);
  notify();
}

/** Applies `change` to a fresh read and writes the result. Refuses to touch an unreadable value. */
export function updateConfig(change: (doc: LocalConfig) => LocalConfig): void {
  const s = readConfig();
  if (s.corrupt) throw new Error('The local configuration cannot be read; reset it first.');
  writeConfig(change(s.doc));
}

/** Replaces all links with `links` (ids as given): ONE store write. */
export function replaceLinks(links: Link[]): void {
  updateConfig((doc) => ({ ...doc, links }));
}

/** The body of Start (and of the link-dependent hints): links without ids, nothing else. */
export function startRequestOf(doc: LocalConfig): StartRequest {
  return {
    links: doc.links.map((l) => ({
      ingress: l.ingress,
      egress: l.egress,
      config: l.config,
    })),
  };
}

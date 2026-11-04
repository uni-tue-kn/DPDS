// The configuration lives only in this browser: one JSON document in localStorage (key `dpds.config.v1`),
// `{version: 1, links, presets, settings}`. The controller stores nothing; Start sends the document's links and settings.
//
// Reading never writes. Writes always start from a fresh read of the store, so two pages never overwrite each other.
// A value that cannot be read is reported (`corrupt`) and left untouched until the user resets it.
import { useMemo, useSyncExternalStore } from 'react';
import type { Link, LinkInput, LocalConfig, Preset, PresetInput, Settings, StartRequest } from '../api/types';

export const STORAGE_KEY = 'dpds.config.v1';

export const DEFAULT_SETTINGS: Settings = { main_lcore: null, eal_extra_args: [], poll_interval_ms: 500 };

export const emptyConfig = (): LocalConfig => ({
  version: 1, links: [], presets: [], settings: { ...DEFAULT_SETTINGS, eal_extra_args: [] },
});

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isString = (v: unknown) => typeof v === 'string';
const isId = (v: unknown) => typeof v === 'number' && Number.isInteger(v) && v > 0;

function isSettings(v: unknown): boolean {
  return isObject(v)
    && Array.isArray(v.eal_extra_args) && v.eal_extra_args.every(isString)
    && typeof v.poll_interval_ms === 'number'
    && (v.main_lcore == null || typeof v.main_lcore === 'number');
}

const isLink = (v: unknown) => isObject(v) && isId(v.id) && isString(v.name) && isString(v.ingress) && isString(v.egress) && isObject(v.config);
const isPreset = (v: unknown) => isObject(v) && isId(v.id) && isString(v.name) && isObject(v.config);

/** True for a version 1 document with well-formed links, presets and settings (unknown extra members are fine). */
export function isLocalConfig(v: unknown): v is LocalConfig {
  return isObject(v) && v.version === 1
    && Array.isArray(v.links) && v.links.every(isLink)
    && Array.isArray(v.presets) && v.presets.every(isPreset)
    && isSettings(v.settings);
}

export interface Stored {
  corrupt: boolean;
  doc: LocalConfig;
}

/** Parses a stored value: absent = the empty document, anything unreadable = corrupt (with the empty document to show). */
export function parseStored(raw: string | null): Stored {
  if (raw === null) return { corrupt: false, doc: emptyConfig() };
  try {
    const v: unknown = JSON.parse(raw);
    if (isLocalConfig(v)) return { corrupt: false, doc: v };
  } catch {
    // not JSON: corrupt below
  }
  return { corrupt: true, doc: emptyConfig() };
}

const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());

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

/** Replaces the stored document. */
export function writeConfig(doc: LocalConfig): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(doc));
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

const nextId = (list: { id: number }[]) => list.reduce((m, x) => Math.max(m, x.id), 0) + 1;

function refuseDuplicate(kind: string, list: { id: number; name: string }[], id: number | undefined, name: string): void {
  if (list.some((x) => x.id !== id && x.name === name)) throw new Error(`A ${kind} named "${name}" already exists`);
}

/** Creates (`id` undefined, id = max + 1) or replaces a link. Throws when another link has the name. */
export function saveLink(input: LinkInput, id?: number): void {
  updateConfig((doc) => {
    refuseDuplicate('link', doc.links, id, input.name);
    const links: Link[] = id === undefined
      ? [...doc.links, { ...input, id: nextId(doc.links) }]
      : doc.links.map((l) => (l.id === id ? { ...input, id } : l));
    return { ...doc, links };
  });
}

export function deleteLink(id: number): void {
  updateConfig((doc) => ({ ...doc, links: doc.links.filter((l) => l.id !== id) }));
}

/** Creates (id = max + 1) or replaces a preset; returns it. Throws when another preset has the name. */
export function savePreset(input: PresetInput, id?: number): Preset {
  let saved!: Preset;
  updateConfig((doc) => {
    refuseDuplicate('preset', doc.presets, id, input.name);
    saved = { ...input, id: id ?? nextId(doc.presets) };
    return { ...doc, presets: id === undefined ? [...doc.presets, saved] : doc.presets.map((p) => (p.id === id ? saved : p)) };
  });
  return saved;
}

export function deletePreset(id: number): void {
  updateConfig((doc) => ({ ...doc, presets: doc.presets.filter((p) => p.id !== id) }));
}

/** The body of Start (and of the link-dependent hints): links without ids, the settings, nothing else. */
export function startRequestOf(doc: LocalConfig): StartRequest {
  return {
    links: doc.links.map((l) => ({ name: l.name, ingress: l.ingress, egress: l.egress, config: l.config, worker: l.worker ?? null })),
    settings: settingsOf(doc.settings),
  };
}

export const settingsOf = (s: Settings): Settings => ({
  main_lcore: s.main_lcore ?? null, eal_extra_args: s.eal_extra_args, poll_interval_ms: s.poll_interval_ms,
});

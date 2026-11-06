// The draft the Settings page edits: one row per link, the config as the form model. Pure functions; the page holds the state.
import { errorMessage } from '../common/format';
import { LINK_NAME, LINK_NAME_RULE, configToForm, defaultConfigForm, formToDto, portError } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Link } from '../api/types';

export interface DraftRow {
  /** React key; a stable identity while rows are renamed, inserted and deleted. */
  key: number;
  /** The stored id; null for a row that has not been saved yet. */
  id: number | null;
  name: string;
  ingress: string;
  egress: string;
  config: ConfigForm;
}

let lastKey = 0;
const newKey = () => ++lastKey;

export type Role = 'ingress' | 'egress';

export const rowsOf = (links: Link[]): DraftRow[] =>
  links.map((l) => ({
    key: newKey(),
    id: l.id,
    name: l.name,
    ingress: l.ingress,
    egress: l.egress,
    config: configToForm(l.config, l.avg_frame_size_byte),
  }));

/** A new row named `link<k>` for the smallest k >= 1 not used by `rows`, without ports, with the default config. */
export function addedRow(rows: DraftRow[]): DraftRow {
  const used = new Set(rows.map((r) => r.name));
  let k = 1;
  while (used.has(`link${k}`)) k += 1;
  return { key: newKey(), id: null, name: `link${k}`, ingress: '', egress: '', config: defaultConfigForm() };
}

/** The first free `<name>_copy`, `<name>_copy2`, ...; the base is shortened so the result has at most 32 characters. */
export function copyName(name: string, rows: DraftRow[]): string {
  const used = new Set(rows.map((r) => r.name));
  for (let n = 1; ; n += 1) {
    const suffix = n === 1 ? '_copy' : `_copy${n}`;
    const candidate = name.slice(0, 32 - suffix.length) + suffix;
    if (!used.has(candidate)) return candidate;
  }
}

/** The copy of `row`: a new row with the given name, no ports, the same config. */
export const copiedRow = (row: DraftRow, name: string): DraftRow => ({
  ...row,
  key: newKey(),
  id: null,
  name,
  ingress: '',
  egress: '',
});

/** Port -> name of the row that uses it in `role`, over all rows but `except`. */
export function portUsers(rows: DraftRow[], except: number, role: Role): Map<string, string> {
  const users = new Map<string, string>();
  for (const r of rows) if (r.key !== except && r[role] !== '') users.set(r[role], r.name);
  return users;
}

/** The first problem of one row, or null. `rows` are all rows (the name and the ports must be unique among them). */
function rowProblem(rows: DraftRow[], at: number): string | null {
  const row = rows[at];
  const name = row.name.trim();
  try {
    if (name === '') throw new Error('Name is required');
    if (!LINK_NAME.test(name)) throw new Error(LINK_NAME_RULE);
    if (rows.some((o, i) => i !== at && o.name.trim() === name))
      throw new Error(`A link named "${name}" already exists`);
    for (const role of ['ingress', 'egress'] as const) {
      const port = row[role];
      if (port === '') throw new Error(`Select an ${role} port`);
      const bad = portError(port);
      if (bad) throw new Error(`${role === 'ingress' ? 'Ingress' : 'Egress'}: ${bad}`);
      const earlier = rows.slice(0, at).find((o) => o[role] === port);
      if (earlier) throw new Error(`Port ${port} is already used as ${role} by "${earlier.name.trim()}"`);
    }
    formToDto(row.config);
    return null;
  } catch (e) {
    return `Link ${at + 1}${name === '' ? '' : ` ("${name}")`}: ${errorMessage(e)}`;
  }
}

/**
 * The links to store for `rows`, in table order, or the first problem (in table order) as `{ problem }`. A new row gets
 * the id max + 1 (the first id is 1).
 */
export function draftLinks(rows: DraftRow[]): { links: Link[] } | { problem: string } {
  for (let at = 0; at < rows.length; at += 1) {
    const problem = rowProblem(rows, at);
    if (problem !== null) return { problem };
  }
  let nextId = rows.reduce((m, r) => Math.max(m, r.id ?? 0), 0) + 1;
  return {
    links: rows.map((r) => ({
      id: r.id ?? nextId++,
      name: r.name.trim(),
      ingress: r.ingress,
      egress: r.egress,
      ...formToDto(r.config),
    })),
  };
}

// The draft the Settings page edits: one row per link, the config as the form model. Pure functions; the page holds the state.
import { errorMessage } from '../common/format';
import { configToForm, defaultConfigForm, formToDto, portError } from './linkForm';
import type { ConfigForm } from './linkForm';
import type { Link } from '../api/types';

export interface DraftRow {
  /** React key; a stable identity while rows are edited, inserted and deleted. */
  key: number;
  /** The stored id; null for a row that has not been saved yet. */
  id: number | null;
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
    ingress: l.ingress,
    egress: l.egress,
    config: configToForm(l.config, l.avg_frame_size_byte),
  }));

/** A new row without ports, with the default config. */
export const addedRow = (): DraftRow => ({
  key: newKey(),
  id: null,
  ingress: '',
  egress: '',
  config: defaultConfigForm(),
});

/** The copy of `row`: a new row with no ports, the same config. */
export const copiedRow = (row: DraftRow): DraftRow => ({ ...row, key: newKey(), id: null, ingress: '', egress: '' });

/** The ports that rows other than `except` use in `role`. */
export function portUsers(rows: DraftRow[], except: number, role: Role): Set<string> {
  const users = new Set<string>();
  for (const r of rows) if (r.key !== except && r[role] !== '') users.add(r[role]);
  return users;
}

/** The first problem of one row, or null. `rows` are all rows (a port is the RX port of at most one, the TX port of at most one). */
function rowProblem(rows: DraftRow[], at: number): string | null {
  const row = rows[at];
  try {
    for (const role of ['ingress', 'egress'] as const) {
      const [kind, select] = role === 'ingress' ? (['RX', 'an RX'] as const) : (['TX', 'a TX'] as const);
      const port = row[role];
      if (port === '') throw new Error(`Select ${select} port`);
      const bad = portError(port);
      if (bad) throw new Error(`${kind} port: ${bad}`);
      if (rows.slice(0, at).some((o) => o[role] === port))
        throw new Error(`${port} is the ${kind} port of more than one link`);
    }
    formToDto(row.config);
    return null;
  } catch (e) {
    return `Link ${at + 1}: ${errorMessage(e)}`;
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
      ingress: r.ingress,
      egress: r.egress,
      ...formToDto(r.config),
    })),
  };
}

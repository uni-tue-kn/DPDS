// Shared helpers of the page tests: the local document (`seedStore`, `storedDoc`, `startBodyOf`, `dropNulls`, `addStages` ...), the
// config form (`section`, `numberField`, `unitSelect`, `unitOf`, `pickUnit`, `typeInto`, `shownBase`, `typeRate`) and the required
// markers.
//
// The frontend contract the tests assume:
//  - The configuration lives only in the browser: ONE `localStorage` key `dpds.config.v1` holds { "version": 1, "links": Link[] }.
//    Reading never writes; ids are assigned in the browser (max + 1, the first is 1); a stored or imported `settings` member and a
//    link's `worker` member are ignored and never written back. The key `theme` is separate.
//  - A value that cannot be read (not JSON, wrong shape, version != 1, an invalid link) raises a `role="alert"` on every page with a
//    Reset button; the stored text stays untouched until Reset, and Start is disabled meanwhile.
//  - Start, validate and hints send exactly { links: [{name, ingress, egress, config}] }, built from the local document, without
//    ids. Without local links nothing is sent. Hints are the host-only ones plus, with local links, the link-dependent ones; merged
//    by hint id, the link-dependent answer wins.
//  - Pending restart compares the local links with `emulation.config` (the running start request) by name, only while the
//    emulation is starting or running: `data-testid="pending-restart"` on the Dashboard and the Links page, `link-pending-restart`
//    on a differing row. "Load running configuration" shows when the emulation reports a config and the local document has no links.
//  - Stages are optional: `ConfigDto.delayer`, `.limiter` and `.loss_model` may be null or absent (not added). The editor always
//    shows Buffer and Statistics; each stage starts as an "Add <stage>" button that becomes its subsection with a "Remove <stage>"
//    button. The sections keep the order Buffer, Delayer, Limiter, Dropper, Statistics.
//  - Units: the API and the stored values are in base units (ns, bit/s, bit, pps); the form shows a number plus a unit select named
//    "<field label> unit" right after the input. Conversions are decimal-exact. A stored value is displayed in the largest unit in
//    which it is >= 1 with at most 3 decimals; 0 and empty show the field's default unit.
//  - Required fields carry a `.required-marker` (aria-hidden, text "*") in the field's group plus `required` and `aria-required`;
//    one "* required" note on the form.
//  - Ports are shown by their netdev name; values, payloads and `<option value>` keep the device name.
//  - The limiter limit is derived by dpds-core from the buffer capacity (capacity x 1542 B x 8 bit); the form shows it read-only.
//
// Toolchain: react-router 8, react-bootstrap + bootstrap 5, chart.js via react-chartjs-2 (mocked in tests), plain CSS. The API
// module `src/api/client.ts` is mocked globally; fixtures are in `fixtures.ts`.

import { render, act, screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import App from '../App';
import type { ConfigDto, Link } from '../api/types';

export function renderApp(path = '/') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

/** Advance fake timers and flush promises (RTL's waitFor does not cooperate with vitest fake timers). */
export async function tick(ms = 0): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

export function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** The `<dd>` that follows the `<dt>` with exactly this text inside `scope`. */
export function field(scope: HTMLElement, label: string): HTMLElement {
  const dts = Array.from(scope.querySelectorAll('dt')).filter((d) => d.textContent?.trim() === label);
  if (dts.length !== 1) throw new Error(`expected exactly one <dt>${label}</dt>, found ${dts.length}`);
  const dd = dts[0].nextElementSibling;
  if (dd?.tagName !== 'DD') throw new Error(`<dt>${label}</dt> is not followed by a <dd>`);
  return dd as HTMLElement;
}

/** A valid initialization rate for flows that create a link (new forms start with this field empty). */
export const TEST_RATE = '10000000';

// ---- units, subsections ------------------------------------------------------------------------------------

export const TIME_UNITS = ['ns', '\u00b5s', 'ms', 's'];
export const RATE_UNITS = ['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s'];
export const PPS_UNITS = ['pps', 'kpps', 'Mpps'];
export const SIZE_UNITS = ['bit', 'kbit', 'Mbit', 'Gbit'];
export const PACKET_UNITS = ['packets', 'kpackets', 'Mpackets'];
export const MULTIPLIER: Record<string, number> = {
  ns: 1,
  '\u00b5s': 1e3,
  ms: 1e6,
  s: 1e9,
  'bit/s': 1,
  'kbit/s': 1e3,
  'Mbit/s': 1e6,
  'Gbit/s': 1e9,
  pps: 1,
  kpps: 1e3,
  Mpps: 1e6,
  bit: 1,
  kbit: 1e3,
  Mbit: 1e6,
  Gbit: 1e9,
  packets: 1,
  kpackets: 1e3,
  Mpackets: 1e6,
};
/** The "Delay accuracy" toggle of the Delayer section replaces the Reordering and Activate correlation checkboxes. */
export const ACCURACY_MODES = ['Basic', 'Reordering', 'Correlation'];
export const accuracyRadio = (mode: string): HTMLInputElement => screen.getByRole('radio', { name: mode });
export async function pickAccuracy(user: UserEvent, mode: string): Promise<void> {
  await user.click(accuracyRadio(mode));
}
/** The selected option of the "Delay accuracy" toggle. */
export function accuracyOf(): string | undefined {
  return ACCURACY_MODES.find((m) => accuracyRadio(m).checked);
}

/** The five subsections of the config form, in their order. */
export const SECTIONS = ['Buffer', 'Delayer', 'Limiter', 'Dropper', 'Statistics'];

/** One subsection fieldset of the (only) config form on the page. */
export function section(name: string): HTMLElement {
  return screen.getByRole('group', { name });
}

/** The input of a labelled field (any scope). */
export function numberField(scope: HTMLElement, label: string): HTMLInputElement {
  return within(scope).getByLabelText(label) as HTMLInputElement;
}

/** The unit `<select>` of a field: accessible name "<label> unit". */
export function unitSelect(scope: HTMLElement, label: string): HTMLSelectElement {
  return within(scope).getByLabelText(`${label} unit`) as HTMLSelectElement;
}

/** Text of the selected unit option of a field. */
export function unitOf(scope: HTMLElement, label: string): string {
  const sel = unitSelect(scope, label);
  return sel.options[sel.selectedIndex].textContent ?? '';
}

export async function pickUnit(user: UserEvent, scope: HTMLElement, label: string, unit: string): Promise<void> {
  const sel = unitSelect(scope, label);
  await user.selectOptions(sel, within(sel).getByRole('option', { name: unit }));
}

/** Optionally switches the unit first (the field is cleared afterwards, so nothing is converted), then replaces the text. */
export async function typeInto(
  user: UserEvent,
  scope: HTMLElement,
  label: string,
  text: string,
  unit?: string,
): Promise<void> {
  if (unit !== undefined) await pickUnit(user, scope, label, unit);
  const el = numberField(scope, label);
  await user.clear(el);
  if (text !== '') await user.type(el, text);
}

/** Shown value x multiplier of the selected unit, rounded to 3 decimals (to read defaults whose unit is free); null when empty. */
export function shownBase(scope: HTMLElement, label: string): number | null {
  const el = numberField(scope, label);
  if (el.value === '') return null;
  return Math.round(Number(el.value) * MULTIPLIER[unitOf(scope, label)] * 1000) / 1000;
}

/** Types `rate` (in `unit`, default pps) into the (empty or filled) "Initialization rate" field of the only config form. */
export async function typeRate(user: UserEvent, rate: string = TEST_RATE, unit: string = 'pps'): Promise<void> {
  await typeInto(user, document.body, 'Initialization rate', rate, unit);
}

// ---- required markers --------------------------------------------------------------------------------------

/** Required markers (class `required-marker`, aria-hidden, text "*") in the group of the control's own label. */
export function requiredMarkers(control: HTMLElement): HTMLElement[] {
  const labels = Array.from((control as HTMLInputElement).labels ?? []);
  const label = labels[0] ?? document.querySelector(`label[for="${control.id}"]`);
  if (!label?.parentElement) throw new Error('control has no label with a parent element');
  return Array.from(label.parentElement.querySelectorAll<HTMLElement>('.required-marker'));
}

/** Marker + `required` + aria-required, exactly one marker, aria-hidden "*". */
export function expectRequired(control: HTMLElement): void {
  const marks = requiredMarkers(control);
  expect(marks).toHaveLength(1);
  expect(marks[0]).toHaveAttribute('aria-hidden', 'true');
  expect(marks[0].textContent?.trim()).toBe('*');
  expect(control).toBeRequired();
  expect(control).toHaveAttribute('required');
  expect(control).toHaveAttribute('aria-required', 'true');
}

/** No marker, no `required`, no aria-required="true". */
export function expectOptional(control: HTMLElement): void {
  expect(requiredMarkers(control)).toHaveLength(0);
  expect(control).not.toHaveAttribute('required');
  expect(control.getAttribute('aria-required')).not.toBe('true');
}

/** Every `.required-marker` inside the form that holds the Save button. */
export function allRequiredMarkers(): HTMLElement[] {
  const form = screen.getByRole('button', { name: /^save$/i }).closest('form');
  if (!form) throw new Error('Save button is not inside a form');
  return Array.from(form.querySelectorAll<HTMLElement>('.required-marker'));
}

// ---- the local document, the start body, optional stages -----------------------------------------------------

/** THE storage key of the local document. */
export const STORAGE_KEY = 'dpds.config.v1';
/** The document shape (`{version, links}`). */
export interface StoredDoc {
  version: 1;
  links: Link[];
}

export function emptyDoc(): StoredDoc {
  return { version: 1, links: [] };
}

/** Writes a document (missing parts empty) to the store, as an earlier session of the app would have. */
export function seedStore(over: Partial<StoredDoc> = {}): StoredDoc {
  const doc: StoredDoc = { ...emptyDoc(), ...over };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(doc));
  return doc;
}
export const seedLinks = (links: Link[]) => seedStore({ links });
/** Writes an arbitrary string (corruption tests). */
export function seedRaw(value: string): void {
  localStorage.setItem(STORAGE_KEY, value);
}
export const rawStore = () => localStorage.getItem(STORAGE_KEY);

/** The stored document; fails when there is none. */
export function storedDoc(): StoredDoc {
  const raw = rawStore();
  if (raw === null) throw new Error(`nothing stored under ${STORAGE_KEY}`);
  return JSON.parse(raw) as StoredDoc;
}
/** The stored document, or the empty one while the key is absent. */
export const storedOrEmpty = (): StoredDoc => (rawStore() === null ? emptyDoc() : storedDoc());
export const storedLinks = () => storedDoc().links;

/** Recursively drops null / undefined object members ("null or absent" are the same optional value). */
export function dropNulls<T>(v: T): T {
  if (Array.isArray(v)) return v.map((x) => dropNulls(x)) as T;
  if (v !== null && typeof v === 'object') {
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>)
        .filter(([, x]) => x !== null && x !== undefined)
        .map(([k, x]) => [k, dropNulls(x)]),
    ) as T;
  }
  return v;
}
/** `a` and `b` are the same config modulo null/absent optional members. */
export const sameConfig = (a: ConfigDto, b: ConfigDto) => expect(dropNulls(a)).toEqual(dropNulls(b));

/** The stage of a config as null when it is absent. */
export const stageOf = <K extends 'delayer' | 'limiter' | 'loss_model'>(
  c: ConfigDto,
  k: K,
): NonNullable<ConfigDto[K]> | null => (c[k] ?? null) as NonNullable<ConfigDto[K]> | null;

const START_LINK_KEYS = ['name', 'ingress', 'egress', 'config'];

/** The start body the document must produce: links without ids and without worker, nothing else. */
export function startBodyOf(doc: StoredDoc): { links: unknown[] } {
  return {
    links: doc.links.map((l) => ({
      name: l.name,
      ingress: l.ingress,
      egress: l.egress,
      config: l.config,
    })),
  };
}

/** `actual` (the argument of startEmulation / getHostHintsFor) has exactly the contracted shape and the content of `doc`. */
export function expectStartBody(actual: unknown, doc: StoredDoc): void {
  const body = actual as { links: Record<string, unknown>[] };
  expect(Object.keys(body)).toEqual(['links']);
  expect(body.links).toHaveLength(doc.links.length);
  for (const l of body.links) {
    expect(Object.keys(l).filter((k) => !START_LINK_KEYS.includes(k))).toEqual([]);
    expect(l).not.toHaveProperty('id');
  }
  expect(dropNulls(JSON.parse(JSON.stringify(body)))).toEqual(dropNulls(JSON.parse(JSON.stringify(startBodyOf(doc)))));
}

export type Stage = 'delayer' | 'limiter' | 'dropper';
export const STAGES: Stage[] = ['delayer', 'limiter', 'dropper'];
export const STAGE_TITLE: Record<Stage, string> = { delayer: 'Delayer', limiter: 'Limiter', dropper: 'Dropper' };

export const addButton = (s: Stage) => screen.getByRole('button', { name: `Add ${s}` });
export const queryAddButton = (s: Stage) => screen.queryByRole('button', { name: `Add ${s}` });
export const removeButton = (s: Stage) => screen.getByRole('button', { name: `Remove ${s}` });
export const queryRemoveButton = (s: Stage) => screen.queryByRole('button', { name: `Remove ${s}` });
export const queryStage = (s: Stage) => screen.queryByRole('group', { name: STAGE_TITLE[s] });

/** Clicks "Add <stage>" for each of the stages (default: all three). */
export async function addStages(user: UserEvent, stages: Stage[] = STAGES): Promise<void> {
  for (const s of stages) await user.click(addButton(s));
}

/** Counts the writes of the local document from now on (spies on `localStorage.setItem`; call it after seeding). */
export function watchStoreWrites(): () => number {
  const spy = vi.spyOn(Storage.prototype, 'setItem');
  spy.mockClear();
  return () => spy.mock.calls.filter((c) => c[0] === STORAGE_KEY).length;
}

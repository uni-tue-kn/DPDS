// Shared helpers of the page tests: the local document (`seedStore`, `storedDoc`, `expectStartBody` ...), the Settings page (table
// rows, modals, toasts, stage switches) and the config form fields (`section`, `numberField`, `unitSelect`, `pickUnit`, `typeInto`)
// with the required markers.
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
//    emulation is starting or running: `data-testid="pending-restart"` on the Dashboard and the Settings page, `link-pending-restart`
//    on a differing row. "Load running configuration" shows when the emulation reports a config and the local document has no links.
//  - Stages are optional: `ConfigDto.delayer`, `.limiter` and `.loss_model` may be null or absent (not added).
//  - The Settings page (`/settings`, spec 63; the only place that edits links) is one P4TG-style page: a table with one editable row
//    per link of a DRAFT (the stored links when the page mounts), "Add link", Save and Reset, Import and Export, and toasts. Row `n`
//    (1-based) has `Name of link <n>`, `Ingress of link <n>` and `Egress of link <n>` controls and the buttons `Histogram of link
//    <n>`, `Settings of link <n>`, `Duplicate link <n>` and `Delete link <n>`. Nothing reaches the store before Save (ONE write).
//    The old routes `/settings/new` and `/settings/:id` are gone (redirect to `/`).
//  - "Settings of link <n>" opens the "Link settings" modal (title "Link #<n>: <name>"): sections (role group) Buffer, Delayer,
//    Limiter, Dropper in an always-open accordion. Delayer, Limiter and Dropper start with a switch "Enable <stage>"; off, the
//    stage's fields are not rendered. "Histogram of link <n>" opens the "Histogram" modal (title "Histograms of link #<n>: <name>"):
//    the switch "Delay statistics" and the groups "Delay histogram" and "IAT histogram". Both modals edit a copy: footer Close (also
//    the header X, Escape) discards it, footer Confirm validates (the fields of the modal only; a problem is a danger Alert inside
//    the modal, which stays open) and writes it into the draft row. The modal of the footer is told from the header X by the
//    footer: both are named "Close", so use `closeModal`.
//  - Feedback is a toast (a `status`, a danger one an `alert`) with the message: "Settings saved successfully.", "Settings reset
//    successfully.", `Cloned "<name>" to "<copy>"`, "Settings imported successfully.", a warning `Link <n> ("<name>"): <problem>` when
//    Save finds a problem, a danger one for a failed import, export or write (see `findToast`). A field label's info button is
//    "About <label>"; it opens an "Info" modal with a Close.
//  - A field label is a `<label>` whose own text is the plain label: the required marker and the info button are siblings, so
//    `getByLabelText('<label>')` finds the input.
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

import { render, act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import { vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import App from '../App';
import type { Link } from '../api/types';
import { dropNulls } from './dropNulls';
import { must } from './must';

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

// ---- units, subsections ------------------------------------------------------------------------------------

/** One section (fieldset) of the open modal, found by its (visually hidden) legend. */
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

// ---- the local document, the start body, optional stages -----------------------------------------------------

/** THE storage key of the local document. */
export const STORAGE_KEY = 'dpds.config.v1';
/** The document shape (`{version, links}`). */
export interface StoredDoc {
  version: 1;
  links: Link[];
}

/** Writes a document (missing parts empty) to the store, as an earlier session of the app would have. */
export function seedStore(over: Partial<StoredDoc> = {}): StoredDoc {
  const doc: StoredDoc = { version: 1, links: [], ...over };
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
export const storedLinks = () => storedDoc().links;

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
const STAGES: Stage[] = ['delayer', 'limiter', 'dropper'];

/** The "Enable <stage>" switch of a stage (a checkbox; found by its label whatever role the implementation gives it). */
export const stageSwitch = (s: Stage) => screen.getByLabelText(`Enable ${s}`) as HTMLInputElement;
export const stageEnabled = (s: Stage) => stageSwitch(s).checked;

/** Switches each of the stages on (default: all three); a stage that is on already stays on. */
export async function addStages(user: UserEvent, stages: Stage[] = STAGES): Promise<void> {
  for (const s of stages) if (!stageEnabled(s)) await user.click(stageSwitch(s));
}
/** Switches each of the stages off (default: all three); the typed values stay in the form. */
export async function removeStages(user: UserEvent, stages: Stage[] = STAGES): Promise<void> {
  for (const s of stages) if (stageEnabled(s)) await user.click(stageSwitch(s));
}

/** Counts the writes of the local document from now on (spies on `localStorage.setItem`; call it after seeding). */
export function watchStoreWrites(): () => number {
  const spy = vi.spyOn(Storage.prototype, 'setItem');
  spy.mockClear();
  return () => spy.mock.calls.filter((c) => c[0] === STORAGE_KEY).length;
}

// ---- the Settings page: table, modals, toasts -------------------------------------------------------------------

/** Renders `/settings` and waits for the page. */
export async function renderSettings(): Promise<UserEvent> {
  const user = userEvent.setup();
  renderApp('/settings');
  await screen.findByRole('heading', { level: 1, name: 'Settings' });
  return user;
}

/** The data rows of the links table (not the header, not the "No links configured yet." row). */
export function linkRows(): HTMLElement[] {
  const table = screen.getByRole('table');
  return Array.from(table.querySelectorAll<HTMLElement>('tbody tr')).filter((r) => r.querySelector('input'));
}
/** Row `n` (1-based). */
export const rowOf = (n: number): HTMLElement => must(linkRows()[n - 1], `row ${n}`);
/** The cell of row `n` under the column `name` (see `COLUMNS`). */
export function cellOf(n: number, name: string): HTMLElement {
  const heads = Array.from(screen.getByRole('table').querySelectorAll('thead th')).map((h) => h.textContent?.trim());
  const at = heads.indexOf(name);
  if (at < 0) throw new Error(`no column "${name}" in [${heads.join(', ')}]`);
  return must(rowOf(n).children[at] as HTMLElement | undefined, `cell ${name} of row ${n}`);
}
export const cellText = (n: number, name: string): string => cellOf(n, name).textContent?.trim() ?? '';

export const nameInput = (n = 1) => screen.getByLabelText(`Name of link ${n}`) as HTMLInputElement;
export const portSelect = (role: 'Ingress' | 'Egress', n = 1) =>
  screen.getByLabelText(`${role} of link ${n}`) as HTMLSelectElement;
export const importButton = () => screen.getByRole('button', { name: 'Import' });
export const exportButton = () => screen.getByRole('button', { name: 'Export' });
export const addLinkButton = () => screen.getByRole('button', { name: /^add link$/i });
export const saveButton = () => screen.getByRole('button', { name: /^save$/i });
export const resetButton = () => screen.getByRole('button', { name: /^reset$/i });
export const addLink = (user: UserEvent) => user.click(addLinkButton());
export const clickSave = (user: UserEvent) => user.click(saveButton());
export const clickReset = (user: UserEvent) => user.click(resetButton());

/** Sets the row's name (replacing it). */
export async function setName(user: UserEvent, name: string, n = 1): Promise<void> {
  await user.clear(nameInput(n));
  if (name !== '') await user.type(nameInput(n), name);
}

/** Waits until the interfaces are loaded (a usable port is an option of the ingress select of row `n`). */
export const portsLoaded = (n = 1) =>
  waitFor(() =>
    expect(
      within(portSelect('Ingress', n)).getByRole('option', { name: /^(enp101s0f0|0000:65:00\.0)/ }),
    ).toBeInTheDocument(),
  );

/** The dialog whose header title matches `title`, or null. */
function dialogTitled(title: RegExp): HTMLElement | null {
  return (
    screen
      .queryAllByRole('dialog')
      .find((d) => title.test(d.querySelector('.modal-title')?.textContent?.trim() ?? '')) ?? null
  );
}
export const linkDialog = () => dialogTitled(/^Link #\d+/);
export const histogramDialog = () => dialogTitled(/^Histograms of link #\d+/);
export const infoDialog = () => dialogTitled(/^Info$/);
/** The open Link settings or Histogram modal (at most one is open). */
export const openModal = () => linkDialog() ?? histogramDialog();
/** The danger Alert inside the open Link settings / Histogram modal. */
export const modalAlert = (): HTMLElement | null => openModal()?.querySelector<HTMLElement>('[role="alert"]') ?? null;

/** A footer button of a dialog (the header X is also named "Close"). */
export function footerButton(dialog: HTMLElement, name: string): HTMLElement {
  return within(must(dialog.querySelector<HTMLElement>('.modal-footer'), 'a modal footer')).getByRole('button', {
    name,
  });
}

/** Opens "Settings of link <n>" and returns the Link settings dialog. */
export async function openSettings(user: UserEvent, n = 1): Promise<HTMLElement> {
  await user.click(screen.getByRole('button', { name: `Settings of link ${n}` }));
  return waitFor(() => must(linkDialog(), 'the Link settings modal'));
}
/** Footer Confirm of the open Link settings / Histogram modal. */
export async function confirmModal(user: UserEvent): Promise<void> {
  await user.click(footerButton(must(openModal(), 'an open modal'), 'Confirm'));
}
/** Footer Close of the open Link settings / Histogram modal (the copy is discarded). */
export async function closeModal(user: UserEvent): Promise<void> {
  await user.click(footerButton(must(openModal(), 'an open modal'), 'Close'));
}
/**
 * Opens "Histogram of link <n>". An open Link settings modal is Confirmed first (its edits stay); it must accept them.
 * Returns the Histogram dialog.
 */
export async function openHistogram(user: UserEvent, n = 1): Promise<HTMLElement> {
  if (openModal()) {
    await confirmModal(user);
    if (openModal()) throw new Error(`the open modal refused Confirm: ${modalAlert()?.textContent ?? '?'}`);
  }
  await user.click(screen.getByRole('button', { name: `Histogram of link ${n}` }));
  return waitFor(() => must(histogramDialog(), 'the Histogram modal'));
}

/**
 * The elements that can be a toast: a `status` or `alert` outside the dialogs (the corrupt-configuration alert is none). Which
 * title, icon or role a toast has is not tested; only that a toast with this message appears.
 */
function toastElements(): HTMLElement[] {
  return [...screen.queryAllByRole('status'), ...screen.queryAllByRole('alert')].filter(
    (e) => e.closest('[role="dialog"]') === null && !/local configuration/i.test(e.textContent ?? ''),
  );
}
/** The toast on screen, or null. */
export function queryToast(): { el: HTMLElement; text: string } | null {
  const el = toastElements()[0];
  return el ? { el, text: el.textContent?.trim() ?? '' } : null;
}
/** Waits for a toast whose text contains `message` (a string) or matches it (a RegExp). */
export async function findToast(message: string | RegExp): Promise<HTMLElement> {
  return waitFor(() => {
    const t = must(queryToast(), 'a toast');
    if (typeof message === 'string') expect(t.text).toContain(message);
    else expect(t.text).toMatch(message);
    return t.el;
  });
}

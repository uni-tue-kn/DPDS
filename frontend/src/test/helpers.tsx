// Test helpers + THE FRONTEND CONTRACT the tests assume (spec v5 part D + v6 + v10 directed links + v12; the tester's
// choices where the spec is silent are listed here so the implementer can follow them or ask the coordinator to
// change them).
//
// ## v12: port labels everywhere, configurable initialization rate (supersedes the v10 label examples below)
// - Port label: `<pci> (<netdev>)`, e.g. "0000:65:00.0 (enp101s0f0)", where the netdev comes from `getInterfaces()` (the
//   interface whose `pci` equals the port string). A port that is not in the list (vdev `net_null0`, an undiscovered
//   address) or whose interface has `netdev: null` (vfio-bound NIC) shows only the device name. The label is used in
//   (1) the Links list row "<name>: <ingress label> → <egress label>", (2) the dashboard card title (same format, from
//   `stats.rx`/`stats.tx`, falling back to the plan's rx/tx), (3) the first column of the dashboard "NIC ports" table (the
//   port's `name`), (4) the link editor pickers (already, "<label> - <reason>" when unusable), (5) the Interfaces page (pci
//   and netdev columns, already). The Hints and Settings pages show no ports of their own: hint texts come from the
//   controller and are printed verbatim, so they do not call `getInterfaces`.
//   Fixture netdevs are "enp101s0f0" (0000:65:00.0) and "enp101s0f1" (0000:65:00.1); the spec's "ens1f0" is only an example.
// - `getInterfaces` is called exactly ONCE per page load by the Links page and ONCE per mount by the Dashboard (not per
//   stats/emulation poll). A rejected call is not an error for these pages: no alert, no warning, plain device names.
//   A slow or never-resolving call does not delay the cards or rows (they appear with plain names and are relabelled).
// - Links list loss text uses `formatPct(p * 100)` of common/format.ts: p 0.07 -> "7.00 % loss" (never "7.000000000000001").
// - `DelayerDto.initialization_rate_pps: number` (required, wire name as is; src/api/types.ts) and the config form field
//   "Initialization rate (pps)" in the delay section of EVERY config form (link editor, preset editor), a number input,
//   shown also while "Activate correlation" is off. A NEW link / preset form starts with the field EMPTY (the user wants
//   the rate always configured, never a preset value): saving the untouched form -> role=alert naming the field, no
//   request; flows that create something type a rate first (`typeRate` below). Name and port errors are reported before
//   the rate error (pure name/port rejection tests do not type a rate and rely on this order). Edit / apply-preset keep stored values. Validation (like the other fields, message "<label> must ..." naming
//   "Initialization rate (pps)"): empty, non-finite, < 1 -> role=alert, no request; 1 and fractions >= 1 are accepted.
//   It is sent as a number in `config.delayer`, populated from the stored link/preset on edit, copied by "Apply preset", and
//   included in "Save as preset". The Links list summary only shows the mean delay (no other delay parameter), so it does
//   not show the initialization rate either (not pinned).
//
// ## v10: directed links (supersedes every "two ports / two configs / Direction d" statement below)
// A link is ONE direction: `interface Link { id: number; name: string; ingress: string; egress: string;
//   config: ConfigDto; worker?: number | null }` and `LinkInput = Omit<Link, 'id'>` (src/api/types.ts; `ports`, `configs`
//   and `workers` are gone). `ingress == egress` (hairpin) is a valid link. `ExportDocument.version` is the literal 2
//   and its `links` are `LinkInput[]` in the new shape. `DirectionPlan`/`DirectionStats` keep `{dir, link, rx, tx, ...}`:
//   dir = link index, link = link name, rx = ingress port, tx = egress port; the API calls still use `dir`.
// - Links list: the row of a link holds ONE element whose own text is exactly "<name>: <ingress> -> <egress>" (arrow
//   U+2192, e.g. "a-b: 0000:65:00.0 → 0000:65:00.1"; a hairpin reads "loop: net_null0 → net_null0"). The row also
//   shows the worker ("auto" without one, else the number). No "Number of ports" / "Directions" notion remains.
// - Link editor labels: "Name", select "Ingress", select "Egress" (same option rules as the old port pickers: text =
//   pci + (netdev) + " - " + reason when unusable, unusable/kernel_route options disabled, a stored non-interface port
//   such as a vdev stays selectable, no preselection, Save does nothing until both are chosen), typed inputs "Ingress
//   (typed)" and "Egress (typed)" (same validation and override rule as before; an invalid value -> role=alert whose
//   text mentions "ingress" resp. "egress", no request), and the optional `<input type="number">` "Worker lcore" (empty ->
//   null/absent; a number input cannot receive non-numeric text; a value that is not a non-negative integer, e.g. "-1"
//   or "1.5" -> role=alert mentioning "worker", no request).
//   The editor loads `getLinks()`. In the Ingress picker every interface that another stored link (not the one being
//   edited) uses as INGRESS is `disabled`, its option text containing "already used as ingress by" + that link's name;
//   likewise the Egress picker and "already used as egress by". A port used as egress elsewhere stays enabled in the
//   Ingress picker (ring A->B, B->C) and vice versa; ingress == egress is allowed. Interfaces that are unusable keep
//   their own reason. Typed values are not checked against other links (the controller answers 409, shown as an alert).
//   A failing `getLinks` must not break the editor (no port is then disabled).
//   Exactly ONE config form: `<fieldset>` (role group) whose legend starts with "Link configuration", with the same
//   field labels as before, "Apply preset" and "Save as preset" inside it; validation messages name the field label
//   (e.g. "Buffer capacity"), no "Direction d" prefix. Save -> `createLink({name, ingress, egress, config, worker})` /
//   `updateLink(id, ...)` (no `ports`/`configs`/`workers` keys), then /links.
// - Dashboard: a card `data-testid="direction-card-<dir>"` per entry of `stats.directions` whose title text is
//   "<link>: <rx> -> <tx>" (e.g. "a-b: 0000:65:00.0 → 0000:65:00.1"); the text "Direction" does not appear in the title.
//   Everything else (stats, charts, `getDelayHistogram(dir)` by the direction index) is unchanged.
// - Export/import: the export document has `version: 2`; the UI passes documents through untouched (also an old
//   `version: 1` one: the controller rejects it with 400 and the UI shows that message).

// Toolchain: react-router-dom 6 (v7 needs Node >= 20), react-bootstrap + bootstrap 5, chart.js via
// react-chartjs-2 (mocked in tests: histogram = `Bar`, rate/drop charts = `Line`), plain CSS (no sass).
// The API module `src/api/client.ts` (see its header) is mocked globally; fixtures in `fixtures.ts`.
//
// ## Entry points
// - `src/App.tsx`: default export `App`, renders sidebar + routed page; does NOT contain a router
//   (`src/main.tsx` wraps it in `BrowserRouter`; tests wrap it in `MemoryRouter`).
// - Routes: `/` Dashboard, `/links`, `/links/new`, `/links/:id` (editor), `/interfaces`, `/hints`, `/settings`.
// - `src/common/polling.ts`: `startPolling(task, intervalMs, runImmediately = false): () => void` (P4TG's
//   helper): runs `task` serially (next run is scheduled `intervalMs` after the previous one settled), errors
//   do not stop it, the returned function stops it for good (also while a run is in flight).
// - `src/assets/index.css`: CSS variables `--color-primary: #e74c3c`, `--color-secondary: #2c3e50`,
//   `--color-okay: #27ae60`, `--color-yellow: #ffc107` in `:root`, and a `[data-theme="dark"]` block
//   overriding `--color-background`/`--color-secondary`/`--color-sidebar-background`.
//
// ## Layout
// - `<nav aria-label="Main navigation">` holds links named (in this order, further links such as API docs
//   may follow): Dashboard (/), Links (/links), Interfaces (/interfaces), Host hints (/hints),
//   Settings (/settings). The link of the current route has `aria-current="page"`.
// - Each page has an `<h1>` with its name: Dashboard, Links, Interfaces, Host hints, Settings; editor:
//   "New link" / "Edit link".
// - Theme: button named "Toggle dark mode" with `aria-pressed` (true = dark). Theme is stored in
//   `localStorage["theme"]` as "dark" | "light" and mirrored to `document.documentElement` attribute
//   `data-theme` ("dark" | "light"). Without stored value: `matchMedia('(prefers-color-scheme: dark)')`.
//
// ## Formats (used by the Dashboard cards and tables)
// - pps: >= 1e6 "1.23 Mpps", >= 1e3 "1.23 kpps", else "123 pps" (2 decimals above, 0 below).
// - bit rates: >= 1e9 "9.50 Gb/s", >= 1e6 "x.xx Mb/s", >= 1e3 "x.xx kb/s", else "x b/s".
// - durations (ns input): < 1e3 "x.xx ns", < 1e6 "x.xx us" (micro sign U+00B5 or U+03BC), < 1e9 "x.xx ms",
//   else "x.xx s". Every quantity picks its own unit.
// - percentages: "0.50 %" (2 decimals; a space before % is optional). Null/absent values print "n/a".
//
// ## Dashboard (see dashboard.test.tsx)
// - `data-testid="state-badge"`: text = the state name (case-insensitive), attribute `data-state`.
// - Buttons "Start" and "Stop". Start enabled in stopped|failed; Stop enabled in starting|running.
//   Both disabled otherwise and while their own request is in flight. API error -> `role="alert"`.
// - failed: `emulation.error` visible + each `log_tail` line visible inside `data-testid="log-tail"`.
// - A card `data-testid="direction-card-<dir>"` per entry of `stats.directions` (one per link, v10 title above), with
//   `<dt>`/`<dd>` pairs labelled: "RX rate" (pps and Gb/s), "TX rate", "Loss drops",
//   "Limiter drops", "Buffer full drops", "NIC TX drops", "NIC RX drops" (the five percentages),
//   "Mean delay" ("<mean> ± <std>"), "Lateness" (mean and max), "Buffer max" (buffer_len_max), and (v7 F2)
//   "Delay min / max" ("800.00 µs / 1.30 ms", n/a when null) and "First loss" (reason, detail and
//   `formatDuration(first_loss_relative_ns)`; "n/a" without a first loss).
// - v7 F2: `data-testid="ports-table"` (rendered iff `stats.ports` is non-empty, headed by a heading "NIC ports"):
//   `<table>` with column headers "Port", "ipackets", "opackets", "ibytes", "obytes", "imissed", "ierrors", "oerrors",
//   "rx_nombuf" (in this order, first column = port name, or the port number without a name) and one row per port;
//   counters are plain integers (thousands separators allowed). Rates are NOT covered (the type has no rate fields).
// - v7 F3: a failed stats poll shows `data-testid="stats-warning"` (message inside, a button named /close|dismiss/i
//   removes it; it also goes away after a later successful poll). A failed histogram fetch shows
//   `data-testid="histogram-error"` (reason inside) in that direction's card instead of the chart; both recover.
//   The sidebar re-polls `getOnline` at least every 5 s (serially) so "Offline" turns into "v<version>".
// - v7 F4: rate/drop/histogram charts are wrapped in `role="img"` elements with a non-empty `aria-label` (rate label
//   differs per card); the emulation error box is `role="alert"`; `state-badge` sits in `aria-live="polite"` (or
//   role=status); (v10: cards read "<link>: <rx> → <tx>", see above).
// - `data-testid="stale-indicator"` with text /stale/i iff the emulation state is `running` AND `stats.stale` AND
//   `stats.age_ms != null` (v7 F1; the stopped payload `{directions:[],ports:[],stale:true}` shows nothing).
// - Histogram per link card: `<Bar>` (`data-testid="chart-bar"`, data.labels.length == bins.length,
//   data.datasets[0].data == bins) fetched with `getDelayHistogram(dir)` on every poll, plus text
//   "Underflow: n" / "Overflow: n". Not rendered, and not fetched, for `delay_stats: false` directions;
//   a rejected histogram request just leaves the chart out.
// - At least one `<Line>` (`data-testid="chart-line"`) rate/drop chart once there are directions.
// - Polling: `getEmulation`, `getStats`, `getDelayHistogram(dir)` each repeat every <= 2 s, never with two
//   requests of the same function in flight, and stop after unmount.
//
// ## Links (links.test.tsx)
// - Table rows: name + ports; per row a link "Edit <name>" (-> /links/<id>) and a button "Delete <name>";
//   link "New link" (-> /links/new); empty list -> text /no links/i. `data-testid="pending-restart"`
//   (text /restart/i) iff `getEmulation().pending_restart`.
// - Delete: modal `role="dialog"` with buttons "Confirm delete" and "Cancel"; only Confirm calls
//   `deleteLink(id)` and reloads the list; API errors -> `role="alert"`.
// - Config form fields (the v10 block above says where the form lives; seeds are kept as strings: a seed above 2^53-1 ->
//   role="alert" mentioning "seed", no request): "Delay distribution" (values = enum names), "Mean delay (ns)",
//   "Delay std (ns)", "Activate correlation", "Half-life period (ns)", "Initialization rate (pps)" (v12), "Limiter
//   active", "Limiter mode", "Rate (bit/s)", "Burst limit (bit)",
//   "Loss model" (values Random | GilbertElliot), Random: "Random loss probability"; Gilbert-Elliot:
//   "Gilbert-Elliot p", "... r", "... k", "... h", "Gilbert-Elliot starts in good state" (only the fields of
//   the selected model are rendered), "Buffer capacity", "Reordering", "Seed" (empty -> null),
//   "Histogram bins", "Histogram lo (ns)", "Histogram hi (ns)" (all empty -> null/absent), "Delay statistics".
// - Buttons "Save" and link "Cancel"; save -> `createLink(input)` / `updateLink(id, input)` then /links.
//   API error -> `role="alert"` with the message, stays on the page, form values kept.
//
// ## v8 (spec8-polish U1-U4): presets, setup export/import, histogram extras
// ### API types (add to src/api/types.ts; names are the tester's choice, the JSON names are K1-K3 of spec8)
//   interface Preset { id: number; name: string; description?: string | null; config: ConfigDto }
//   type PresetInput = Omit<Preset, 'id'>;
//   interface ExportDocument { format: 'dpds-controller-export'; version: 2; settings: Settings;
//     links: LinkInput[]; presets: PresetInput[] }          // no ids anywhere
//   interface ImportSummary { links: number; presets: number }
//   HistogramView gains (all optional so an old controller still works; the UI must tolerate their absence):
//     window_s?: number;  span_ms?: number;  count?: number;           // count incl. under/overflow
//     percentiles?: { p50, p90, p99, p999: number | null;             // ns, null = no samples
//                     p50_clamped?, p90_clamped?, p99_clamped?, p999_clamped?: 'underflow' | 'overflow' | null };
//       (spec K3 says only "a flag p99_clamped style field per percentile": the tester fixed it as
//        `<name>_clamped` INSIDE `percentiles`, absent/null = not clamped; the Rust side must emit the same.)
//     expected?: number[];  expected_underflow?: number;  expected_overflow?: number;
//     expected_model?: 'exact' | 'normal_approximation';  expected_note?: string;   // note only when limiter active
// ### Client (src/api/client.ts; every non-GET carries Content-Type: application/json, see its header)
//   getPresets(): Preset[]                         GET    /api/presets
//   getPreset(id): Preset                          GET    /api/presets/{id}
//   createPreset(input: PresetInput): Preset       POST   /api/presets
//   updatePreset(id, input: PresetInput): Preset   PUT    /api/presets/{id}
//   deletePreset(id): void                         DELETE /api/presets/{id}   (204)
//   exportSetup(): ExportDocument                  GET    /api/export
//   importSetup(doc: ExportDocument): ImportSummary  POST /api/import?mode=replace   (body = the document untouched)
//   getDelayHistogram(dir, windowS?): without windowS -> /api/stats/{dir}/delay_histogram (no query string);
//     with 10 | 60 | 300 -> ...?window_s=<n>. The UI calls getDelayHistogram(dir) (ONE argument) for "Since start"
//     and getDelayHistogram(dir, n) for a window, so the pre-v8 `toHaveBeenCalledWith(dir)` tests keep passing.
// ### Presets page (presets.test.tsx)
//   Sidebar: link "Presets" (/presets) inside the main navigation, placed AFTER the five existing links (the
//   layout test pins the first five); heading h1 "Presets"; aria-current on /presets.
//   List: table, one row per preset with name and description; empty -> text /no presets/i; link "New preset"
//   (-> /presets/new); per row link "Edit <name>" (-> /presets/<id>) and button "Delete <name>" -> modal
//   role=dialog with "Confirm delete" / "Cancel"; only Confirm calls deletePreset(id) and reloads. API errors -> role=alert.
//   Editor (routes /presets/new "New preset", /presets/:id "Edit preset"): text inputs "Name" and "Description",
//   then exactly ONE config form (same labels as the link editor: "Mean delay (ns)", ...), buttons
//   "Save" / link "Cancel". Save -> createPreset / updatePreset(id) with {name, description, config} (config built by
//   the same validation as the link editor; an empty description is sent as null/absent) then navigate to /presets.
//   Empty name or an invalid config -> role=alert, no request. API error -> role=alert with the message, stays, values kept.
// ### Link editor additions (presetsInEditor.test.tsx)
//   Inside the link's config fieldset: a select labelled "Apply preset" (first option = placeholder, then one option per
//   preset whose text is EXACTLY the preset name; none besides the placeholder when there are no presets). Choosing
//   a preset copies ALL its fields into the form (not name, ingress, egress, worker). A failing getPresets must not break the editor.
//   Inside the fieldset: a button "Save as preset" -> modal role=dialog with inputs "Preset name" and
//   "Description" and buttons "Save preset" / "Cancel"; Save -> createPreset({name, description, config}) with the
//   CURRENT form values (validated like Save; invalid -> alert, no request); success closes the
//   dialog, role=status /saved/i, and the new preset shows up in the "Apply preset" selects; API error (409...) ->
//   role=alert INSIDE the dialog, dialog stays open. Empty name -> no request.
// ### Settings additions (settingsSetup.test.tsx)
//   Button "Export setup": exportSetup() -> Blob (JSON) -> URL.createObjectURL -> an <a download> whose `.click()`
//   is called (the test spies on HTMLAnchorElement.prototype.click; dispatching a click event instead would not be
//   seen) -> URL.revokeObjectURL. Filename `dpds-setup-YYYY-MM-DD.json`. Error -> role=alert, no download.
//   File input labelled "Import setup file" (accept .json). The file is read with FileReader (jsdom has no Blob.text()).
//   Valid JSON -> modal role=dialog warning (text mentions "replace", "links", "settings" and "presets") with buttons
//   "Confirm import" / "Cancel"; Confirm -> importSetup(parsedDocument) once; Cancel -> nothing. Success -> role=status
//   with "<n> links" and "<m> presets" (counts from the reply), settings form reloaded. 400/409 -> the API message in a
//   role=alert. Not JSON -> role=alert mentioning JSON, no dialog, no request. The input is enabled only while the
//   emulation is `stopped` (getEmulation); otherwise it is disabled and `data-testid="import-hint"` (text /stop/i) shows.
// ### Dashboard histogram (histogramExtras.test.tsx), inside each link card with a histogram
//   select "Histogram window" (options "Since start" | "10 s" | "60 s" | "300 s", values "0"|"10"|"60"|"300"); a change
//   refetches immediately and every later poll of that direction keeps the window; other directions are unaffected.
//   `data-testid="percentiles"` holding <dt>/<dd> pairs "p50", "p90", "p99", "p99.9" (dd = formatDuration, "n/a" for null);
//   a clamped dd carries `data-clamped="underflow"|"overflow"` and shows "≤" (underflow) / "≥" (overflow).
//   Checkbox "Log scale" (off by default): chart options scales.y.type is 'logarithmic' when on, 'linear'/absent when off.
//   Checkbox "Show expected distribution" (on by default, only meaningful when `expected` is present): datasets[1] of the
//   Bar data = {type: 'line', data: expected, label: 'Expected (<exact|normal approximation>)' + ' - <note>' if any};
//   the legend (options.plugins.legend.display) is shown while the overlay is. Unchecked / no `expected` -> one dataset only.
//   setup.ts now also exposes the chart `options` as `data-options` JSON next to `data-chart`.
//   components/charts.ts must register LineController (mixed bar+line chart).
// ## Settings (v7 F5): EAL extra arguments are a list, see the header of settings.test.tsx.
// ## Interfaces / Host hints / Settings: see their test files.
// ## Static packaging checks (Dockerfile, nginx.conf, docker-compose.yml, F6 leftovers): src/static.test.ts.
import { render, act, screen } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import App from '../App';

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
  if (!dd || dd.tagName !== 'DD') throw new Error(`<dt>${label}</dt> is not followed by a <dd>`);
  return dd as HTMLElement;
}

/** A valid initialization rate for flows that create a link or preset (v12: new forms start with this field EMPTY). */
export const TEST_RATE = '10000000';

/** Types `rate` into the (empty or filled) "Initialization rate (pps)" field of the only config form on the page. */
export async function typeRate(user: UserEvent, rate: string = TEST_RATE): Promise<void> {
  const el = screen.getByLabelText('Initialization rate (pps)');
  await user.clear(el);
  await user.type(el, rate);
}

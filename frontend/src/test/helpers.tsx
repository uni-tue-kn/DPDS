// Test helpers + THE FRONTEND CONTRACT the tests assume (spec v5 part D + v6; the tester's choices where the
// spec is silent are listed here so the implementer can follow them or ask the coordinator to change them).
//
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
// - Per direction (from `stats.directions`) a card `data-testid="direction-card-<dir>"` with the link name and
//   rx/tx device names, and `<dt>`/`<dd>` pairs labelled: "RX rate" (pps and Gb/s), "TX rate", "Loss drops",
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
//   differs per direction); the emulation error box is `role="alert"`; `state-badge` sits in `aria-live="polite"` (or
//   role=status); direction cards read "Direction d: <rx> → <tx>" (d 0-based, no link name between).
// - `data-testid="stale-indicator"` with text /stale/i iff the emulation state is `running` AND `stats.stale` AND
//   `stats.age_ms != null` (v7 F1; the stopped payload `{directions:[],ports:[],stale:true}` shows nothing).
// - Histogram per direction card: `<Bar>` (`data-testid="chart-bar"`, data.labels.length == bins.length,
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
// - Editor labels: "Name", "Number of ports" (select, values "1" | "2", default "2"), "Port A", "Port B"
//   (selects; options from `getInterfaces`, option text = pci + (netdev) + " - " + reason when unusable;
//   unusable or kernel_route interfaces are `disabled`; a stored port that is not an interface, e.g. a vdev,
//   stays selectable as its own option; option value = the pci address / device name; a new link starts with
//   no port selected and Save does nothing until both are chosen), "Worker lcores" (comma-separated, the edit form
//   shows them as "7, 2"; empty -> null). v7 F5: next to each picker a text input "Port A (typed)" / "Port B (typed)"
//   (none for Port B with 1 port); a non-empty typed value wins over the picker and is validated like the controller
//   (PCI `dddd:bb:dd.f` with function 0-7, or `net_(null|ring)<digits>`; nothing else, e.g. no net_pcap and no
//   `,key=value`): invalid -> role="alert" mentioning "port", no request. Seeds are kept as strings: a seed above
//   2^53-1 -> role="alert" mentioning "seed", no request. Per direction a `<fieldset>` (role group) whose legend (v7 F4)
//   reads "Direction d: <rx> → <tx>" (d 0-based; rx/tx from the chosen ports, loopback p → p) containing
//   (validation messages are prefixed with the same "Direction d: "):
//   "Delay distribution" (values = enum names), "Mean delay (ns)", "Delay std (ns)", "Activate correlation",
//   "Half-life period (ns)", "Limiter active", "Limiter mode", "Rate (bit/s)", "Burst limit (bit)",
//   "Loss model" (values Random | GilbertElliot), Random: "Random loss probability"; Gilbert-Elliot:
//   "Gilbert-Elliot p", "... r", "... k", "... h", "Gilbert-Elliot starts in good state" (only the fields of
//   the selected model are rendered), "Buffer capacity", "Reordering", "Seed" (empty -> null),
//   "Histogram bins", "Histogram lo (ns)", "Histogram hi (ns)" (all empty -> null/absent), "Delay statistics".
// - Buttons "Save" and link "Cancel"; save -> `createLink(input)` / `updateLink(id, input)` then /links.
//   API error -> `role="alert"` with the message, stays on the page, form values kept.
//
// ## Settings (v7 F5): EAL extra arguments are a list, see the header of settings.test.tsx.
// ## Interfaces / Host hints / Settings: see their test files.
// ## Static packaging checks (Dockerfile, nginx.conf, docker-compose.yml, F6 leftovers): src/static.test.ts.
import { render, act } from '@testing-library/react';
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

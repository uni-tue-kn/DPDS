// ## v27 (spec27, user 2026-11-04): presets are gone, replaced by Import/Export on the Settings page and Duplicate in the
//    Links list (supersedes EVERY statement below about presets, "Apply preset", "Save as preset", `/presets`, "Export setup",
//    "Import setup file" and the "Backup" card)
//   - The local document is { "version": 1, "links": Link[], "settings": Settings }: the app never writes a `presets`
//     member. A stored document with an extra `presets` member is still readable (the member is ignored, not migrated).
//   - No sidebar item "Presets", no `/presets` route (the router shows its not-found state), no "Apply preset" and no
//     "Save as preset" in the link editor. The editor's config fieldset holds only the subsections and the stage buttons.
//   - Settings: a toolbar at the TOP of the page with, right-aligned, buttons "Import" (icon bi-cloud-arrow-down-fill,
//     opens a hidden file input labelled "Import links file", accept ".json") and "Export" (icon bi-cloud-arrow-up-fill).
//     No "Backup" card, no "Export setup" button. Export downloads { version: 1, links } (links as stored, ids and
//     `avg_frame_size_byte` included, no settings) as `dpds-links-<YYYY-MM-DD>.json`. Import: the file must be a version 1
//     document whose `links` are well formed (extra members such as `presets` or `settings` are ignored), otherwise an
//     inline role=alert and nothing is written; a valid file opens a dialog ("Importing will replace all existing links")
//     whose "Confirm import" replaces `links` ONLY (settings stay), whose "Cancel" writes nothing; then role=status
//     "Imported <n> links". Import/Export are not disabled while an emulation runs.
//   - Links list: each row has a button "Duplicate <name>" (icon bi-files) between Edit and Delete. It opens the new-link
//     editor (`/links/new`) pre-filled with the copy: same config, ports, worker and frame size, name `<name>_copy`, if
//     taken `<name>_copy2`, `_copy3` ..., the base shortened so the name is at most 32 characters. Nothing is stored
//     until Save is clicked.
// ## v22 (spec22, user 2026-11-04): stateless controller, configuration only in the browser (supersedes EVERY REST
//    statement below about links, presets, settings, export/import and pending_restart; the v16/v21 UI rules stay unless
//    the "Optional stages" part says otherwise)
// ### Local document ("v22 contract": the constants and helpers `STORAGE_KEY`, `seedStore`, `storedDoc` ... below pin it)
//   - ONE `localStorage` key, `dpds.config.v1`, whose value is the JSON document
//       { "version": 1, "links": Link[], "presets": Preset[], "settings": Settings }
//     `Link` = {id, name, ingress, egress, config, worker?}, `Preset` = {id, name, description?, config}, `Settings` =
//     {main_lcore, eal_extra_args, poll_interval_ms} (the types of src/api/types.ts). The key `theme` is separate.
//   - Reading never writes: opening any page with an empty/absent store leaves the key absent. The first save (link,
//     preset, settings, import, "Load running configuration") creates the document. Missing parts read as: no links, no
//     presets, settings {main_lcore: null, eal_extra_args: [], poll_interval_ms: 500} (the old controller default).
//   - IDS ARE ASSIGNED IN THE BROWSER: a new link / preset gets `max(existing ids of its own list) + 1`, the first one
//     gets 1 (links and presets count separately). Editing keeps the id; ids are positive integers; `/links/:id` and
//     `/presets/:id` look the id up in the local document (unknown id -> role=alert /not found/i, no editor).
//     "Load running configuration" and import never invent ids differently: load assigns 1..n in the running order, import
//     keeps the ids of the file (an imported document must already be a valid v1 document).
//   - Names: a link name or preset name that another entry of the same list already has (exact match) is refused on save
//     with role=alert /already exists/i and nothing is written. (The controller re-validates at start.)
//   - Optional values are compared and persisted as "null or absent": tests treat `worker`, `seed`, `histogram`,
//     `delay_stats`-less histograms, and a missing stage as equal whether the key is null or missing (`dropNulls` below).
// ### Corrupt storage
//   Value not JSON, JSON of the wrong shape (links/presets not arrays, settings missing ...), or `version` != 1 ->
//   on EVERY page (checked on `/`, `/links`, `/settings`) a `role="alert"` whose text matches /local configuration/i and
//   whose button named /^reset/i resets it. The alert appears without any user action, the stored string is left untouched
//   until the button is clicked ("never silently drop"), and the Start button is disabled meanwhile. After Reset the
//   alert is gone and the store is absent or an empty valid document (links [], presets [], default settings).
// ### Start
//   `api.startEmulation(body)` with body = { links: [{name, ingress, egress, config, worker?}], settings: {main_lcore,
//   eal_extra_args, poll_interval_ms} } built from the local document: NO ids, NO presets, no other keys (the controller
//   has deny_unknown_fields; the Rust tests use exactly this shape). `worker`/`main_lcore` may be null or absent when
//   automatic. The client POSTs it as JSON to `/api/emulation/start`. Without local links nothing is sent: role=alert
//   /no links|nothing to start/i. Stop is unchanged (`api.stopEmulation()`).
// ### Pending restart (compares the local document with `emulation.config`, the running StartRequest)
//   `EmulationView.config` = null/absent while stopped|failed, else {links: [...], settings} where every link holds at least
//   {name, ingress, egress, config, worker?} (extra keys such as the assigned id/dir/lcore are ignored). Differences are
//   computed on {name, ingress, egress, config, worker} per link (matched BY NAME; null == absent) and on settings.
//   Only while the state is `starting` or `running`. The global notice is `data-testid="pending-restart"` (text
//   /restart/i) on the Dashboard AND on the Links page; it shows iff any local link differs from, is missing in, or is
//   extra to the running list, or the settings differ. On the Links page a link row that differs or is new also holds an
//   element `data-testid="link-pending-restart"`; unchanged rows hold none. `pending_restart` is gone from the API.
// ### Load running configuration
//   A button named "Load running configuration" on the Dashboard and on the Links page, shown iff the emulation reports a
//   `config` AND the local document has no links. Click -> the local document's links = the running links
//   ({id 1..n, name, ingress, egress, config, worker}), its settings = the running settings, presets untouched; the pages
//   then show the links and no pending-restart notice (the configurations are equal) and the button is gone.
// ### Export / import (Settings page, no REST at all)
//   "Export setup" downloads the local document (as stored, ids included) as `dpds-setup-<YYYY-MM-DD>.json`, through a
//   Blob + object URL + `<a download>` click like before. With nothing stored it exports the empty document. "Import setup
//   file" (file input) accepts a v1 document: valid -> confirm dialog (replace links, settings, presets) -> Confirm writes it
//   verbatim to the store and shows role=status "<n> links" / "<m> presets"; Cancel writes nothing. Not JSON or not a valid
//   v1 document -> role=alert mentioning JSON / the setup file, no dialog, nothing written. The input is enabled whatever the
//   emulation state is (no `import-hint`; the Settings page does not need the emulation).
// ### Host hints
//   `api.getHostHints()` (GET /api/host/hints: host-only hints) and, only if the local document has links,
//   `api.getHostHintsFor({links, settings})` (POST /api/host/hints with the start body above). The page lists both,
//   de-duplicated by hint `id` (the POST answer wins). No local links -> no POST. A failing POST shows its message
//   in a role=alert while the host-only hints stay; a failing GET shows its message.
// ### Removed REST surface
//   `src/api/client.ts` exports exactly: ApiError, getOnline, getInterfaces, getHostHints, getHostHintsFor, getEmulation,
//   startEmulation, stopEmulation, getStats, getDelayHistogram. No function for /links, /presets, /settings, /export,
//   /import. The mocked module (`mockApi.ts`) has the same surface, so a page that still calls a removed function fails.
// ### Optional stages (delayer, limiter, dropper added on demand)
//   - `ConfigDto.delayer`, `.limiter`, `.loss_model` are `X | null | undefined` (null/absent = stage not added).
//     `LimiterDto` = {mode, rate_bps}: the `activate` flag is gone (present = active). A present stage has all fields.
//   - Editor (link and preset): Buffer and Statistics are always shown. Delayer, Limiter, Dropper each start as ONE button
//     "Add delayer" / "Add limiter" / "Add dropper" with no fields of that stage rendered (no group of that name). Click
//     -> the subsection (role group named Delayer / Limiter / Dropper) with the fields, units and required markers of v16
//     and a button "Remove delayer" / "Remove limiter" / "Remove dropper" INSIDE the subsection (it is its only extra
//     control); the "Add ..." button is gone while the stage is added and comes back after removing. The sections keep
//     the order Buffer, Delayer, Limiter, Dropper, Statistics whatever the clicking order. Removing then re-adding in the
//     same form shows the values typed before. A stored config opens with exactly its present stages added. A removed or
//     never added stage is sent as null or absent. There is no "Limiter active" switch. Newly added stages show the v16
//     defaults (mean 1e6 ns, std 1e5 ns, half-life 0, correlation off, initialization rate EMPTY; Spacer, rate 1e10;
//     Random, p 0). The derived-limit info line lives in the Limiter subsection, so it exists only while the limiter is added.
//   - Links list: the Delay, Limiter and Loss cells read exactly `none` for a missing stage; the Limiter cell of a present
//     limiter is "spacing <rate>" / "policing <rate>". Inspect panel: only added stages get their group
//     (Buffer and Statistics always); every missing one is an element with own text exactly "Delayer: not added",
//     "Limiter: not added", "Dropper: not added" and has no group. The Limiter group lists "Limiter mode" and "Rate" only.
// ### Helpers added in this block: see below (`seedStore`, `storedDoc`, `startBodyOf`, `dropNulls`, `addStages` ...).
//
// Test helpers + THE FRONTEND CONTRACT the tests assume (spec v5 part D + v6 + v10 directed links + v12 + v16; the
// tester's choices where the spec is silent are listed here so the implementer can follow them or ask the coordinator
// to change them).
//
// ## v21 (spec20, user 2026-11-03): the limiter limit is derived, there is no "Burst limit" (supersedes the v16 text below)
//   - `limit_bit` is gone from the API (`LimiterDto` = {mode, activate, rate_bps}); the controller answers a body that
//     still has it with 400. dpds-core derives the limit for BOTH modes (Spacer fill-state limit, Token Bucket Filter
//     bucket depth) as `buffer_capacity x 1542 B x 8 bit` (1542 B = 1518 B maximum frame + 4 FEC + 20 B L1 overhead,
//     what the limiter counts per maximum frame: exactly `capacity` maximum frames fit).
//   - The config form has NO "Burst limit" field and no "Burst limit unit" select (every v16 mention of them below is void):
//     the Limiter subsection holds "Limiter active", "Limiter mode", "Rate" and "Rate unit" as controls, and the unit
//     selects are now seven. Of the limiter fields only Rate is required (while the limiter is active).
//   - The Limiter subsection also shows the derived limit READ-ONLY as plain information (not a control, no required
//     marker), one element whose own text is "Limit: derived from buffer capacity × 1542 B = <value> <unit>" (the
//     multiplication sign is U+00D7). `<value> <unit>` is capacity x 12336 bit in the editor's display-unit rule
//     (largest of bit | kbit | Mbit | Gbit with value >= 1 and at most 3 decimals): capacity 100000 -> "1233.6 Mbit",
//     1000 -> "12.336 Mbit", 321 -> "3959.856 kbit", 1000000 -> "12.336 Gbit". It follows the Buffer capacity field while
//     it is edited, is shown whatever the mode and the "Limiter active" switch say, and with an empty or invalid capacity
//     shows no number (never NaN).
//   - The Links list shows no limit (it never showed it); the inspect panel's Limiter subsection lists "Limiter" and,
//     only while the limiter is on, "Limiter mode" and "Rate": no "Burst limit" and no replacement item.
//   - No non-test source file mentions `limit_bit` or "Burst limit" (limitDerived.test.tsx scans them).
//
// ## v16: units per field, subsections, no typed ports, visibly required fields (supersedes the v8/v10/v12 label lists)
// The REST API and the stored values stay in base units (ns, bit/s, bit, pps); only the form shows units.
// ### Field labels (the unit suffix is gone from the label; the unit lives in a select next to the input)
//   Buffer: "Buffer capacity" (plain packet count, no unit), "Reordering", "Seed".
//   Delayer: "Delay distribution", "Mean delay", "Delay std", "Activate correlation", "Half-life period",
//     "Initialization rate" (was "Initialization rate (pps)"; message and label no longer carry the unit).
//   Limiter: "Limiter active", "Limiter mode", "Rate" (was "Rate (bit/s)"), "Burst limit" (was "Burst limit (bit)").
//   Dropper: "Loss model" (options Random | GilbertElliot only: the DTO has no "none", Random with p 0 is "no loss"),
//     "Random loss probability" | "Gilbert-Elliot p|r|k|h" (spec26: no "starts in good state" any more).
//   Statistics: "Delay statistics", "Histogram bins" (no unit), "Histogram lo" (was "Histogram lo (ns)"), "Histogram hi".
//   Validation messages name the label without a unit ("Mean delay must ...", never "(ns)"); the rules are on the base
//   value (Initialization rate: finite and >= 1 pps AFTER conversion, so 0.5 Mpps is fine and 0.9 pps is not).
// ### Unit selects
//   Fields with a unit: Mean delay, Delay std, Half-life period, Histogram lo, Histogram hi (time); Initialization rate
//   (pps); Rate (bit/s); Burst limit (bit). Each has ONE `<select>` whose accessible name is "<field label> unit"
//   (e.g. "Mean delay unit", "Initialization rate unit", "Rate unit", "Burst limit unit"), located in the same field
//   group as the input and directly AFTER it (no other form control between the input and its unit select, "right of
//   the input like P4TG"). No other field has a unit select (not Buffer capacity, Seed, Histogram bins, probabilities).
//   Option TEXTS in this order, exactly (tests pick options by text, values are free):
//     time   ns | µs | ms | s            (the micro sign is U+00B5, as in the spec and P4TG's "µs", never "us")
//     rate   bit/s | kbit/s | Mbit/s | Gbit/s  (Rate)
//     pps    pps | kpps | Mpps                 (Initialization rate)
//     size   bit | kbit | Mbit | Gbit          (Burst limit)
//   Multipliers: x1000 per step. A unit select is disabled exactly when its input is.
// ### Values and conversion (all decimal-exact; no float noise such as 1004999.9999999999)
//   - Display when a form opens with a stored base value: the LARGEST unit in which the value is >= 1 and has at most
//     3 decimals; 0 (and an empty optional field) shows the field's default unit. Examples: 10000000 ns -> "10" ms,
//     2500 ns -> "2.5" µs, 1001 ns -> "1.001" µs, 1234567 ns -> "1234.567" µs (ms would need 6 decimals),
//     6543210 pps -> "6543.21" kpps, 0 -> "0" in the default unit.
//   - Default units for empty/0 fields: delay mean/std ms, half-life ms, histogram lo/hi ms, Rate Gbit/s, Burst limit
//     Mbit, Initialization rate Mpps. A NEW link/preset form keeps the old default VALUES (mean 1e6 ns, std 1e5 ns,
//     half-life 0, rate 1e10 bit/s, limit 12e6 bit, buffer 100000, histogram empty) which are sent unchanged; only how
//     they are displayed is free (the tests read them as "shown value x selected unit"). Initialization rate starts EMPTY
//     (value null) with the unit select on Mpps.
//   - Changing a unit CONVERTS the shown number (value x oldMultiplier / newMultiplier, exact: 8.2 ms -> 8200 µs, 2500 ns ->
//     2.5 µs -> 0.0025 ms and back), it does not reinterpret it. An empty field stays empty.
//   - Saving: shown value x multiplier in base units, exact (1.005 ms -> 1005000, 8.2 ms -> 8200000, 1.001 µs -> 1001,
//     0.067 Gbit/s -> 67000000, 1.5 Gbit/s -> 1500000000). NO rounding to an integer: the API fields are f64, so
//     1.5 ns is sent as 1.5. buffer_capacity, seed and histogram bins stay integers as before.
// ### Subsections
//   The config form keeps its outer fieldset (legend "Link configuration" / "Preset configuration"). Inside it, in this
//   order, exactly five nested `<fieldset>`s with legends "Buffer", "Delayer", "Limiter", "Dropper", "Statistics"
//   (role group, accessible name = legend). Contents: Buffer = capacity, reordering, seed; Delayer = distribution, mean,
//   std, correlation switch, half-life, initialization rate (each with its unit select); Limiter = active, mode, rate,
//   limit; Dropper = loss model and the parameters of the chosen model; Statistics = delay statistics and the histogram
//   (bins, lo, hi). "Apply preset" and "Save as preset" are direct children of the outer fieldset, in no subsection.
//   Name / Worker lcore / Ingress / Egress stay in the card above (outside the config fieldset).
// ### Enable / disable (chosen: fields stay visible but DISABLED; the stored value is kept and still sent)
//   - Half-life period (+ its unit) is disabled while "Activate correlation" is off. Initialization rate is always enabled.
//   - Limiter mode, Rate, Burst limit (+ units) are disabled while "Limiter active" is off.
//   - Loss parameters: only the fields of the selected model are rendered (hidden, not disabled: they are alternatives);
//     switching the model away and back keeps the typed values.
//   - Histogram bins/lo/hi are NOT switch-dependent: enabled also while "Delay statistics" is off.
//   - Toggling a switch off and on keeps the field values; a disabled field's value is still in the payload.
//   - A conditionally required field that is empty while it is not required is sent as 0 (std with the deterministic
//     distribution, half-life with correlation off, Rate/Burst limit with an inactive limiter), never refused.
// ### Ports
//   The typed "Ingress (typed)" / "Egress (typed)" inputs are gone (also their hint). Ports come only from the pickers.
//   A stored port that is not a discovered interface (vdev, undiscovered PCI address) is an extra option, shown
//   selected, so an edit does not lose it; it is sent back unchanged. A stored port that fails the port rule
//   (e.g. "eth0") is still refused on save with an alert naming "ingress" / "egress".
// ### Required fields are visibly marked (user, 2026-11-03)
//   - Marker: an element with class `required-marker`, `aria-hidden="true"` and text exactly "*", inside the field's own
//     group (descendant of the parent element of the field's `<label>`, so it may sit in the label or right after it;
//     exactly one per required field, none otherwise). index.css gives `.required-marker` a red colour via
//     `var(--color-primary)`. aria-hidden keeps the accessible name stable: `getByLabelText("Mean delay")` and
//     `getByRole("spinbutton", {name: "Mean delay"})` keep matching (the implementer may put the marker into the label
//     and add `aria-label`, or place it after the label; both pass).
//   - The control carries the `required` attribute AND `aria-required="true"`; optional fields have neither.
//   - ONE note on the form (not aria-hidden) whose own text is exactly "* required".
//   - Required: Name (link and preset editor), Ingress, Egress (link editor); Buffer capacity; Mean delay; Delay std
//     unless the distribution is DeterministicDistribution; Half-life period only while correlation is on;
//     Initialization rate; Rate and Burst limit only while the limiter is active; every loss parameter of the chosen
//     model (Random loss probability, or Gilbert-Elliot p, r, k, h). Marked as soon as the rule holds, independent of
//     whether the field is filled.
//   - NOT marked: Description, Worker lcore, Seed, Histogram bins/lo/hi (all automatic when empty), the selects with a
//     default (Delay distribution, Limiter mode, Loss model, Apply preset), checkboxes, every unit select.
//   - The forms keep `noValidate`: the in-app alert messages still appear instead of the browser's own bubbles.
// ### Port labels, compact Links list, inspect panel (user, 2026-11-03; supersedes the v12 "<pci> (<netdev>)" label)
//   - A port is displayed by its Linux interface name (netdev) ONLY; a port with no netdev (vfio-bound NIC, vdev) or not
//     in the interface list shows its device name (PCI address / vdev name). Values, API payloads and `<option value>`
//     stay the device name. Applies to: Links list, dashboard card titles ("<link>: <ingress> -> <egress>", e.g.
//     "a-b: enp101s0f0 → enp101s0f1"), the first column of the dashboard NIC ports table, the editor pickers (option text
//     = netdev [+ " - " + reason]) and the inspect panel (which shows BOTH netdev and PCI). The Interfaces page is
//     unchanged (PCI and Netdev columns). Fixture netdevs: 0000:65:00.0 enp101s0f0, 0000:65:00.1 enp101s0f1,
//     0000:b3:00.0 enp179s0f0, 0000:17:00.0 eno1. The "once per page load, never an alert, never blocks" rules of v12 hold.
//   - Links list: columnheaders exactly [Name, Interfaces, Delay, Reordering, Limiter, Loss, <actions>] (7 columns, in this
//     order; the user chose one column per section over a single summary line). Row = name cell (own text exactly the
//     name), interface cell (ONE element, own text "enp101s0f0 → enp101s0f1", arrow U+2192), four config cells (cell index
//     2..5, own text exactly as below), actions: link "Edit <name>", button "Delete <name>", button "Inspect <name>".
//     No worker text, no PCI address, no parentheses, brackets or " · " in the row (the user's brackets were placeholders).
//   - Config cells:
//       Delay       "<dist> <mean> / <std>", dist = Normal | LogNormal | Gamma | Uniform (enum name without "Distribution"),
//                   e.g. "Normal 10 ms / 1 ms"; Deterministic shows the mean only ("Deterministic 10 ms"), also when a std is
//                   stored. mean and std each use the editor's display-unit rule on their own ("2.5 µs / 1 ms", "0 ms / 0 ms").
//       Reordering  "+" (on) or "−" (off); the minus is U+2212.
//       Limiter     "spacing <rate>" (Spacer), "policing <rate>" (TokenBucketFilter), "none" (inactive); rate in its display
//                   unit ("10 Gbit/s", "1.5 kbit/s"); the burst limit is not shown.
//       Loss        "Random p=<p>" | "Gilbert-Elliott p=<p>, r=<r>, h=<h>, k=<k>" (this order); numbers as stored, no float noise.
//     Examples: config() -> "Normal 1 µs / 10 ns" | "−" | "none" | "Random p=0";
//               config2() -> "Gamma 2.5 µs / 300 ns" | "+" | "policing 40 Gbit/s" | "Gilbert-Elliott p=0.01, r=0.2, h=0.3, k=0.9".
//   - Inspect = SIDE PANEL (react-bootstrap Offcanvas, the user's "slider"), not an expandable row. "Inspect <name>" opens
//     exactly one `role="dialog"` whose accessible name is the link name (it contains a heading with the name, one
//     "Close" button, and a link "Edit <name>" to /links/<id>); nothing is rendered while closed; it uses the list data
//     (no extra link request). Content: <dt>/<dd> pairs "Ingress" and "Egress" (dd holds netdev AND PCI address; a device
//     without netdev stands alone) and "Worker lcore" ("auto" or the number) outside any subsection, then five
//     `role="group"` subsections named Buffer, Delayer, Limiter, Dropper, Statistics in this order, each a list of <dt>/<dd>:
//       Buffer:     Buffer capacity "1000" | Reordering "on"/"off" | Seed "42" or "random"
//       Delayer:    Delay distribution (enum name) | Mean delay | Delay std | Correlation "on"/"off" |
//                   Half-life period (ONLY while correlation is on) | Initialization rate
//       Limiter:    Limiter "off" and nothing else, or Limiter "on" + Limiter mode | Rate | Burst limit
//       Dropper:    Loss model (enum name) + Random loss probability, or Gilbert-Elliot p | r | k | h (spec26: no good-state row)
//       Statistics: Delay statistics "on"/"off" and, only while on, Histogram bins | lo | hi ("automatic" when not set)
//     Unit fields read "<number> <unit>" by the editor's display rule ("2.5 \u00b5s", "12.5 Mpps", "40 Gbit/s", "120 kbit",
//     "4 ms"), probabilities as the plain stored number ("0.07", no float noise), no "(ns)" anywhere.
// ### Test helpers (below): `section`, `numberField`, `unitSelect`, `unitOf`, `pickUnit`, `typeInto`, `shownBase`,
//   and `typeRate(user, rate = TEST_RATE, unit = 'pps')`, which now also selects the unit (default pps, so TEST_RATE
//   stays 10,000,000 pps = what it always meant).
//
// ## v12: configurable initialization rate (its port-label part is superseded by the "Port labels, compact Links list,
//   inspect panel" block above: ports are shown by their netdev, never as "<pci> (<netdev>)")
// - Port labels: see that block. The Hints and Settings pages show no ports of their own: hint texts come from the
//   controller and are printed verbatim, so they do not call `getInterfaces`.
// - `getInterfaces` is called exactly ONCE per page load by the Links page and ONCE per mount by the Dashboard (not per
//   stats/emulation poll). A rejected call is not an error for these pages: no alert, no warning, plain device names.
//   A slow or never-resolving call does not delay the cards or rows (they appear with plain names and are relabelled).
// - Links list loss cell: superseded, it shows the stored numbers ("Random p=0.07", never "0.07000000000000001"), see the
//   compact Links list block above.
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
// - Links list: superseded by the table of the "compact Links list" block above (name, interfaces, config cells, actions;
//   no worker text). What stays: a hairpin reads "net_null0 → net_null0" in its interface cell, and no "Number of ports" /
//   "Directions" notion remains.
// - Link editor labels: "Name", select "Ingress", select "Egress" (same option rules as the old port pickers: text =
//   netdev [+ " - " + reason] when unusable, unusable/uplink options disabled, a stored non-interface port
//   such as a vdev stays selectable, no preselection, Save does nothing until both are chosen; there are no typed port
//   inputs any more, see "Ports" in the v16 block), and the optional `<input type="number">` "Worker lcore" (empty ->
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

// Toolchain: react-router 8, react-bootstrap + bootstrap 5, chart.js via
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
//   "Gilbert-Elliot p", "... r", "... k", "... h" (spec26: no good-state checkbox; only the fields of
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
// ## Static checks (no Docker/nginx files, vite dev proxy, CI without a frontend asset job, README, F6 leftovers): src/static.test.ts.
import { render, act, screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import App from '../App';
import type { ConfigDto, Link, Settings } from '../api/types';

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

/** A valid initialization rate for flows that create a link (v12: new forms start with this field EMPTY). */
export const TEST_RATE = '10000000';

// ---- v16: units, subsections ------------------------------------------------------------------------------------

export const TIME_UNITS = ['ns', '\u00b5s', 'ms', 's'];
export const RATE_UNITS = ['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s'];
export const PPS_UNITS = ['pps', 'kpps', 'Mpps'];
export const SIZE_UNITS = ['bit', 'kbit', 'Mbit', 'Gbit'];
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
};
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

// ---- v16: required markers --------------------------------------------------------------------------------------

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

// ---- v22: the local document, the start body, optional stages -----------------------------------------------------

/** THE storage key of the local document (v22 contract). */
export const STORAGE_KEY = 'dpds.config.v1';
/** What the Settings page shows (and Start sends) while nothing is stored. */
export const DEFAULT_SETTINGS: Settings = { main_lcore: null, eal_extra_args: [], poll_interval_ms: 500 };

/** The document shape of the v22 contract. */
export interface StoredDoc {
  version: 1;
  links: Link[];
  settings: Settings;
}

export function emptyDoc(): StoredDoc {
  return { version: 1, links: [], settings: { ...DEFAULT_SETTINGS } };
}

/** Writes a document (missing parts empty) to the store, as an earlier session of the app would have. */
export function seedStore(over: Partial<StoredDoc> = {}): StoredDoc {
  const doc: StoredDoc = { ...emptyDoc(), ...over };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(doc));
  return doc;
}
export const seedLinks = (links: Link[]) => seedStore({ links });
export const seedSettings = (settings: Settings) => seedStore({ settings });
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

const START_LINK_KEYS = ['name', 'ingress', 'egress', 'config', 'worker'];
const START_SETTINGS_KEYS = ['main_lcore', 'eal_extra_args', 'poll_interval_ms'];

/** The start body the document must produce: links without ids, settings, nothing else. */
export function startBodyOf(doc: StoredDoc): { links: unknown[]; settings: unknown } {
  return {
    links: doc.links.map((l) => ({
      name: l.name,
      ingress: l.ingress,
      egress: l.egress,
      config: l.config,
      worker: l.worker,
    })),
    settings: doc.settings,
  };
}

/** `actual` (the argument of startEmulation / getHostHintsFor) has exactly the contracted shape and the content of `doc`. */
export function expectStartBody(actual: unknown, doc: StoredDoc): void {
  const body = actual as { links: Record<string, unknown>[]; settings: Record<string, unknown> };
  expect(Object.keys(body).sort()).toEqual(['links', 'settings']);
  expect(body.links).toHaveLength(doc.links.length);
  for (const l of body.links) {
    expect(Object.keys(l).filter((k) => !START_LINK_KEYS.includes(k))).toEqual([]);
    expect(l).not.toHaveProperty('id');
  }
  expect(Object.keys(body.settings).filter((k) => !START_SETTINGS_KEYS.includes(k))).toEqual([]);
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

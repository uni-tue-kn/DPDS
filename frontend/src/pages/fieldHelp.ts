// What each form field means, its unit and its valid range: the text of the info tooltip after the label. Every fact is
// taken from the Rust source under crates/ (dpds-core, dpds-controller, dpds); the comment above each entry names the
// file:line it rests on, or says it is UI behaviour. The keys are the label texts, except where one label has two meanings.

export const HELP = {
  // dpds-controller/src/api.rs:232-237 (valid_name), model.rs:10-11 (MAX_NAME_CHARS = 32)
  'Link name':
    'Name of this link. 1 to 32 characters: letters, digits and _ . - (the controller refuses anything else at start). Must be unique among your links.',
  // no backend source: presets exist only in this browser (localConfig.ts refuseDuplicate; PresetEditor maxLength 64)
  'Preset name':
    'Name of this preset, up to 64 characters. Must be unique among your presets. Kept only in this browser.',
  // no backend source: presets are not part of the controller's model (model.rs)
  Description: 'Optional note about this preset. Kept only in this browser, never sent to the controller.',
  // dpds-controller/src/lcores.rs:65-67 (override), 83-106 (online, not main, not shared), 108-136 (automatic pick); model.rs worker is Option<u32>
  'Worker lcore':
    'CPU core that runs this link\u2019s forwarding loop (receive, delay, send). A whole number of at least 0: an online core that is not the main lcore and not used by another link. Empty: automatic, the lowest free isolated core, preferring the NUMA node of the ingress NIC.',
  // dpds-controller/src/model.rs:12-15 (one ingress and one egress per port, hairpin allowed), api.rs:295-305 (409 on reuse), dpds/src/main.rs:3-6
  Ingress:
    'Port this link receives packets on. A port can be the ingress of at most one link. The same port may also be the egress (hairpin).',
  Egress:
    'Port this link sends its packets out on. A port can be the egress of at most one link. The same port may also be the ingress (hairpin).',
  // UI behaviour only (ConfigFieldset copies configToForm(preset.config, preset.avg_frame_size_byte) into the form)
  'Apply preset':
    'Copies the settings of a preset into this form, including its average frame size. The link keeps its own copy: later changes to the preset do not affect it.',
  // dpds-core/src/buffer.rs:259-261 (BufferFull when len >= capacity), dpds-controller/src/dto.rs:18-19 and 210-215 (at most 2^31), dpds-core/src/lib.rs:159-173 (derived limiter limit)
  'Buffer capacity':
    'Most packets the buffer of this link holds at once (a number of packets, 0 to 2147483648 = 2^31). A packet arriving at a full buffer is dropped. The limiter\u2019s limit is derived from it.',
  // dpds-core/src/lib.rs:106-113
  Reordering:
    'Off: packets leave in arrival order (FIFO), so a packet with a late send time holds back the ones behind it. On: packets leave in send-time order, so a packet with a short delay can overtake one with a long delay.',
  // dpds-core/src/lib.rs:94-100 (seeding), dpds-controller/src/dto.rs:162-163 (u64); the 2^53 - 1 cap is this form's own (linkForm.ts seed())
  Seed: 'Seed of the random generators for delay and loss. A whole number from 0 to 9007199254740991 (2^53 \u2212 1, the most this form can hold). The same seed and input give the same results. Empty: a random seed from the operating system.',
  // spec 25 (UI only); the 20 B L1 overhead is dpds-core/src/limiter.rs:1 (L1_OVERHEAD = 7 + 1 + 12)
  'Average frame size':
    'Average Ethernet frame size in bytes, including the 4 B FCS (e.g. 1518); at least 64, decimals allowed. Only converts between packets/s and bit/s using 20 B of preamble and inter-frame gap per frame: pps = bit/s / ((frame + 20) \u00d7 8). Kept in this browser, never sent to the controller. Empty: the rate fields offer only their own unit family.',
  // dpds-core/src/delayer.rs:89-129 (distributions), 275-319 (sampling, clamp at 0), dpds-controller/src/dto.rs:235-242
  'Delay distribution':
    'Distribution of the per-packet delay, given by mean and standard deviation. Deterministic: every packet gets exactly the mean. Uniform: spread evenly around the mean. A sample below 0 counts as 0. Parameters the distribution cannot use are refused at start.',
  // dpds-controller/src/dto.rs:235-242 (finite, >= 0; ns)
  'Mean delay': 'Mean of the delay each packet gets. Stored in ns (the unit select converts); finite and at least 0.',
  // dpds-controller/src/dto.rs:235-242; dpds-core/src/delayer.rs:85-87 (std compensation), 101 (Deterministic ignores it), 319 (clamp at 0)
  'Delay std':
    'Standard deviation of the delay. Stored in ns; finite and at least 0. Ignored by the Deterministic distribution. With correlation the random draws are scaled so that the smoothed delay has this std in the long run, before negative delays are cut to 0.',
  // dpds-core/src/delayer.rs:80-83 (weight), 309-312 (smoothing)
  'Activate correlation':
    'Makes successive delays depend on each other: each packet\u2019s delay is w \u00d7 the previous delay + (1 \u2212 w) \u00d7 a fresh random draw. w follows from the half-life period and the packet rate.',
  // dpds-core/src/delayer.rs:80-83 (weight formula), 221-225 (checked always), 233-246 (weight >= 1 refused with correlation)
  'Half-life period':
    'Time over which the influence of earlier delays halves when correlation is on (w = 2^(\u22121 / (half-life \u00d7 packet rate))). Stored in ns; finite and at least 0, checked even with correlation off. With correlation, a value so long that w reaches 1 is refused at start.',
  // dpds-core/src/delayer.rs:36, 50 (1 ms estimate), 216-220 (always checked), 231, 275-284; dpds-core/src/lib.rs:152-156; dpds-controller/src/dto.rs:61-63
  'Initialization rate':
    'Required, with or without correlation, but it only matters with correlation on: the packet rate (packets/s) the correlation assumes at start-up until the first measured rate (updated every 1 ms) replaces it. Use the expected packet rate of this link. Stored in pps; finite and at least 1.',
  // dpds-core/src/limiter.rs:25-26 (Spacer starts empty, TBF starts full), 78-107 (Spacer), 110-127 (TBF), dpds-core/src/lib.rs:159-173 (derived limit)
  'Limiter mode':
    'Spacer: delays packets so the link never exceeds the rate (a queue that starts empty); a packet that would overflow the derived limit shown below is dropped. TokenBucketFilter: packets within the token budget pass undelayed (a bucket that starts full), the rest are dropped.',
  // dpds-controller/src/dto.rs:249-254 (finite, > 0); dpds-core/src/buffer.rs:266 (+4 B FCS) and limiter.rs:1, 63 (+20 B L1)
  Rate: 'Rate the limiter allows. Stored in bit/s; finite and greater than 0. Each packet counts as its Ethernet frame incl. FCS plus 20 B of preamble and inter-frame gap.',
  // dpds-core/src/dropper.rs:7-17 (parameters), 56-85 (decision and transition)
  'Loss model':
    'Random: every packet is dropped independently with one probability. Gilbert-Elliot: a two-state (good/bad) chain with bursty loss; each packet is dropped or kept by the current state, then the state may change.',
  // dpds-controller/src/dto.rs:194-202 and 216-221 (0 to 1); dpds-core/src/dropper.rs:58-62 (p = 0 never drops)
  'Random loss probability': 'Probability that a packet is dropped, between 0 and 1 (0.01 = 1 %). 0 never drops.',
  // dpds-core/src/dropper.rs:11-17 (definitions), 74-81 (transitions); dpds-controller/src/dto.rs:222-231 (all in [0, 1])
  'Gilbert-Elliot p': 'Probability, per packet, of switching from the good to the bad state. Between 0 and 1.',
  'Gilbert-Elliot r': 'Probability, per packet, of switching from the bad back to the good state. Between 0 and 1.',
  // dpds-core/src/dropper.rs:11-17, 68-72 (drop when the draw > k)
  'Gilbert-Elliot k':
    'Probability that a packet is delivered (not dropped) while in the good state, so loss there is 1 \u2212 k. Between 0 and 1.',
  // dpds-core/src/dropper.rs:11-17, 68-72 (drop when the draw > h)
  'Gilbert-Elliot h':
    'Probability that a packet is delivered (not dropped) while in the bad state, so loss there is 1 \u2212 h. Between 0 and 1.',
  // dpds-controller/src/dto.rs:166-170
  'Delay statistics':
    'Record the intended delay, lateness and delay histogram of this link. Off: dpds skips that work; the counters and drop causes stay.',
  // dpds-controller/src/dto.rs:15-16 and 121-129 (1 to 500); dpds/src/delay_stats.rs:46-48 (256 default, 500 max), 209-221
  'Histogram bins': 'Number of equal-width bins of the delay histogram: a whole number from 1 to 500. Empty: 256 bins.',
  // dpds/src/delay_stats.rs:30-31 (range [lo, lo + bins x width)), 131-152 (automatic window), 206-221 (from_spec), 229-231 (underflow); dpds-controller/src/dto.rs:130-143
  'Histogram lo':
    'Lower edge of the histogram range (included), the start of the first bin; delays below it are counted as underflow. Stored in ns; finite, at least 0 and below Histogram hi. Empty: the mean \u2212 6 std, but not below 0. If the automatic range is narrower than 1 \u00b5s it is recentred to exactly 1 \u00b5s wide: lo = max(0, midpoint \u2212 500 ns).',
  // dpds/src/delay_stats.rs:131-152 (automatic window incl. the Spacer allowance, 1 us minimum), 206-221, 229-231; dpds-controller/src/dto.rs:130-143
  'Histogram hi':
    'Upper edge of the histogram range (excluded); delays from it on are counted as overflow. Stored in ns; finite and above Histogram lo. Empty: the mean + 6 std (plus the queueing allowance of an active Spacer), but at least 1 \u00b5s above lo. A value you enter is used as given.',
  // dpds-controller/src/lcores.rs:72-79 (online core or lowest non-isolated), 92-97 (no link on it); dpds/src/main.rs:5-6; model.rs:34
  'Main lcore':
    'CPU core of the EAL main lcore, which handles signals and polls the NIC counters; it forwards no packets, so no link may use it. A whole number of at least 0 that is an online core. Empty: the lowest online core that is not isolated. Applies at the next start.',
  // dpds-controller/src/validate.rs:46-81 (allow-list), eal.rs:69 (appended after the fixed arguments), model.rs:36
  'EAL extra arguments':
    'Extra DPDK EAL arguments, added after the fixed ones, one per row. Only these are allowed: --no-huge, --no-pci, -m <MB>, -n <channels>, --socket-mem=<MB>[,<MB>...] and --log-level=<spec> (an option and its value, like -m and 256, take two rows). Applies at the next start.',
  // dpds-controller/src/api.rs:312-316 (>= 1), telemetry.rs:147 (waits interval_ms between polls), 154 (default 500), 215; model.rs:38-39
  'Poll interval (ms)':
    'How long, in milliseconds, the controller waits between two polls of the running dpds for its statistics. A whole number of at least 1; the default is 500. Applies at the next start.',
  // no backend source: UI behaviour (Settings.tsx confirmImport)
  'Import setup file':
    'A JSON file made by Export setup. After you confirm, it replaces all links, presets and settings stored in this browser.',
} as const;

export type HelpKey = keyof typeof HELP;

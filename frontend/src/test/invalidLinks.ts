// Tables of invalid and edge-case links for the stored-document and import checks (localStore.test.ts). A link is valid only if the
// link form would accept it unchanged (name rule, non-empty ports, a config that survives the form round trip, optional frame size).
// Each entry is a whole link, built on `fx.link()`.
import * as fx from './fixtures';

type Loose = Record<string, unknown>;

/** A link that differs from `fx.link()` by `over` (members may be removed with `undefined`, which JSON drops). */
function link(over: Loose): Loose {
  return { ...fx.link(), ...over };
}
function withDelayer(over: Loose): Loose {
  return link({ config: { ...fx.config(), delayer: { ...fx.config().delayer, ...over } } });
}
function withConfig(over: Loose): Loose {
  return link({ config: { ...fx.config(), ...over } });
}

export interface InvalidLink {
  label: string;
  link: Loose;
  /** What the import error must say is wrong (the first offending link is named separately). */
  problem: RegExp;
}

export const INVALID_LINKS: InvalidLink[] = [
  { label: 'config {}', link: link({ config: {} }), problem: /buffer capacity|buffer_capacity|config/i },
  { label: 'delayer {}', link: link({ config: { ...fx.config(), delayer: {} } }), problem: /mean|delayer/i },
  {
    label: 'a config without buffer_capacity',
    link: link({ config: { ...fx.config(), buffer_capacity: undefined } }),
    problem: /buffer capacity|buffer_capacity/i,
  },
  {
    label: 'a delayer without initialization_rate_pps',
    link: withDelayer({ initialization_rate_pps: undefined }),
    problem: /initialization rate|initialization_rate_pps/i,
  },
  {
    label: 'an initialization rate below 1 pps',
    link: withDelayer({ initialization_rate_pps: 0.5 }),
    problem: /initialization rate/i,
  },
  { label: 'a negative delay std', link: withDelayer({ std_ns: -1 }), problem: /std/i },
  { label: 'a negative buffer capacity', link: withConfig({ buffer_capacity: -1 }), problem: /buffer capacity/i },
  { label: 'a fractional buffer capacity', link: withConfig({ buffer_capacity: 1.5 }), problem: /buffer capacity/i },
  {
    label: 'a loss probability above 1',
    link: withConfig({ loss_model: { type: 'Random', p: 2 } }),
    problem: /loss probability|between 0 and 1/i,
  },
  {
    label: 'a limiter rate of 0',
    link: withConfig({ limiter: { mode: 'Spacer', rate_bps: 0 } }),
    problem: /rate/i,
  },
  { label: 'a fractional seed', link: withConfig({ seed: 1.5 }), problem: /seed/i },
  { label: 'an empty name', link: link({ name: '' }), problem: /name/i },
  { label: 'a name with a space', link: link({ name: 'has space' }), problem: /name/i },
  { label: 'a name of 33 characters', link: link({ name: 'x'.repeat(33) }), problem: /name/i },
  { label: 'a name with a non-ASCII letter', link: link({ name: 'ä-b' }), problem: /name/i },
  { label: 'an empty ingress', link: link({ ingress: '' }), problem: /ingress/i },
  { label: 'an empty egress', link: link({ egress: '' }), problem: /egress/i },
  { label: 'a frame size below 64', link: link({ avg_frame_size_byte: 63 }), problem: /frame/i },
  { label: 'a frame size that is a string', link: link({ avg_frame_size_byte: '1518' }), problem: /frame/i },
];

/** Links at the edge of validity: all of them are accepted. */
export const EDGE_LINKS: [string, Loose][] = [
  ['frame size 64', link({ avg_frame_size_byte: 64 })],
  ['frame size null', link({ avg_frame_size_byte: null })],
  ['a name of 32 characters', link({ name: 'x'.repeat(32) })],
  ['a name with . - _ and digits', link({ name: 'A-z_0.9' })],
  ['a config without stages', link({ config: fx.configBare() })],
  ['config2 (every stage, Gilbert-Elliott, histogram)', link({ config: fx.config2() })],
  ['config3', link({ config: fx.config3() })],
  ['a config without delay_stats', link({ config: { ...fx.config(), delay_stats: undefined } })],
  ['a vdev port', link({ ingress: 'net_null0', egress: 'net_null0' })],
];

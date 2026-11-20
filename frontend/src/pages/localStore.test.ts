import { describe, expect, it } from 'vitest';
import type { EmulationState, Link } from '../api/types';
import {
  STORAGE_KEY,
  emptyConfig,
  isLinksFile,
  isLocalConfig,
  linksProblem,
  parseStored,
  readConfig,
  readRaw,
  resetConfig,
  startRequestOf,
  updateConfig,
  writeConfig,
} from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import { dropNulls } from '../test/dropNulls';
import * as fx from '../test/fixtures';
import { EDGE_LINKS, INVALID_LINKS } from '../test/invalidLinks';
import { must } from '../test/must';

// Pure logic of the browser-side document `{version: 1, links}` (key `dpds.config.v1`): reading, the corrupt cases, writing, the
// import checks, the Start body and the pending-restart comparison. Nothing is rendered.

const doc = (links: unknown, over: Record<string, unknown> = {}) => JSON.stringify({ version: 1, links, ...over });
const LINKS = (): Link[] => [fx.link(), fx.linkRev()];
const NOT_READABLE: [string, string][] = [
  ['not JSON', '{not json'],
  ['an empty string', ''],
  ['JSON null', 'null'],
  ['a JSON array', '[]'],
  ['an unknown version', JSON.stringify({ version: 2, links: [] })],
  ['version 0', JSON.stringify({ version: 0, links: [] })],
  ['a missing version', JSON.stringify({ links: [] })],
  ['links that are not a list', JSON.stringify({ version: 1, links: {} })],
];

describe('reading the document', () => {
  it('nothing stored is the empty document; reading never creates it', () => {
    expect(parseStored(null)).toEqual({ corrupt: false, doc: { version: 1, links: [] } });
    expect(readConfig().corrupt).toBe(false);
    expect(readRaw()).toBeNull();
    expect(emptyConfig()).toEqual({ version: 1, links: [] });
  });

  it.each(NOT_READABLE)('%s is corrupt and shown as the empty document', (_n, raw) => {
    expect(parseStored(raw)).toEqual({ corrupt: true, doc: { version: 1, links: [] } });
  });

  it('a corrupt value is left untouched by reading', () => {
    for (const [, raw] of NOT_READABLE) {
      localStorage.setItem(STORAGE_KEY, raw);
      expect(readConfig().corrupt).toBe(true);
      expect(readRaw()).toBe(raw);
    }
  });

  it('a valid document is read as it is; the empty one is valid', () => {
    expect(parseStored(doc(LINKS()))).toEqual({ corrupt: false, doc: { version: 1, links: LINKS() } });
    expect(parseStored(doc([])).corrupt).toBe(false);
  });

  it('unknown members (old settings, presets, a link name or worker) are ignored, not an error, and never part of the result', () => {
    const raw = doc([{ ...fx.link(), name: 'old', worker: { lcore: 3 } }], {
      settings: { main_lcore: 2, eal_extra_args: ['--no-huge'] },
      presets: [{ name: 'p' }],
    });
    const parsed = parseStored(raw);
    expect(parsed.corrupt).toBe(false);
    expect(parsed.doc).toStrictEqual({ version: 1, links: [fx.link()] });
  });

  it('keeps the average frame size of a link', () => {
    expect(parseStored(doc([{ ...fx.link(), avg_frame_size_byte: 512 }])).doc.links[0].avg_frame_size_byte).toBe(512);
  });

  it.each(INVALID_LINKS)('$label makes the whole document corrupt, valid links beside it included', ({ link }) => {
    expect(parseStored(doc([fx.link({ id: 7 }), link])).corrupt).toBe(true);
  });

  it.each(EDGE_LINKS)('%s is still a valid link', (_label, link) => {
    const parsed = parseStored(doc([link]));
    expect(parsed.corrupt).toBe(false);
    expect(dropNulls(JSON.parse(JSON.stringify(parsed.doc.links[0])))).toEqual(
      dropNulls(JSON.parse(JSON.stringify(link))),
    );
  });

  it('isLocalConfig and isLinksFile: the shapes of the stored document and of an export file', () => {
    expect(isLocalConfig({ version: 1, links: LINKS() })).toBe(true);
    expect(isLocalConfig({ version: 1, links: [{ ...fx.link(), ingress: '' }] })).toBe(false);
    expect(isLocalConfig({ version: 1, links: [{ ...fx.link(), name: 'old' }] })).toBe(true);
    expect(isLinksFile({ version: 1, links: [{ not: 'checked here' }] })).toBe(true);
    for (const bad of [null, [], 'x', { version: 2, links: [] }, { version: 1 }, { version: 1, links: {} }]) {
      expect(isLinksFile(bad)).toBe(false);
    }
  });
});

describe('writing and resetting the document', () => {
  it('writeConfig stores exactly version and links, and each link only id, ports, config and frame size', () => {
    const dirty = { ...fx.link(), avg_frame_size_byte: 512, name: 'old', worker: { lcore: 3 } } as unknown as Link;
    writeConfig({ version: 1, links: [dirty], settings: { main_lcore: 2 } } as never);
    const stored = JSON.parse(must(readRaw())) as { version: number; links: Record<string, unknown>[] };
    expect(Object.keys(stored).sort()).toEqual(['links', 'version']);
    expect(Object.keys(stored.links[0]).sort()).toEqual(['avg_frame_size_byte', 'config', 'egress', 'id', 'ingress']);
    expect(readConfig()).toEqual({
      corrupt: false,
      doc: { version: 1, links: [{ ...fx.link(), avg_frame_size_byte: 512 }] },
    });
  });

  it('resetConfig removes the value (corrupt or not) and leaves other keys alone', () => {
    localStorage.setItem('theme', 'dark');
    localStorage.setItem(STORAGE_KEY, '{not json');
    resetConfig();
    expect(readRaw()).toBeNull();
    expect(readConfig().corrupt).toBe(false);
    expect(localStorage.getItem('theme')).toBe('dark');
  });

  it('updateConfig starts from a fresh read, so a change made elsewhere is not overwritten', () => {
    writeConfig({ version: 1, links: [fx.link()] });
    localStorage.setItem(STORAGE_KEY, doc([fx.link(), fx.linkRev()])); // another tab
    updateConfig((d) => ({ ...d, links: d.links.filter((l) => l.ingress !== fx.link().ingress) }));
    expect(readConfig().doc.links.map((l) => l.ingress)).toEqual([fx.linkRev().ingress]);
  });

  it('updateConfig refuses an unreadable value and leaves its text untouched', () => {
    for (const [, raw] of NOT_READABLE) {
      localStorage.setItem(STORAGE_KEY, raw);
      expect(() => updateConfig((d) => d)).toThrow(/cannot be read/);
      expect(readRaw()).toBe(raw);
    }
  });
});

describe('linksProblem: what an import refuses, naming the first offending link', () => {
  it('accepts a valid list and the empty one', () => {
    expect(linksProblem(LINKS())).toBeNull();
    expect(linksProblem([])).toBeNull();
  });

  it.each(INVALID_LINKS)('$label: names position 2 and the problem', ({ link, problem }) => {
    const m = must(linksProblem([fx.link({ id: 7 }), link]));
    expect(m).toMatch(/^Link 2: /);
    expect(m).toMatch(problem);
  });

  it('with several bad links only the first is named', () => {
    const m = must(linksProblem([fx.link({ ingress: '' }), fx.link({ id: 2, egress: '' })]));
    expect(m).toMatch(/^Link 1/);
    expect(m).not.toMatch(/Link 2/);
  });

  it('a link that is not an object is located by position', () => {
    expect(linksProblem([fx.link(), 'x'])).toMatch(/^Link 2: /);
    expect(linksProblem([fx.link(), null])).toMatch(/^Link 2: /);
  });

  it('two links with the same id are refused, naming both positions', () => {
    expect(linksProblem([fx.link(), fx.linkRev({ id: 1 })])).toMatch(/Duplicate link id 1.*1 and 2/);
  });

  it('an extra name member is accepted (and dropped on import, like any unknown member)', () => {
    expect(
      linksProblem([
        { ...fx.link(), name: 'old' },
        { ...fx.linkRev(), name: '' },
      ]),
    ).toBeNull();
  });
});

describe('the Start body', () => {
  it('has the links without ids and nothing but ingress, egress and config', () => {
    const links = [
      { ...fx.link(), avg_frame_size_byte: 512, name: 'old', worker: { lcore: 1 } } as unknown as Link,
      fx.linkRev(),
    ];
    expect(startRequestOf({ version: 1, links })).toStrictEqual({
      links: links.map((l) => ({ ingress: l.ingress, egress: l.egress, config: l.config })),
    });
  });

  it('is {links: []} for the empty document', () => {
    expect(startRequestOf(emptyConfig())).toStrictEqual({ links: [] });
  });
});

describe('pending restart: the local links against the running start request', () => {
  const running = (links: Link[] = LINKS(), state: EmulationState = 'running') =>
    fx.emulation(state, { config: fx.runningConfig(links) });
  const local = (links: Link[] = LINKS()) => ({ version: 1 as const, links });
  const meanOf = (l: Link, mean_ns: number): Link => ({
    ...l,
    config: { ...l.config, delayer: { ...must(l.config.delayer), mean_ns } },
  });

  it('nothing differs when the links are the same: ids, controller extras (id, dir, lcore) and the frame size do not count', () => {
    const p = pendingRestart(local([{ ...fx.link(), id: 9, avg_frame_size_byte: 512 }, fx.linkRev()]), running());
    expect(p.any).toBe(false);
    expect(p.links.size).toBe(0);
  });

  it('null and absent optional values are the same; so is the order of the keys', () => {
    const { seed: _s, histogram: _h, ...absent } = fx.config();
    const a = fx.link({ config: absent as Link['config'] });
    expect(pendingRestart(local([a]), running([fx.link()])).any).toBe(false);
    const reordered = fx.link({
      config: JSON.parse(JSON.stringify(Object.fromEntries(Object.entries(fx.config()).reverse()))),
    });
    expect(pendingRestart(local([reordered]), running([fx.link()])).any).toBe(false);
  });

  it('a changed link is flagged by its ingress port; the unchanged one is not', () => {
    const p = pendingRestart(local([meanOf(fx.link(), 5), fx.linkRev()]), running());
    expect(p.any).toBe(true);
    expect([...p.links]).toEqual([fx.link().ingress]);
  });

  it('a port or histogram difference counts too', () => {
    expect(pendingRestart(local([fx.link({ egress: 'net_null0' }), fx.linkRev()]), running()).any).toBe(true);
    const h = fx.link({ config: { ...fx.config(), histogram: { bins: 64, lo_ns: null, hi_ns: null } } });
    expect(pendingRestart(local([h, fx.linkRev()]), running()).any).toBe(true);
  });

  it('a link only known locally is flagged; one removed locally raises the notice with nothing to mark', () => {
    const added = pendingRestart(local([...LINKS(), fx.loopLink()]), running());
    expect([added.any, [...added.links]]).toEqual([true, [fx.loopLink().ingress]]);
    const removed = pendingRestart(local([fx.link()]), running());
    expect([removed.any, removed.links.size]).toEqual([true, 0]);
  });

  it('links are matched by ingress port, not by position or id; a link with another ingress is a new one', () => {
    expect(pendingRestart(local([fx.linkRev({ id: 1 }), fx.link({ id: 2 })]), running()).any).toBe(false);
    const moved = pendingRestart(local([fx.link({ ingress: 'net_null1' }), fx.linkRev()]), running());
    expect([moved.any, [...moved.links]]).toEqual([true, ['net_null1']]);
  });

  it.each([
    ['starting', true],
    ['running', true],
    ['stopping', false],
    ['stopped', false],
    ['failed', false],
  ] as [EmulationState, boolean][])('while the emulation is %s a difference is reported: %s', (state, reported) => {
    expect(pendingRestart(local([meanOf(fx.link(), 5)]), running([fx.link()], state)).any).toBe(reported);
  });

  it('there is nothing to compare without a running config, without an emulation, or without local links', () => {
    const changed = local([meanOf(fx.link(), 5)]);
    expect(pendingRestart(changed, fx.emulation('running')).any).toBe(false);
    expect(pendingRestart(changed, undefined).any).toBe(false);
    expect(pendingRestart(local([]), running()).any).toBe(false);
  });
});

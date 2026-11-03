import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { screen, within } from '@testing-library/react';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import * as fx from '../test/fixtures';
import { numberField, section } from '../test/helpers';

// Contract: "v21" block of src/test/helpers.tsx (spec20). The limiter's limit is derived by dpds-core from the buffer
// capacity (capacity x 1542 B x 8 bit, the same for Spacer and Token Bucket Filter); `limit_bit` is gone from the API.
// The form has no "Burst limit" field; the Limiter subsection shows the derived limit read-only. Every test runs
// against the link editor AND the preset editor (they share ConfigFieldset).
const body = () => document.body;
const BITS_PER_PACKET = 1542 * 8; // 12336

/** The text of the info line: "Limit: derived from buffer capacity × 1542 B = <value> <unit>". */
const INFO = /Limit: derived from buffer capacity × 1542 B/;
const infoWith = (value: string) => new RegExp(`Limit: derived from buffer capacity × 1542 B = ${value.replace('.', '\\.')}$`);

function infoElement(): HTMLElement {
  return within(section('Limiter')).getByText(INFO);
}

describe.each(PAGES)('$name: no burst limit field', (page: ConfigPage) => {
  it('has neither a "Burst limit" control, nor its unit select, nor any such text', async () => {
    await page.openNew();
    expect(screen.queryByLabelText('Burst limit')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Burst limit unit')).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/burst/i)).not.toBeInTheDocument();
    expect(body().textContent).not.toMatch(/burst limit/i);
  });

  it('the Limiter subsection holds exactly four controls: active, mode, rate and its unit', async () => {
    await page.openNew();
    const controls = Array.from(section('Limiter').querySelectorAll('input, select, textarea, button'));
    expect(controls).toHaveLength(4);
    expect(controls.map((c) => c.getAttribute('aria-label') ?? '').filter(Boolean)).not.toContain('Burst limit');
    for (const l of ['Limiter active', 'Limiter mode', 'Rate', 'Rate unit']) {
      expect(section('Limiter')).toContainElement(within(body()).getByLabelText(l));
    }
  });

  it('sends the limiter as exactly {mode, activate, rate_bps}', async () => {
    const user = await page.openNew();
    await page.ready(user);
    await user.click(screen.getByLabelText('Limiter active'));
    await page.save(user);
    expect(page.created().limiter).toEqual({ mode: 'Spacer', activate: true, rate_bps: 1e10 });
  });

  it.each([true, false])('a stored limiter (active=%s) is sent back without any limit field', async (activate) => {
    const user = await page.openStored(cfg({}, { activate, rate_bps: 4e10 }));
    await page.save(user);
    expect(page.updated().limiter).toEqual({ mode: 'Spacer', activate, rate_bps: 4e10 });
  });
});

describe.each(PAGES)('$name: the derived limit is shown read-only', (page: ConfigPage) => {
  it('a new form (capacity 100000) shows "1233.6 Mbit" in the Limiter subsection', async () => {
    await page.openNew();
    expect(numberField(body(), 'Buffer capacity')).toHaveValue(100000);
    expect(infoElement()).toHaveTextContent(infoWith('1233.6 Mbit'));
  });

  it.each([
    [fx.config(), '12.336 Mbit'], // capacity 1000
    [fx.config2(), '61.68 Mbit'], // capacity 5000, TBF, limiter active
    [fx.config3(), '3959.856 kbit'], // capacity 321: 3 959 856 bit has 6 decimals in Mbit
  ])('a stored config shows capacity x 12336 bit in readable units (%#)', async (config, text) => {
    expect(config.buffer_capacity * BITS_PER_PACKET).toBeGreaterThan(0); // the value is derived, not stored
    await page.openStored(config);
    expect(infoElement()).toHaveTextContent(infoWith(text));
  });

  it.each([
    ['Spacer', false], ['Spacer', true], ['TokenBucketFilter', false], ['TokenBucketFilter', true],
  ] as const)('is the same for %s with the limiter active=%s', async (mode, activate) => {
    await page.openStored(cfg({}, { mode, activate }));
    expect(infoElement()).toHaveTextContent(infoWith('12.336 Mbit'));
  });

  it.each([
    ['1', '12.336 kbit'],
    ['100', '1233.6 kbit'],
    ['2000', '24.672 Mbit'],
    ['10000', '123.36 Mbit'],
    ['1000000', '12.336 Gbit'],
  ])('follows the buffer capacity while it is edited: %s packets -> %s', async (capacity, text) => {
    const user = await page.openNew();
    await user.clear(numberField(body(), 'Buffer capacity'));
    await user.type(numberField(body(), 'Buffer capacity'), capacity);
    expect(infoElement()).toHaveTextContent(infoWith(text));
  });

  it('shows no number while the capacity is empty or not a number, and never "NaN"', async () => {
    const user = await page.openNew();
    await user.clear(numberField(body(), 'Buffer capacity'));
    expect(infoElement().textContent).not.toMatch(/=\s*\d/);
    expect(body().textContent).not.toMatch(/NaN|Infinity|undefined/);
  });

  it('is information, not a control: no input, no required marker, nothing in the payload', async () => {
    const user = await page.openNew();
    const info = infoElement();
    expect(info.closest('input, select, textarea, button')).toBeNull();
    expect(info.querySelector('input, select, textarea, button')).toBeNull();
    expect(info.querySelector('.required-marker')).toBeNull();
    expect(section('Limiter')).toContainElement(info);
    await page.ready(user);
    await page.save(user);
    expect(JSON.stringify(page.created())).not.toMatch(/limit_bit/);
  });

  it('exists once per form', async () => {
    await page.openNew();
    expect(screen.getAllByText(INFO)).toHaveLength(1);
  });
});

// ---- nothing in the app sources still knows `limit_bit` or the "Burst limit" label ----------------------------

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return name === 'test' ? [] : sourceFiles(p);
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [p] : [];
  });
}

describe('app sources', () => {
  it('no non-test source mentions limit_bit or the Burst limit label', () => {
    const offenders = sourceFiles(join(process.cwd(), 'src'))
      .filter((f) => /limit_bit|burst limit/i.test(readFileSync(f, 'utf8')));
    expect(offenders).toEqual([]);
  });
});

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { screen, within } from '@testing-library/react';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import * as fx from '../test/fixtures';
import { addButton, numberField, removeButton, section } from '../test/helpers';

// v22: the limiter is an optional stage (no "Limiter active" switch, `activate` is gone), so the info line exists only while
// the limiter is added (forms are opened with `openFull()` unless said otherwise).
// Contract: "v21" block of src/test/helpers.tsx (spec20). The limiter's limit is derived by dpds-core from the buffer
// capacity (capacity x 1542 B x 8 bit, the same for Spacer and Token Bucket Filter); `limit_bit` is gone from the API.
// The form has no "Burst limit" field; the Limiter subsection shows the derived limit read-only. Every test runs
// against the link editor (the only page with the config form since spec27).
const body = () => document.body;
const BITS_PER_PACKET = 1542 * 8; // 12336

/** The text of the info line: "Limit: derived from buffer capacity × 1542 B = <value> <unit>". */
const INFO = /Limit: derived from buffer capacity × 1542 B/;
const infoWith = (value: string) =>
  new RegExp(`Limit: derived from buffer capacity × 1542 B = ${value.replace('.', '\\.')}$`);

function infoElement(): HTMLElement {
  return within(section('Limiter')).getByText(INFO);
}

describe.each(PAGES)('$name: no burst limit field', (page: ConfigPage) => {
  it('has neither a "Burst limit" control, nor its unit select, nor any such text', async () => {
    await page.openFull();
    expect(screen.queryByLabelText('Burst limit')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Burst limit unit')).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/burst/i)).not.toBeInTheDocument();
    expect(body().textContent).not.toMatch(/burst limit/i);
  });

  it('the Limiter subsection holds exactly four controls (besides info tooltips): mode, rate, its unit and "Remove limiter"', async () => {
    await page.openFull();
    const controls = Array.from(
      section('Limiter').querySelectorAll('input, select, textarea, button:not([aria-label^="About "])'),
    );
    expect(controls).toHaveLength(4);
    expect(controls.map((c) => c.getAttribute('aria-label') ?? '').filter(Boolean)).not.toContain('Burst limit');
    for (const l of ['Limiter mode', 'Rate', 'Rate unit']) {
      expect(section('Limiter')).toContainElement(within(body()).getByLabelText(l));
    }
    expect(within(section('Limiter')).getByRole('button', { name: 'Remove limiter' })).toBeInTheDocument();
    expect(screen.queryByLabelText('Limiter active')).not.toBeInTheDocument();
  });

  it('sends the limiter as exactly {mode, rate_bps}', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await page.save(user);
    expect(page.created().limiter).toEqual({ mode: 'Spacer', rate_bps: 1e10 });
  });

  it('a stored limiter is sent back without any limit or activate field', async () => {
    const user = await page.openStored(cfg({}, { rate_bps: 4e10 }));
    await page.save(user);
    expect(page.updated().limiter).toEqual({ mode: 'Spacer', rate_bps: 4e10 });
  });
});

describe.each(PAGES)('$name: the derived limit is shown read-only', (page: ConfigPage) => {
  it('a new form (capacity 100000) shows "1233.6 Mbit" in the Limiter subsection', async () => {
    await page.openFull();
    expect(numberField(body(), 'Buffer capacity')).toHaveValue(100000);
    expect(infoElement()).toHaveTextContent(infoWith('1233.6 Mbit'));
  });

  it.each([
    [{ ...fx.config(), limiter: { mode: 'Spacer' as const, rate_bps: 1e9 } }, '12.336 Mbit'], // capacity 1000, with a limiter
    [fx.config2(), '61.68 Mbit'], // capacity 5000, TBF
    [fx.config3(), '3959.856 kbit'], // capacity 321: 3 959 856 bit has 6 decimals in Mbit
  ])('a stored config shows capacity x 12336 bit in readable units (%#)', async (config, text) => {
    expect(config.buffer_capacity * BITS_PER_PACKET).toBeGreaterThan(0); // the value is derived, not stored
    await page.openStored(config);
    expect(infoElement()).toHaveTextContent(infoWith(text));
  });

  it.each(['Spacer', 'TokenBucketFilter'] as const)('is the same for %s', async (mode) => {
    await page.openStored(cfg({}, { mode }));
    expect(infoElement()).toHaveTextContent(infoWith('12.336 Mbit'));
  });

  it.each([
    ['1', '12.336 kbit'],
    ['100', '1233.6 kbit'],
    ['2000', '24.672 Mbit'],
    ['10000', '123.36 Mbit'],
    ['1000000', '12.336 Gbit'],
  ])('follows the buffer capacity while it is edited: %s packets -> %s', async (capacity, text) => {
    const user = await page.openFull();
    await user.clear(numberField(body(), 'Buffer capacity'));
    await user.type(numberField(body(), 'Buffer capacity'), capacity);
    expect(infoElement()).toHaveTextContent(infoWith(text));
  });

  it('shows no number while the capacity is empty or not a number, and never "NaN"', async () => {
    const user = await page.openFull();
    await user.clear(numberField(body(), 'Buffer capacity'));
    expect(infoElement().textContent).not.toMatch(/=\s*\d/);
    expect(body().textContent).not.toMatch(/NaN|Infinity|undefined/);
  });

  it('is information, not a control: no input, no required marker, nothing in the payload', async () => {
    const user = await page.openFull();
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
    await page.openFull();
    expect(screen.getAllByText(INFO)).toHaveLength(1);
  });

  it('v22: exists only while the limiter is added: not on a bare form, gone after "Remove limiter", back after "Add limiter"', async () => {
    const user = await page.openNew();
    expect(screen.queryByText(INFO)).not.toBeInTheDocument();
    await user.click(addButton('limiter'));
    expect(infoElement()).toHaveTextContent(infoWith('1233.6 Mbit'));
    await user.click(removeButton('limiter'));
    expect(screen.queryByText(INFO)).not.toBeInTheDocument();
    await user.click(addButton('limiter'));
    expect(screen.getAllByText(INFO)).toHaveLength(1);
  });

  it('v22: a stored config without a limiter shows no info line', async () => {
    await page.openStored(fx.config());
    expect(screen.queryByText(INFO)).not.toBeInTheDocument();
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
    const offenders = sourceFiles(join(process.cwd(), 'src')).filter((f) =>
      /limit_bit|burst limit/i.test(readFileSync(f, 'utf8')),
    );
    expect(offenders).toEqual([]);
  });
});

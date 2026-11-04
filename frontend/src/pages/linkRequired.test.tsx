import { screen } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import type { Distribution } from '../api/types';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  addButton,
  allRequiredMarkers,
  expectOptional,
  expectRequired,
  numberField,
  queryStage,
  removeButton,
  stageOf,
  typeInto,
} from '../test/helpers';
import { must } from '../test/must';

// Contract: "Required fields are visibly marked" in the v16 block of src/test/helpers.tsx. Marker = `.required-marker`,
// aria-hidden, text "*", in the field's own group; control has `required` + aria-required="true"; one "* required" note.
const body = () => document.body;
const ctl = (label: string) => screen.getByLabelText(label) as HTMLElement;
const isLink = (page: ConfigPage) => page.name === 'link editor';

/** v22: controls required on a new form whose stages are not added yet: no stage field exists, so none is required. */
function bareRequired(page: ConfigPage): string[] {
  return ['Name', ...(isLink(page) ? ['Ingress', 'Egress'] : []), 'Buffer capacity'];
}

/** Controls required once delayer, limiter and dropper are added (Normal distribution, correlation off, Random loss). */
function pristineRequired(page: ConfigPage): string[] {
  return [...bareRequired(page), 'Mean delay', 'Delay std', 'Initialization rate', 'Rate', 'Random loss probability'];
}

/** Controls that are never required. */
function neverRequired(page: ConfigPage): string[] {
  return [
    ...(isLink(page) ? ['Worker lcore'] : ['Description']),
    'Seed',
    'Histogram bins',
    'Histogram lo',
    'Histogram hi',
    'Delay distribution',
    'Limiter mode',
    'Loss model',
    'Reordering',
    'Activate correlation',
    'Delay statistics',
    'Mean delay unit',
    'Delay std unit',
    'Half-life period unit',
    'Initialization rate unit',
    'Rate unit',
    'Histogram lo unit',
    'Histogram hi unit',
    ...(isLink(page) ? ['Apply preset'] : []),
  ];
}

describe.each(PAGES)('$name: required fields are marked', (page: ConfigPage) => {
  it('shows ONE visible note "* required" (not aria-hidden)', async () => {
    await page.openNew();
    const notes = screen.getAllByText('* required');
    expect(notes).toHaveLength(1);
    expect(notes[0].closest('[aria-hidden="true"]')).toBeNull();
  });

  it('v22: a new form with no stage added marks only name, ports and buffer capacity, and has no stage field at all', async () => {
    const user = await page.openNew();
    for (const l of bareRequired(page)) expectRequired(ctl(l));
    expect(allRequiredMarkers()).toHaveLength(bareRequired(page).length);
    for (const l of ['Seed', 'Histogram bins', 'Histogram lo', 'Histogram hi', 'Reordering', 'Delay statistics'])
      expectOptional(ctl(l));
    for (const l of [
      'Mean delay',
      'Delay std',
      'Initialization rate',
      'Rate',
      'Random loss probability',
      'Half-life period',
    ]) {
      expect(screen.queryByLabelText(l)).not.toBeInTheDocument();
    }
    // adding a stage adds exactly its required markers, removing it takes them away again
    await user.click(addButton('delayer'));
    expect(allRequiredMarkers()).toHaveLength(bareRequired(page).length + 3); // mean, std, initialization rate
    await user.click(addButton('limiter'));
    expect(allRequiredMarkers()).toHaveLength(bareRequired(page).length + 4); // + rate
    await user.click(addButton('dropper'));
    expect(allRequiredMarkers()).toHaveLength(pristineRequired(page).length);
    await user.click(removeButton('delayer'));
    expect(allRequiredMarkers()).toHaveLength(pristineRequired(page).length - 3);
    expect(queryStage('delayer')).not.toBeInTheDocument();
  });

  it('marks exactly the required fields of a pristine form (marker, required, aria-required)', async () => {
    await page.openFull();
    for (const l of pristineRequired(page)) expectRequired(ctl(l));
  });

  it('marks nothing else: optional fields, selects with a default, checkboxes and unit selects carry no marker', async () => {
    await page.openFull();
    for (const l of neverRequired(page)) expectOptional(ctl(l));
  });

  it('the half-life is unmarked while its correlation switch is off', async () => {
    await page.openFull();
    expectOptional(ctl('Half-life period'));
  });

  it('counts exactly the right number of markers in the form (no stray markers)', async () => {
    await page.openFull();
    expect(allRequiredMarkers()).toHaveLength(pristineRequired(page).length);
  });

  it('the markers do not change the accessible names (getByRole by name still matches, "*" is not part of it)', async () => {
    await page.openFull();
    expect(screen.getByRole('spinbutton', { name: 'Mean delay' })).toBe(ctl('Mean delay'));
    expect(screen.getByRole('spinbutton', { name: 'Initialization rate' })).toBe(ctl('Initialization rate'));
    expect(screen.getByRole('spinbutton', { name: 'Buffer capacity' })).toBe(ctl('Buffer capacity'));
    expect(screen.getByRole('textbox', { name: 'Name' })).toBe(ctl('Name'));
    if (isLink(page)) {
      expect(screen.getByRole('combobox', { name: 'Ingress' })).toBe(ctl('Ingress'));
      expect(screen.getByRole('combobox', { name: 'Egress' })).toBe(ctl('Egress'));
    }
  });

  it('the required attribute does not replace the in-app messages: saving the empty form still shows an alert', async () => {
    const user = await page.openNew();
    await page.save(user);
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(page.requests()).toBe(0);
  });

  it('the markers stay when the field is filled (they mark the rule, not the state)', async () => {
    const user = await page.openFull();
    await page.ready(user);
    for (const l of pristineRequired(page)) expectRequired(ctl(l));
  });

  it('marks the same fields when editing a stored config', async () => {
    await page.openStored(cfg());
    const expected = pristineRequired(page).filter((l) => l !== 'Ingress' && l !== 'Egress');
    for (const l of expected) expectRequired(ctl(l));
    for (const l of neverRequired(page)) expectOptional(ctl(l));
    if (isLink(page)) for (const l of ['Ingress', 'Egress']) expectRequired(ctl(l));
  });
});

describe.each(PAGES)('$name: conditionally required fields', (page: ConfigPage) => {
  it('half-life is marked only while correlation is on', async () => {
    const user = await page.openFull();
    expectOptional(ctl('Half-life period'));
    await user.click(ctl('Activate correlation'));
    expectRequired(ctl('Half-life period'));
    await user.click(ctl('Activate correlation'));
    expectOptional(ctl('Half-life period'));
  });

  it('v22: the limiter rate is required as soon as the limiter is added (there is no switch); the mode select never', async () => {
    const user = await page.openNew();
    await user.click(addButton('limiter'));
    expectRequired(ctl('Rate'));
    expectOptional(ctl('Limiter mode'));
    expect(screen.queryByLabelText('Limiter active')).not.toBeInTheDocument();
    await user.click(removeButton('limiter'));
    expect(screen.queryByLabelText('Rate')).not.toBeInTheDocument();
  });

  it.each([
    ['NormalDistribution', true],
    ['LogNormalDistribution', true],
    ['GammaDistribution', true],
    ['UniformDistribution', true],
    ['DeterministicDistribution', false],
  ] as [Distribution, boolean][])('with %s the delay std is required: %s (the mean always is)', async (dist, std) => {
    const user = await page.openFull();
    await user.selectOptions(ctl('Delay distribution'), dist);
    if (std) expectRequired(ctl('Delay std'));
    else expectOptional(ctl('Delay std'));
    expectRequired(ctl('Mean delay'));
  });

  it('a stored deterministic config shows the std unmarked', async () => {
    await page.openStored(cfg({ distribution: 'DeterministicDistribution' }));
    expectOptional(ctl('Delay std'));
    expectRequired(ctl('Mean delay'));
  });

  it('Random loss: only its probability is marked; Gilbert-Elliot: p, r, k and h', async () => {
    const user = await page.openFull();
    expectRequired(ctl('Random loss probability'));
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    expect(screen.queryByLabelText('Random loss probability')).not.toBeInTheDocument();
    for (const n of ['p', 'r', 'k', 'h']) expectRequired(ctl(`Gilbert-Elliot ${n}`));
    expectOptional(ctl('Loss model'));
  });

  it('with everything switched on the marker count is the full required set (no stale markers from hidden fields)', async () => {
    const user = await page.openFull();
    await user.click(ctl('Activate correlation'));
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    // pristine set minus the Random probability, plus half-life and the four Gilbert-Elliot parameters
    expect(allRequiredMarkers()).toHaveLength(pristineRequired(page).length - 1 + 1 + 4);
  });

  it('a stored config with switches on and Gilbert-Elliot loss marks the dependent fields', async () => {
    await page.openStored({
      ...cfg(),
      delayer: { ...must(cfg().delayer), activate_correlation: true },
      loss_model: { type: 'GilbertElliot', p: 0.01, r: 0.2, k: 0.9, h: 0.3 },
    });
    for (const l of [
      'Half-life period',
      'Rate',
      'Gilbert-Elliot p',
      'Gilbert-Elliot r',
      'Gilbert-Elliot k',
      'Gilbert-Elliot h',
    ]) {
      expectRequired(ctl(l));
    }
  });
});

describe.each(PAGES)('$name: a field that is not required is never refused for being empty', (page: ConfigPage) => {
  async function ready(user: UserEvent) {
    await page.ready(user);
  }

  it('deterministic distribution with an empty std saves; the std is sent as 0', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.selectOptions(ctl('Delay distribution'), 'DeterministicDistribution');
    await user.clear(numberField(body(), 'Delay std'));
    await page.save(user);
    const c = page.created();
    expect(must(c.delayer).distribution).toBe('DeterministicDistribution');
    expect(must(c.delayer).std_ns).toBe(0);
  });

  it('a non-deterministic distribution with an empty std is refused (alert naming the field, no request)', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.clear(numberField(body(), 'Delay std'));
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Delay std/);
    expect(page.requests()).toBe(0);
  });

  it('an emptied half-life with correlation switched off again saves; it is sent as 0', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.click(ctl('Activate correlation'));
    await user.clear(numberField(body(), 'Half-life period'));
    await user.click(ctl('Activate correlation'));
    await page.save(user);
    const d = must(page.created().delayer);
    expect(d.activate_correlation).toBe(false);
    expect(d.half_life_period_ns).toBe(0);
  });

  it('v22: an emptied rate with the limiter removed again saves; the limiter is sent as null / absent', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.clear(numberField(body(), 'Rate'));
    await user.click(removeButton('limiter'));
    await page.save(user);
    expect(stageOf(page.created(), 'limiter')).toBeNull();
  });

  it('v22: an emptied rate while the limiter is added is refused (alert naming Rate, no write)', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.clear(numberField(body(), 'Rate'));
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/\bRate\b/);
    expect(page.requests()).toBe(0);
  });

  it('v22: a removed delayer needs no initialization rate and is sent as null / absent', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.click(removeButton('delayer'));
    await page.save(user);
    expect(stageOf(page.created(), 'delayer')).toBeNull();
  });

  it('an emptied half-life with correlation ON is refused (it is marked required)', async () => {
    const user = await page.openFull();
    await ready(user);
    await user.click(ctl('Activate correlation'));
    await user.clear(numberField(body(), 'Half-life period'));
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Half-life period/);
    expect(page.requests()).toBe(0);
  });

  it('typing into a required field then saving still sends it (marking changes nothing about values)', async () => {
    const user = await page.openFull();
    await ready(user);
    await typeInto(user, body(), 'Mean delay', '2', 'ms');
    await page.save(user);
    expect(must(page.created().delayer).mean_ns).toBe(2000000);
  });
});

describe('link editor: name, ingress and egress are required before saving', () => {
  const link = PAGES[0];

  it('saving without a name, then without ports, is refused (the marked fields are the ones the form insists on)', async () => {
    const user = await link.openFull();
    await typeInto(user, body(), 'Initialization rate', '1', 'Mpps');
    await link.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/name/i);
    expect(link.requests()).toBe(0);
  });
});

describe('preset editor: name is required, description is not', () => {
  const preset = PAGES[1];

  it('saves without a description', async () => {
    const user = await preset.openFull();
    await preset.ready(user);
    expectOptional(ctl('Description'));
    await preset.save(user);
    expect(preset.requests()).toBe(1);
  });
});

import { cleanup, screen, within } from '@testing-library/react';
import type { ConfigDto } from '../api/types';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  accuracyOf,
  addButton,
  addStages,
  numberField,
  pickAccuracy,
  queryAddButton,
  queryRemoveButton,
  queryStage,
  removeButton,
  sameConfig,
  section,
  SECTIONS,
  STAGE_TITLE,
  stageOf,
  STAGES,
  typeInto,
  unitSelect,
} from '../test/helpers';
import type { Stage } from '../test/helpers';
import * as fx from '../test/fixtures';
import { must } from '../test/must';

// Contract: "Subsections" and "Enable / disable" in the v16 block and "Optional stages" in the v22 block of
// src/test/helpers.tsx. Each test runs against the link editor, the only page with the config form since spec27.
// Tests of the five-section layout use `openFull()` (all three stages added); `openNew()` is the bare form.
const body = () => document.body;
const outer = (page: ConfigPage) => screen.getByRole('group', { name: page.outerName });
const ctl = (label: string) => screen.getByLabelText(label) as HTMLInputElement;

// Labels (controls) that live in each subsection. The unit selects are "<label> unit".
const CONTENT: Record<string, string[]> = {
  Buffer: ['Buffer capacity', 'Seed'],
  Delayer: [
    'Delay distribution',
    'Mean delay',
    'Mean delay unit',
    'Delay std',
    'Delay std unit',
    'Half-life period',
    'Half-life period unit',
    'Initialization rate',
    'Initialization rate unit',
  ],
  Limiter: ['Limiter mode', 'Rate', 'Rate unit'],
  Dropper: ['Loss model', 'Random loss probability'],
  Statistics: [
    'Delay statistics',
    'Histogram bins',
    'Histogram lo',
    'Histogram lo unit',
    'Histogram hi',
    'Histogram hi unit',
  ],
};

describe.each(PAGES)('$name: subsections', (page: ConfigPage) => {
  it('with all stages added: keeps the outer fieldset and nests exactly five fieldsets, legends Buffer, Delayer, Limiter, Dropper, Statistics in order', async () => {
    await page.openFull();
    const nested = Array.from(outer(page).querySelectorAll('fieldset'));
    expect(nested.map((f) => f.querySelector(':scope > legend')?.textContent?.trim())).toEqual(SECTIONS);
    for (const f of nested) {
      expect(f.tagName).toBe('FIELDSET');
      expect(f.firstElementChild?.tagName).toBe('LEGEND');
      expect(
        within(outer(page)).getAllByRole('group', { name: f.querySelector('legend')?.textContent ?? '' }),
      ).toContain(f);
    }
    // the five are direct subsections, not nested into each other
    for (const f of nested) expect(f.parentElement?.closest('fieldset')).toBe(outer(page));
  });

  it('the subsections follow each other in the contracted order in the document', async () => {
    await page.openFull();
    for (let i = 0; i + 1 < SECTIONS.length; i += 1) {
      const a = section(SECTIONS[i]);
      const b = section(SECTIONS[i + 1]);
      expect(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(a.contains(b)).toBe(false);
    }
  });

  it.each(Object.entries(CONTENT))('%s holds its fields and no field of another subsection', async (name, labels) => {
    await page.openFull();
    const own = section(name);
    for (const l of labels) expect(within(own).getByLabelText(l)).toBeInTheDocument();
    for (const [other, otherLabels] of Object.entries(CONTENT)) {
      if (other === name) continue;
      for (const l of otherLabels) expect(within(own).queryByLabelText(l)).not.toBeInTheDocument();
    }
  });

  it('the Dropper holds the Gilbert-Elliot parameters once that model is chosen, and only them', async () => {
    const user = await page.openFull();
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    const d = section('Dropper');
    for (const l of ['Gilbert-Elliot p', 'Gilbert-Elliot r', 'Gilbert-Elliot k', 'Gilbert-Elliot h']) {
      expect(within(d).getByLabelText(l)).toBeInTheDocument();
    }
    expect(within(d).queryByLabelText('Random loss probability')).not.toBeInTheDocument();
    expect(within(d).getByLabelText('Loss model')).toBeInTheDocument();
  });

  it.each(STAGES)('the %s subsection holds a "Remove %s" button, and no other subsection does', async (stage) => {
    await page.openFull();
    expect(within(section(STAGE_TITLE[stage])).getByRole('button', { name: `Remove ${stage}` })).toBeInTheDocument();
    for (const other of SECTIONS.filter((n) => n !== STAGE_TITLE[stage])) {
      expect(within(section(other)).queryByRole('button', { name: `Remove ${stage}` })).not.toBeInTheDocument();
    }
  });

  it('every control of the config form lies in a subsection (spec27: no loose Apply preset / Save as preset any more)', async () => {
    await page.openFull();
    const subs = SECTIONS.map(section);
    const loose = Array.from(
      outer(page).querySelectorAll<HTMLElement>('input, select, textarea, button:not([aria-label^="About "])'),
    ).filter((c) => !subs.some((s) => s.contains(c)));
    const names = loose.map((c) =>
      (c.tagName === 'BUTTON' ? c.textContent : (c as HTMLInputElement).labels?.[0]?.textContent)?.trim(),
    );
    expect(names.sort()).toEqual([]);
  });

  it('v22: on a bare form the only loose controls are the three "Add ..." buttons', async () => {
    await page.openNew();
    const subs = ['Buffer', 'Statistics'].map(section);
    const loose = Array.from(
      outer(page).querySelectorAll<HTMLElement>('input, select, textarea, button:not([aria-label^="About "])'),
    ).filter((c) => !subs.some((x) => x.contains(c)));
    const names = loose.map((c) =>
      (c.tagName === 'BUTTON' ? c.textContent : (c as HTMLInputElement).labels?.[0]?.textContent)?.trim(),
    );
    expect(names.sort()).toEqual(['Add delayer', 'Add dropper', 'Add limiter']);
  });

  it('the same structure shows when editing a stored config', async () => {
    await page.openStored(fx.config2());
    // nested fieldsets only: the outer one may be named by a legend or by aria-labelledby
    const nested = Array.from(outer(page).querySelectorAll('fieldset'));
    expect(nested.map((f) => f.querySelector(':scope > legend')?.textContent?.trim())).toEqual(SECTIONS);
    for (const l of CONTENT.Delayer) expect(within(section('Delayer')).getByLabelText(l)).toBeInTheDocument();
    for (const l of ['Gilbert-Elliot p', 'Gilbert-Elliot h'])
      expect(within(section('Dropper')).getByLabelText(l)).toBeInTheDocument();
  });
});

describe.each(PAGES)('$name: v22 optional stages', (page: ConfigPage) => {
  const outerEl = () => screen.getByRole('group', { name: page.outerName });
  const titles = () =>
    Array.from(outerEl().querySelectorAll('fieldset')).map((f) =>
      f.querySelector(':scope > legend')?.textContent?.trim(),
    );
  const FIELD: Record<Stage, string> = { delayer: 'Mean delay', limiter: 'Rate', dropper: 'Loss model' };

  it('a new form shows Buffer and Statistics and one "Add ..." button per stage; no stage group, no stage field', async () => {
    await page.openNew();
    expect(titles()).toEqual(['Buffer', 'Statistics']);
    for (const s of STAGES) {
      expect(addButton(s)).toBeInTheDocument();
      expect(addButton(s)).toBeEnabled();
      expect(queryStage(s)).not.toBeInTheDocument();
      expect(queryRemoveButton(s)).not.toBeInTheDocument();
      expect(screen.queryByLabelText(FIELD[s])).not.toBeInTheDocument();
    }
    expect(screen.queryByLabelText('Limiter active')).not.toBeInTheDocument();
  });

  it('the "Add ..." buttons sit between Buffer and Statistics in the order delayer, limiter, dropper, inside the outer fieldset', async () => {
    await page.openNew();
    const order = ['Buffer', 'Add delayer', 'Add limiter', 'Add dropper', 'Statistics'];
    const els = [
      section('Buffer'),
      addButton('delayer'),
      addButton('limiter'),
      addButton('dropper'),
      section('Statistics'),
    ];
    for (let i = 0; i + 1 < els.length; i += 1) {
      expect(
        els[i].compareDocumentPosition(els[i + 1]) & Node.DOCUMENT_POSITION_FOLLOWING,
        `${order[i]} before ${order[i + 1]}`,
      ).toBeTruthy();
    }
    for (const b of els.slice(1, 4)) expect(outerEl()).toContainElement(b);
  });

  it.each(STAGES)(
    '"Add %s" shows exactly that subsection with its fields and a "Remove %s" button; the Add button is gone',
    async (stage) => {
      const user = await page.openNew();
      await user.click(addButton(stage));
      const sec = section(STAGE_TITLE[stage]);
      expect(within(sec).getByLabelText(FIELD[stage])).toBeInTheDocument();
      expect(within(sec).getByRole('button', { name: `Remove ${stage}` })).toBeInTheDocument();
      expect(queryAddButton(stage)).not.toBeInTheDocument();
      for (const other of STAGES.filter((x) => x !== stage)) {
        expect(queryStage(other)).not.toBeInTheDocument();
        expect(addButton(other)).toBeInTheDocument();
      }
    },
  );

  it('clicking "Add ..." does not submit the form: nothing is written and no alert appears', async () => {
    const user = await page.openNew();
    await addStages(user);
    expect(page.requests()).toBe(0);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it.each([[['dropper', 'delayer', 'limiter']], [['limiter', 'dropper', 'delayer']], [['limiter', 'delayer']]] as [
    Stage[],
  ][])(
    'the subsections keep the order Buffer, Delayer, Limiter, Dropper, Statistics whatever the clicking order (%j)',
    async (clicks) => {
      const user = await page.openNew();
      await addStages(user, clicks);
      const expected = SECTIONS.filter(
        (n) => ['Buffer', 'Statistics'].includes(n) || clicks.some((c) => STAGE_TITLE[c] === n),
      );
      expect(titles()).toEqual(expected);
    },
  );

  it.each(STAGES)('"Remove %s" collapses the subsection and brings the Add button back', async (stage) => {
    const user = await page.openFull();
    await user.click(removeButton(stage));
    expect(queryStage(stage)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(FIELD[stage])).not.toBeInTheDocument();
    expect(addButton(stage)).toBeInTheDocument();
    for (const other of STAGES.filter((x) => x !== stage)) expect(queryStage(other)).toBeInTheDocument();
  });

  it('removing and re-adding within the form keeps what was typed (delayer, limiter and dropper)', async () => {
    const user = await page.openFull();
    await typeInto(user, document.body, 'Mean delay', '7', 'ms');
    await pickAccuracy(user, 'Correlation');
    await typeInto(user, document.body, 'Half-life period', '3', 'ms');
    await typeInto(user, document.body, 'Initialization rate', '2', 'Mpps');
    await user.selectOptions(screen.getByLabelText('Limiter mode'), 'TokenBucketFilter');
    await typeInto(user, document.body, 'Rate', '2.5', 'Gbit/s');
    await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
    await typeInto(user, document.body, 'Gilbert-Elliot p', '0.02');
    for (const s of STAGES) await user.click(removeButton(s));
    for (const s of STAGES) await user.click(addButton(s));
    expect(numberField(document.body, 'Mean delay')).toHaveValue(7);
    expect(accuracyOf()).toBe('Correlation');
    expect(numberField(document.body, 'Half-life period')).toHaveValue(3);
    expect(numberField(document.body, 'Initialization rate')).toHaveValue(2);
    expect(screen.getByLabelText('Limiter mode')).toHaveValue('TokenBucketFilter');
    expect(numberField(document.body, 'Rate')).toHaveValue(2.5);
    expect(screen.getByLabelText('Loss model')).toHaveValue('GilbertElliot');
    expect(numberField(document.body, 'Gilbert-Elliot p')).toHaveValue(0.02);
  });

  it('a newly added stage shows the v16 defaults: delayer mean 1 ms / std 100 us / correlation off / rate empty, limiter Spacer 10 Gbit/s, dropper Random p=0', async () => {
    const user = await page.openNew();
    await addStages(user);
    expect(numberField(document.body, 'Mean delay')).toHaveValue(1);
    expect(screen.getByLabelText('Delay distribution')).toHaveValue('NormalDistribution');
    expect(accuracyOf()).toBe('Basic');
    expect(numberField(document.body, 'Initialization rate')).toHaveValue(null);
    expect(screen.getByLabelText('Limiter mode')).toHaveValue('Spacer');
    expect(numberField(document.body, 'Rate')).toHaveValue(10);
    expect(screen.getByLabelText('Loss model')).toHaveValue('Random');
    expect(numberField(document.body, 'Random loss probability')).toHaveValue(0);
  });

  it('saving a bare form sends no stage (null or absent) and keeps Buffer and Statistics', async () => {
    const user = await page.openNew();
    await page.ready(user);
    await page.save(user);
    const c = page.created();
    for (const k of ['delayer', 'limiter', 'loss_model'] as const) expect(stageOf(c, k)).toBeNull();
    expect(c.buffer_capacity).toBe(100000);
    expect(c.delay_stats).toBe(true);
  });

  it.each([[['limiter']], [['delayer']], [['dropper']], [['delayer', 'dropper']]] as [Stage[]][])(
    'saving with only %j added sends exactly those stages',
    async (added) => {
      const user = await page.openNew();
      await addStages(user, added);
      await page.ready(user);
      await page.save(user);
      const c = page.created();
      expect(stageOf(c, 'delayer') !== null).toBe(added.includes('delayer'));
      expect(stageOf(c, 'limiter') !== null).toBe(added.includes('limiter'));
      expect(stageOf(c, 'loss_model') !== null).toBe(added.includes('dropper'));
    },
  );

  it('a stage removed after typing is sent as null / absent, not with its typed values', async () => {
    const user = await page.openFull();
    await typeInto(user, document.body, 'Rate', '2', 'Gbit/s');
    await page.ready(user);
    await user.click(removeButton('limiter'));
    await user.click(removeButton('dropper'));
    await page.save(user);
    const c = page.created();
    expect(stageOf(c, 'limiter')).toBeNull();
    expect(stageOf(c, 'loss_model')).toBeNull();
    expect(stageOf(c, 'delayer')).not.toBeNull();
  });

  it('the shapes of the stages: limiter {mode, rate_bps}, dropper {type, p}, delayer with its six fields', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await page.save(user);
    const c = page.created();
    expect(c.limiter).toEqual({ mode: 'Spacer', rate_bps: 1e10 });
    expect(c.loss_model).toEqual({ type: 'Random', p: 0 });
    expect(Object.keys(must(c.delayer)).sort()).toEqual([
      'activate_correlation',
      'distribution',
      'half_life_period_ns',
      'initialization_rate_pps',
      'mean_ns',
      'std_ns',
    ]);
  });

  it.each([
    ['everything', fx.config2(), ['delayer', 'limiter', 'dropper']],
    ['nothing', fx.configBare(), []],
    ['a delayer and a loss model but no limiter', fx.config(), ['delayer', 'dropper']], // fx.config(): no limiter, Random p=0 loss
    ['only the limiter', { ...fx.configBare(), limiter: { mode: 'Spacer' as const, rate_bps: 2e9 } }, ['limiter']],
    ['a limiter that is explicitly absent', { ...fx.config2(), limiter: undefined }, ['delayer', 'dropper']],
  ] as [string, ConfigDto, Stage[]][])(
    'a stored config with %s opens with exactly its present stages added',
    async (_n, config, present) => {
      await page.openStored(config);
      for (const s of STAGES) {
        if (present.includes(s)) {
          expect(queryStage(s)).toBeInTheDocument();
          expect(queryAddButton(s)).not.toBeInTheDocument();
        } else {
          expect(queryStage(s)).not.toBeInTheDocument();
          expect(addButton(s)).toBeInTheDocument();
        }
      }
    },
  );

  it('saving a stored config unchanged keeps its stages and values (absent stages stay absent)', async () => {
    for (const config of [fx.configBare(), fx.config2(), { ...fx.config3(), delayer: null, reordering: false }]) {
      // without a Delayer the delay accuracy is Basic, so reordering is saved as false (spec 45 F1)
      const user = await page.openStored(config);
      await page.save(user);
      sameConfig(page.updated(), config);
      cleanup();
      localStorage.clear();
    }
  });
});

describe('link editor: what stays outside the subsections', () => {
  const link = PAGES[0];

  it('has no Apply preset and no Save as preset (presets are gone, spec27)', async () => {
    await link.openFull();
    expect(screen.queryByLabelText('Apply preset')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save as preset' })).not.toBeInTheDocument();
    expect(screen.queryByText(/preset/i)).not.toBeInTheDocument();
  });

  it('Name, Ingress and Egress are outside the config fieldset', async () => {
    await link.openFull();
    for (const l of ['Name', 'Ingress', 'Egress']) {
      expect(outer(link)).not.toContainElement(ctl(l));
    }
  });
});

describe.each(PAGES)(
  '$name: switch dependent fields are disabled, not hidden, and keep their values',
  (page: ConfigPage) => {
    const halfLife = () => ctl('Half-life period');
    const limiterFields = ['Limiter mode', 'Rate', 'Rate unit'];

    it('half-life and its unit are disabled while "Delay accuracy" is not Correlation and enabled while it is', async () => {
      const user = await page.openFull();
      expect(accuracyOf()).toBe('Basic');
      expect(halfLife()).toBeInTheDocument();
      expect(halfLife()).toBeDisabled();
      expect(unitSelect(body(), 'Half-life period')).toBeDisabled();
      await pickAccuracy(user, 'Correlation');
      expect(halfLife()).toBeEnabled();
      expect(unitSelect(body(), 'Half-life period')).toBeEnabled();
      await pickAccuracy(user, 'Basic');
      expect(halfLife()).toBeDisabled();
      expect(unitSelect(body(), 'Half-life period')).toBeDisabled();
    });

    it('the initialization rate and its unit are enabled whatever the correlation switch says', async () => {
      const user = await page.openFull();
      expect(ctl('Initialization rate')).toBeEnabled();
      expect(unitSelect(body(), 'Initialization rate')).toBeEnabled();
      await pickAccuracy(user, 'Correlation');
      expect(ctl('Initialization rate')).toBeEnabled();
      expect(unitSelect(body(), 'Initialization rate')).toBeEnabled();
    });

    it('v22: there is no "Limiter active" switch; mode, rate and its unit are enabled as soon as the limiter is added', async () => {
      await page.openFull();
      expect(screen.queryByLabelText('Limiter active')).not.toBeInTheDocument();
      for (const l of limiterFields) expect(ctl(l)).toBeEnabled();
    });

    it('a typed half-life survives switching correlation off and on and is still sent while it is off', async () => {
      const user = await page.openFull();
      await pickAccuracy(user, 'Correlation');
      await typeInto(user, body(), 'Half-life period', '5', 'ms');
      await pickAccuracy(user, 'Basic');
      expect(halfLife()).toHaveValue(5);
      await pickAccuracy(user, 'Correlation');
      expect(halfLife()).toHaveValue(5);
      await pickAccuracy(user, 'Basic');
      await page.ready(user);
      await page.save(user);
      const d = must(page.created().delayer);
      expect(d.activate_correlation).toBe(false);
      expect(d.half_life_period_ns).toBe(5000000);
    });

    it('typed limiter values survive removing and re-adding the limiter and are sent', async () => {
      const user = await page.openFull();
      await user.selectOptions(ctl('Limiter mode'), 'TokenBucketFilter');
      await typeInto(user, body(), 'Rate', '2.5', 'Gbit/s');
      await user.click(removeButton('limiter'));
      await user.click(addButton('limiter'));
      expect(ctl('Rate')).toHaveValue(2.5);
      expect(ctl('Limiter mode')).toHaveValue('TokenBucketFilter');
      await page.ready(user);
      await page.save(user);
      expect(page.created().limiter).toEqual({ mode: 'TokenBucketFilter', rate_bps: 2500000000 });
    });

    it('with the switches on, a stored config has every dependent field enabled', async () => {
      await page.openStored(fx.config2());
      expect(halfLife()).toBeEnabled();
      for (const l of limiterFields) expect(ctl(l)).toBeEnabled();
    });

    it('with the correlation switch off, a stored config shows its half-life in a disabled field; the limiter rate is enabled', async () => {
      await page.openStored(
        cfg({ activate_correlation: false, half_life_period_ns: 4000000 }, { rate_bps: 5000000000 }),
      );
      expect(halfLife()).toBeDisabled();
      expect(halfLife()).toHaveValue(4);
      expect(ctl('Rate')).toBeEnabled();
      expect(ctl('Rate')).toHaveValue(5);
    });

    it('everything else stays enabled whatever the switches say', async () => {
      await page.openFull();
      for (const l of [
        'Delay distribution',
        'Mean delay',
        'Mean delay unit',
        'Delay std',
        'Delay std unit',
        'Loss model',
        'Random loss probability',
        'Buffer capacity',
        'Seed',
        'Delay statistics',
        'Histogram bins',
        'Histogram lo',
        'Histogram lo unit',
        'Histogram hi',
        'Histogram hi unit',
      ]) {
        expect(ctl(l)).toBeEnabled();
      }
    });

    it('the histogram fields are not switch dependent: enabled while "Delay statistics" is off', async () => {
      const user = await page.openFull();
      await user.click(ctl('Delay statistics'));
      expect(ctl('Delay statistics')).not.toBeChecked();
      for (const l of ['Histogram bins', 'Histogram lo', 'Histogram lo unit', 'Histogram hi', 'Histogram hi unit']) {
        expect(ctl(l)).toBeEnabled();
      }
    });

    it('typed loss parameters survive switching the loss model away and back', async () => {
      const user = await page.openFull();
      await user.clear(ctl('Random loss probability'));
      await user.type(ctl('Random loss probability'), '0.05');
      await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
      await user.clear(ctl('Gilbert-Elliot p'));
      await user.type(ctl('Gilbert-Elliot p'), '0.02');
      await user.selectOptions(ctl('Loss model'), 'Random');
      expect(ctl('Random loss probability')).toHaveValue(0.05);
      await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
      expect(ctl('Gilbert-Elliot p')).toHaveValue(0.02);
    });

    it('only the loss model that is selected is rendered (the alternatives are hidden, not disabled)', async () => {
      const user = await page.openFull();
      expect(screen.queryByLabelText('Gilbert-Elliot p')).not.toBeInTheDocument();
      await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
      expect(screen.queryByLabelText('Random loss probability')).not.toBeInTheDocument();
      expect(numberField(body(), 'Gilbert-Elliot r')).toBeEnabled();
    });
  },
);

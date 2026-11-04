import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import { linkPage, presetPage } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import { allRequiredMarkers, rawStore, renderApp, typeInto } from '../test/helpers';

// Spec 25 (user 2026-11-04): an info-icon tooltip after EVERY field label in the link editor, the preset editor (the shared
// ConfigFieldset) and the Settings page. Mechanics (spec): react-bootstrap OverlayTrigger + Tooltip, hover or keyboard focus;
// the trigger is a `<button type="button">` with the Bootstrap icon `bi-info-circle` and the accessible name "About <label>",
// placed after the label (and after a required marker); it must not change the field's accessible name and must not count as a
// required marker. The prose is not pinned except for the few facts that are certain from the backend: the base units.
// Tester's choice: "EAL extra arguments" (a fieldset legend, not a <label>) also gets "About EAL extra arguments", and the
// per-row "EAL argument N" inputs get none.

const MIN_TEXT = 10;

const aboutButton = (label: string) => screen.getByRole('button', { name: `About ${label}` });
const queryAbout = (label: string) => screen.queryByRole('button', { name: `About ${label}` });

/** Every field label of the page: all <label> elements except the visually hidden "<label> unit" ones. */
function fieldLabels(): HTMLLabelElement[] {
  return Array.from(document.querySelectorAll('label')).filter((l) => {
    const t = l.textContent?.trim() ?? '';
    return t !== '' && !t.endsWith(' unit');
  });
}
const textOf = (l: HTMLElement) => l.textContent?.trim() ?? '';

const tooltipVisible = () => screen.findByRole('tooltip');
const noTooltip = () => waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());

/** Hovers the button of `label`, returns the tooltip text, and moves away again. */
async function hoverText(user: UserEvent, label: string): Promise<string> {
  const btn = aboutButton(label);
  await user.hover(btn);
  const tip = await tooltipVisible();
  const text = tip.textContent?.trim() ?? '';
  await user.unhover(btn);
  await noTooltip();
  return text;
}

/** Focuses the button of `label` (keyboard focus), returns the tooltip text, and blurs it again. */
async function focusText(label: string): Promise<string> {
  const btn = aboutButton(label);
  act(() => btn.focus());
  const tip = await tooltipVisible();
  const text = tip.textContent?.trim() ?? '';
  act(() => btn.blur());
  await noTooltip();
  return text;
}

const LINK_LABELS = [
  'Name',
  'Worker lcore',
  'Ingress',
  'Egress',
  'Apply preset',
  'Buffer capacity',
  'Reordering',
  'Seed',
  'Average frame size',
  'Delay distribution',
  'Mean delay',
  'Delay std',
  'Activate correlation',
  'Half-life period',
  'Initialization rate',
  'Limiter mode',
  'Rate',
  'Loss model',
  'Random loss probability',
  'Delay statistics',
  'Histogram bins',
  'Histogram lo',
  'Histogram hi',
];
const GE_LABELS = ['Gilbert-Elliot p', 'Gilbert-Elliot r', 'Gilbert-Elliot k', 'Gilbert-Elliot h'];
const PRESET_ONLY_LABELS = ['Description'];
const LINK_ONLY_LABELS = ['Worker lcore', 'Ingress', 'Egress', 'Apply preset'];

const labelsOf = (page: ConfigPage) =>
  page === linkPage
    ? LINK_LABELS
    : [...LINK_LABELS.filter((l) => !LINK_ONLY_LABELS.includes(l)), ...PRESET_ONLY_LABELS];

describe.each([linkPage, presetPage])('$name: tooltips after every field label', (page: ConfigPage) => {
  it('every field has a button "About <label>" (all stages added)', async () => {
    await page.openFull();
    for (const label of labelsOf(page)) expect(aboutButton(label)).toBeInTheDocument();
  });

  it('the Gilbert-Elliot fields have one too', async () => {
    const user = await page.openFull();
    await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
    for (const label of GE_LABELS) expect(aboutButton(label)).toBeInTheDocument();
    expect(queryAbout('Random loss probability')).not.toBeInTheDocument();
  });

  it('no stage fields, no stage tooltips while a stage is not added', async () => {
    await page.openNew();
    for (const label of ['Mean delay', 'Initialization rate', 'Rate', 'Random loss probability']) {
      expect(queryAbout(label)).not.toBeInTheDocument();
    }
    expect(aboutButton('Buffer capacity')).toBeInTheDocument();
    expect(aboutButton('Average frame size')).toBeInTheDocument();
  });

  it('EVERY <label> of the form has exactly one button right after it, outside the label', async () => {
    const user = await page.openFull();
    await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
    const labels = fieldLabels();
    expect(labels.length).toBeGreaterThanOrEqual(labelsOf(page).length - 1 + GE_LABELS.length);
    for (const l of labels) {
      const name = `About ${textOf(l)}`;
      const buttons = screen.getAllByRole('button', { name });
      expect(buttons, name).toHaveLength(1);
      const [btn] = buttons;
      expect(l.contains(btn), `${name} must not be inside the label`).toBe(false);
      expect(
        l.compareDocumentPosition(btn) & Node.DOCUMENT_POSITION_FOLLOWING,
        `${name} follows the label`,
      ).toBeTruthy();
      expect(btn).toHaveAttribute('type', 'button');
      expect(btn.querySelector('.bi-info-circle')).not.toBeNull();
    }
  });

  it('the button comes after the required marker of a required field', async () => {
    await page.openNew();
    const label = screen.getByText('Buffer capacity', { selector: 'label' });
    const marker = label.parentElement?.querySelector('.required-marker');
    expect(marker).not.toBeNull();
    const btn = aboutButton('Buffer capacity');
    expect((marker?.compareDocumentPosition(btn) ?? 0) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('labels keep their accessible names: every input is still found by its plain label', async () => {
    await page.openFull();
    for (const label of [
      'Buffer capacity',
      'Seed',
      'Average frame size',
      'Mean delay',
      'Initialization rate',
      'Rate',
      'Histogram lo',
      'Delay distribution',
      'Reordering',
    ]) {
      const el = screen.getByLabelText(label);
      expect(el.tagName).not.toBe('BUTTON');
    }
    // the unit selects keep theirs too
    expect(screen.getByLabelText('Initialization rate unit').tagName).toBe('SELECT');
    expect(screen.getByLabelText('Rate unit').tagName).toBe('SELECT');
  });

  it('the required-marker count is unchanged (the icon is no marker)', async () => {
    await page.openNew();
    const bare = page === linkPage ? 4 : 2; // name, [ingress, egress], buffer capacity
    expect(allRequiredMarkers()).toHaveLength(bare);
    const user = userEvent.setup();
    for (const s of ['delayer', 'limiter', 'dropper'])
      await user.click(screen.getByRole('button', { name: `Add ${s}` }));
    expect(allRequiredMarkers()).toHaveLength(bare + 5); // mean, std, initialization rate, rate, random p
    for (const b of screen.getAllByRole('button', { name: /^About / })) {
      expect(b.classList.contains('required-marker')).toBe(false);
      expect(b.querySelector('.required-marker')).toBeNull();
    }
  });

  it('no button sits inside a <legend> (a legend names its group)', async () => {
    await page.openFull();
    expect(document.querySelectorAll('legend button')).toHaveLength(0);
  });

  it('hovering a button shows a tooltip with text; moving away hides it', async () => {
    const user = await page.openFull();
    const text = await hoverText(user, 'Mean delay');
    expect(text.length).toBeGreaterThanOrEqual(MIN_TEXT);
  });

  it('keyboard focus shows it too, blur hides it', async () => {
    await page.openFull();
    const text = await focusText('Buffer capacity');
    expect(text.length).toBeGreaterThanOrEqual(MIN_TEXT);
  });

  it('every label of the form has a tooltip with real text', async () => {
    const user = await page.openFull();
    await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
    for (const l of fieldLabels()) {
      const text = await hoverText(user, textOf(l));
      expect(text.length, `tooltip of ${textOf(l)}`).toBeGreaterThanOrEqual(MIN_TEXT);
    }
  });

  it('Initialization rate names its base unit (pps)', async () => {
    const user = await page.openFull();
    expect(await hoverText(user, 'Initialization rate')).toMatch(/pps|packets\/s|packets per second/i);
  });

  it('Rate names its base unit (bit/s)', async () => {
    const user = await page.openFull();
    expect(await hoverText(user, 'Rate')).toMatch(/bit\/s/i);
  });

  it('Average frame size mentions the 20 B L1 overhead', async () => {
    const user = await page.openNew();
    expect(await hoverText(user, 'Average frame size')).toMatch(/20\s*B/);
  });

  it('clicking a button neither submits nor navigates nor writes the form', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await user.click(aboutButton('Buffer capacity'));
    await user.click(aboutButton('Initialization rate'));
    expect(page.requests()).toBe(0);
    expect(rawStore()).toBeNull();
    expect(
      screen.getByRole('heading', { level: 1, name: page === linkPage ? 'New link' : 'New preset' }),
    ).toBeInTheDocument();
  });

  it('typing into a field still works next to its button', async () => {
    const user = await page.openFull();
    await typeInto(user, document.body, 'Seed', '42');
    expect(screen.getByLabelText('Seed')).toHaveValue(42);
  });
});

describe('Settings: tooltips', () => {
  const open = async () => {
    const user = userEvent.setup();
    renderApp('/settings');
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    return user;
  };

  it.each(['Main lcore', 'Poll interval (ms)', 'Import setup file', 'EAL extra arguments'])(
    'has a button "About %s"',
    async (label) => {
      await open();
      expect(aboutButton(label)).toBeInTheDocument();
    },
  );

  it('EVERY <label> of the page has exactly one button right after it, outside the label', async () => {
    await open();
    const labels = fieldLabels();
    expect(labels.length).toBeGreaterThanOrEqual(3);
    for (const l of labels) {
      const name = `About ${textOf(l)}`;
      const buttons = screen.getAllByRole('button', { name });
      expect(buttons, name).toHaveLength(1);
      expect(l.contains(buttons[0])).toBe(false);
      expect(l.compareDocumentPosition(buttons[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(buttons[0]).toHaveAttribute('type', 'button');
    }
  });

  it('every tooltip has real text', async () => {
    const user = await open();
    for (const label of [...fieldLabels().map(textOf), 'EAL extra arguments']) {
      const text = await hoverText(user, label);
      expect(text.length, `tooltip of ${label}`).toBeGreaterThanOrEqual(MIN_TEXT);
    }
  });

  it('keyboard focus shows a tooltip too', async () => {
    await open();
    expect((await focusText('Poll interval (ms)')).length).toBeGreaterThanOrEqual(MIN_TEXT);
  });

  it('the inputs keep their plain labels', async () => {
    await open();
    expect(screen.getByLabelText('Main lcore').tagName).toBe('INPUT');
    expect(screen.getByLabelText('Poll interval (ms)').tagName).toBe('INPUT');
    expect(screen.getByLabelText('Import setup file').tagName).toBe('INPUT');
  });

  it('the Poll interval tooltip names milliseconds', async () => {
    const user = await open();
    expect(await hoverText(user, 'Poll interval (ms)')).toMatch(/ms|millisecond/i);
  });

  // Review follow-up: the button sat INSIDE the <legend>, so a browser computes the group name from the legend content
  // ("EAL extra arguments About EAL extra arguments"). jsdom's name computation ignores that button, so the exact name
  // alone cannot go red here; the structural checks below pin the cause: no button inside a legend or inside whatever
  // names the group.
  it('the group is named exactly "EAL extra arguments"', async () => {
    await open();
    expect(screen.getByRole('group', { name: 'EAL extra arguments' })).toBeInTheDocument();
  });

  it('the "About EAL extra arguments" button exists but is not part of the group name', async () => {
    await open();
    const group = screen.getByRole('group', { name: 'EAL extra arguments' });
    const btn = aboutButton('EAL extra arguments');
    const legend = group.querySelector(':scope > legend');
    if (legend) expect(legend.contains(btn)).toBe(false);
    const ids = (group.getAttribute('aria-labelledby') ?? '').split(/\s+/).filter(Boolean);
    for (const id of ids) expect(document.getElementById(id)?.contains(btn)).toBe(false);
    // the button is still next to the heading of the list: inside the same form card, after the group's name
    expect(btn.closest('form')).not.toBeNull();
  });

  it('no button sits inside a <legend> on any page of the spec (a legend names its group)', async () => {
    await open();
    expect(document.querySelectorAll('legend button')).toHaveLength(0);
  });

  it('clicking a button does not save the settings', async () => {
    const user = await open();
    await user.click(aboutButton('Main lcore'));
    await user.click(aboutButton('Poll interval (ms)'));
    expect(screen.queryByText(/Settings saved/)).not.toBeInTheDocument();
    expect(rawStore()).toBeNull();
  });
});

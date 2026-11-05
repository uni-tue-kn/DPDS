// The page that hosts the config form (`ConfigFieldset`): the link editor. Spec27 removed the preset editor, so `PAGES` has one
// entry; the v16 tests (linkUnits / linkRequired ...) still run through this harness.
// v22: nothing is requested from the controller any more; "stored" means seeded into the local document, and what the page
// created or updated is read back from it. Stages (delayer, limiter, dropper) start as "Add ..." buttons.
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import * as fx from './fixtures';
import { addStages, renderApp, seedLinks, storedLinks, typeRate, watchStoreWrites } from './helpers';
import { must } from './must';

export interface ConfigPage {
  name: string;
  /** Legend of the outer config fieldset. */
  outerName: string;
  /** A new, untouched form: Buffer and Statistics, the three stages not added. */
  openNew(): Promise<UserEvent>;
  /** A new form with delayer, limiter and dropper added (their v16 default values). */
  openFull(): Promise<UserEvent>;
  /** The edit form (id 1) of a stored link whose config is `config`; it opens with exactly its present stages. */
  openStored(config: ConfigDto): Promise<UserEvent>;
  /** Everything but the config that a new form needs (name, ports), plus a valid initialization rate (pps) when a delayer is shown. */
  ready(user: UserEvent): Promise<void>;
  /** Clicks Save. */
  save(user: UserEvent): Promise<void>;
  /** The config of the one stored link after a save of a new form; fails when there is not exactly one link. */
  created(): ConfigDto;
  /** The config of the (seeded) link 1 after a save of a stored form; fails when there is not exactly one link. */
  updated(): ConfigDto;
  /** Number of times the local document was written since the form opened. */
  requests(): number;
}

let writeCount: () => number = () => 0;
/** Starts counting writes of the local document (call after seeding). */
function watchWrites(): void {
  writeCount = watchStoreWrites();
}

async function readyFields(user: UserEvent): Promise<void> {
  // a delayer needs its initialization rate (new forms start with it empty)
  if (screen.queryByLabelText('Initialization rate')) await typeRate(user);
}

const clickSave = (user: UserEvent) => user.click(screen.getByRole('button', { name: /^save$/i }));

export const linkPage: ConfigPage = {
  name: 'link editor',
  outerName: 'Link configuration',
  async openNew() {
    const user = userEvent.setup();
    renderApp('/settings/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
    watchWrites();
    return user;
  },
  async openFull() {
    const user = await linkPage.openNew();
    await addStages(user);
    return user;
  },
  async openStored(config) {
    const link = fx.link({ config });
    seedLinks([link]);
    const user = userEvent.setup();
    renderApp(`/settings/${link.id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue(link.name));
    watchWrites();
    return user;
  },
  async ready(user) {
    await user.type(screen.getByLabelText('Name'), 'my-link');
    await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.0');
    await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.1');
    await readyFields(user);
  },
  save: clickSave,
  created() {
    expect(storedLinks()).toHaveLength(1);
    return storedLinks()[0].config;
  },
  updated() {
    expect(storedLinks()).toHaveLength(1);
    return storedLinks()[0].config;
  },
  requests: () => writeCount(),
};

export const PAGES: ConfigPage[] = [linkPage];

/**
 * `fx.config()` with some delayer / limiter / histogram fields replaced (stored base values). Unlike `fx.config()` the
 * limiter is PRESENT (Spacer, 1 Gbit/s, with the overrides applied); pass `null` for a missing limiter or delayer.
 */
export function cfg(
  delayer: Partial<NonNullable<ConfigDto['delayer']>> | null = {},
  limiter: Partial<NonNullable<ConfigDto['limiter']>> | null = {},
  histogram: ConfigDto['histogram'] = null,
): ConfigDto {
  const base = fx.config();
  return {
    ...base,
    delayer: delayer === null ? null : { ...must(base.delayer), ...delayer },
    limiter: limiter === null ? null : { mode: 'Spacer', rate_bps: 1e9, ...limiter },
    histogram,
  };
}

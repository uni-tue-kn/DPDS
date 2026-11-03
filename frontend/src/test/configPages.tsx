// The two pages that host the shared config form (`ConfigFieldset`): the link editor and the preset editor.
// The v16 tests (linkUnits / linkRequired) run the same checks against both through this harness.
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import { api } from './mockApi';
import * as fx from './fixtures';
import { renderApp, typeRate } from './helpers';

export interface ConfigPage {
  name: string;
  /** Legend of the outer config fieldset. */
  outerName: string;
  /** A new, untouched form. */
  openNew(): Promise<UserEvent>;
  /** The edit form of a stored link/preset whose config is `config`. */
  openStored(config: ConfigDto): Promise<UserEvent>;
  /** Everything but the config that a new form needs (name, ports for links) plus a valid initialization rate (pps). */
  ready(user: UserEvent): Promise<void>;
  /** Clicks Save. */
  save(user: UserEvent): Promise<void>;
  /** The config of the one create call (new form); fails when there was not exactly one. */
  created(): ConfigDto;
  /** The config of the one update call (stored form); fails when there was not exactly one. */
  updated(): ConfigDto;
  /** Number of create + update requests made so far. */
  requests(): number;
}

const clickSave = (user: UserEvent) => user.click(screen.getByRole('button', { name: /^save$/i }));

export const linkPage: ConfigPage = {
  name: 'link editor',
  outerName: 'Link configuration',
  async openNew() {
    api.getLinks.mockResolvedValue([]);
    const user = userEvent.setup();
    renderApp('/links/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
    return user;
  },
  async openStored(config) {
    const link = fx.link({ config });
    api.getLink.mockResolvedValue(link);
    api.getLinks.mockResolvedValue([link]);
    const user = userEvent.setup();
    renderApp(`/links/${link.id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue(link.name));
    return user;
  },
  async ready(user) {
    await user.type(screen.getByLabelText('Name'), 'my-link');
    await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.0');
    await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.1');
    await typeRate(user);
  },
  save: clickSave,
  created() {
    expect(api.createLink).toHaveBeenCalledTimes(1);
    return api.createLink.mock.calls[0][0].config;
  },
  updated() {
    expect(api.updateLink).toHaveBeenCalledTimes(1);
    return api.updateLink.mock.calls[0][1].config;
  },
  requests: () => api.createLink.mock.calls.length + api.updateLink.mock.calls.length,
};

export const presetPage: ConfigPage = {
  name: 'preset editor',
  outerName: 'Preset configuration',
  async openNew() {
    const user = userEvent.setup();
    renderApp('/presets/new');
    await screen.findByRole('heading', { level: 1, name: 'New preset' });
    return user;
  },
  async openStored(config) {
    const preset = fx.preset({ id: 1, name: 'stored', config });
    api.getPreset.mockResolvedValue(preset);
    const user = userEvent.setup();
    renderApp('/presets/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('stored'));
    return user;
  },
  async ready(user) {
    await user.type(screen.getByLabelText('Name'), 'my-preset');
    await typeRate(user);
  },
  save: clickSave,
  created() {
    expect(api.createPreset).toHaveBeenCalledTimes(1);
    return api.createPreset.mock.calls[0][0].config;
  },
  updated() {
    expect(api.updatePreset).toHaveBeenCalledTimes(1);
    return api.updatePreset.mock.calls[0][1].config;
  },
  requests: () => api.createPreset.mock.calls.length + api.updatePreset.mock.calls.length,
};

export const PAGES: ConfigPage[] = [linkPage, presetPage];

/** `fx.config()` with some delayer / limiter / histogram fields replaced (stored base values). */
export function cfg(
  delayer: Partial<ConfigDto['delayer']> = {},
  limiter: Partial<ConfigDto['limiter']> = {},
  histogram: ConfigDto['histogram'] = null,
): ConfigDto {
  const base = fx.config();
  return { ...base, delayer: { ...base.delayer, ...delayer }, limiter: { ...base.limiter, ...limiter }, histogram };
}

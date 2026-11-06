import { screen } from '@testing-library/react';
import { readdirSync } from 'node:fs';
import { join } from 'node:path';
import { vi } from 'vitest';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api } from '../test/mockApi';

// The Interfaces page does not show the NIC statistics table (data-testid="ports-table", heading "NIC ports"), and `GET /api/stats`
// carries no `ports`, so the page does not poll the stats either: it does not call getStats at all.

describe('Interfaces page (spec 43 F6): no NIC statistics table', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
  });

  it('keeps the interface list and shows neither the table nor its heading', async () => {
    renderApp('/interfaces');
    expect(await screen.findByRole('cell', { name: '0000:65:00.0' })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'PCI' })).toBeInTheDocument();
    expect(screen.queryByTestId('ports-table')).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /NIC ports/i })).not.toBeInTheDocument();
    for (const c of ['ipackets', 'opackets', 'ibytes', 'obytes', 'ierrors', 'oerrors']) {
      expect(screen.queryByRole('columnheader', { name: c })).not.toBeInTheDocument();
    }
  });

  it('does not poll the stats (their ports are gone)', async () => {
    vi.useFakeTimers();
    renderApp('/interfaces');
    await tick(0);
    await tick(5000);
    expect(api.getStats).not.toHaveBeenCalled();
  });

  it('PortsTable.tsx is deleted', () => {
    expect(readdirSync(join(process.cwd(), 'src/pages'))).not.toContain('PortsTable.tsx');
  });
});

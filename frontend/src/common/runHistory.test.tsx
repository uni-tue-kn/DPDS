// The run history: summing the series of several links, and the polling of the controller's series (only what is new, a full
// answer for another run, errors that recover).
import { renderHook } from '@testing-library/react';
import { vi } from 'vitest';
import * as fx from '../test/fixtures';
import { tick } from '../test/helpers';
import { api, type HistoryServer, serveHistory } from '../test/mockApi';
import { must } from '../test/must';
import { sumHistories, useRunHistory } from './runHistory';

describe('sumHistories', () => {
  it('sums rates and cumulative drops at the times every link has a sample', () => {
    const a = [fx.historySample(1000), fx.historySample(2000)];
    const b = [fx.historySample(2000, { loss_drops: 2 }), fx.historySample(3000)];
    const sum = sumHistories([a, b]);
    expect(sum.map((s) => s.t_ms)).toEqual([2000]);
    expect(sum[0]).toMatchObject({ rx_gbps: 19, tx_mpps: 2.4, loss_drops: 42 });
  });

  it('is empty without links, and when one link has no sample', () => {
    expect(sumHistories([])).toEqual([]);
    expect(sumHistories([fx.historySamples(0, 3), []])).toEqual([]);
  });

  it('a count that is null on some links is the sum of the others, null when it is null on all', () => {
    const vf = fx.historySample(0, { nic_rx_phy_discards: null });
    const pf = fx.historySample(0, { nic_rx_phy_discards: 4 });
    expect(must(sumHistories([[vf], [pf]])[0]).nic_rx_phy_discards).toBe(4);
    expect(must(sumHistories([[vf], [vf]])[0]).nic_rx_phy_discards).toBeNull();
  });
});

describe('useRunHistory', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it('asks for everything first, then only for what is newer than the newest sample held', async () => {
    const server: HistoryServer = { startedAt: 1, samples: { '0': fx.historySamples(0, 2) } };
    serveHistory(server);
    const { result } = renderHook(() => useRunHistory());
    await tick(0);
    expect(api.getStatsHistory).toHaveBeenLastCalledWith(undefined);
    expect(result.current.history['0']).toHaveLength(2);

    server.samples = { '0': fx.historySamples(0, 4) };
    await tick(1000);
    expect(api.getStatsHistory).toHaveBeenLastCalledWith(1000);
    expect(result.current.history['0']?.map((s) => s.t_ms)).toEqual([0, 1000, 2000, 3000]);
    expect(result.current.runId).toBe(1);
  });

  it('another run (new run_id) replaces the series by a full answer', async () => {
    const server: HistoryServer = { startedAt: 1, samples: { '0': fx.historySamples(0, 5) } };
    serveHistory(server);
    const { result } = renderHook(() => useRunHistory());
    await tick(0);
    server.runId = 2;
    server.startedAt = 2;
    server.samples = { '0': fx.historySamples(0, 2) };
    await tick(1000);
    expect(result.current.history['0']).toHaveLength(2);
    expect(result.current.runId).toBe(2);
    expect(api.getStatsHistory).toHaveBeenLastCalledWith();
  });

  it('a failing request reports its message and the next good answer clears it', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    api.getStatsHistory.mockRejectedValueOnce(new Error('history down'));
    const { result } = renderHook(() => useRunHistory());
    await tick(0);
    expect(result.current.error).toBe('history down');
    await tick(1000);
    expect(result.current.error).toBeUndefined();
  });
});

import { vi } from 'vitest';
import { startPolling } from './polling';

describe('startPolling', () => {
  beforeEach(() => vi.useFakeTimers());

  it('does not run before the first interval unless asked to', async () => {
    const task = vi.fn().mockResolvedValue(undefined);
    const stop = startPolling(task, 1000);
    await vi.advanceTimersByTimeAsync(999);
    expect(task).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(task).toHaveBeenCalledTimes(1);
    stop();
  });

  it('runs immediately with runImmediately', async () => {
    const task = vi.fn().mockResolvedValue(undefined);
    const stop = startPolling(task, 1000, true);
    await vi.advanceTimersByTimeAsync(0);
    expect(task).toHaveBeenCalledTimes(1);
    stop();
  });

  it('never overlaps runs: the delay starts after the previous run settled', async () => {
    let active = 0;
    let maxActive = 0;
    let calls = 0;
    const task = async () => {
      calls += 1;
      active += 1;
      maxActive = Math.max(maxActive, active);
      await new Promise((r) => setTimeout(r, 3000));
      active -= 1;
    };
    const stop = startPolling(task, 1000, true);
    await vi.advanceTimersByTimeAsync(19999);
    stop();
    expect(maxActive).toBe(1);
    // 3 s run + 1 s pause = 4 s per cycle: runs at 0, 4, 8, 12, 16 s (the next would be 20 s).
    expect(calls).toBe(5);
  });

  it('keeps polling after a task throws or rejects', async () => {
    const task = vi
      .fn()
      .mockRejectedValueOnce(new Error('boom'))
      .mockImplementationOnce(() => {
        throw new Error('sync boom');
      })
      .mockResolvedValue(undefined);
    const spy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const stop = startPolling(task, 100, true);
    await vi.advanceTimersByTimeAsync(1000);
    stop();
    spy.mockRestore();
    expect(task.mock.calls.length).toBeGreaterThanOrEqual(5);
  });

  it('the stop function ends polling, also while a run is in flight', async () => {
    let finish: () => void = () => undefined;
    const task = vi.fn(
      () =>
        new Promise<void>((r) => {
          finish = r;
        }),
    );
    const stop = startPolling(task, 100, true);
    await vi.advanceTimersByTimeAsync(0);
    expect(task).toHaveBeenCalledTimes(1);
    stop();
    finish();
    await vi.advanceTimersByTimeAsync(10000);
    expect(task).toHaveBeenCalledTimes(1);
  });

  it('stop before the first run cancels it', async () => {
    const task = vi.fn();
    const stop = startPolling(task, 100);
    stop();
    await vi.advanceTimersByTimeAsync(1000);
    expect(task).not.toHaveBeenCalled();
  });
});

// `isUpdateAvailable` compares the controller's version with the first line of VERSION.md on main; any failure means "no update".
import { vi } from 'vitest';
import { isUpdateAvailable } from './updateCheck';

const fetchMock = vi.fn();
beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});

const remoteSays = (body: string, init: { ok?: boolean; status?: number } = {}) =>
  fetchMock.mockResolvedValue({ ok: init.ok ?? true, status: init.status ?? 200, text: async () => body });

describe('isUpdateAvailable', () => {
  it('asks GitHub for the VERSION.md of main without caching', async () => {
    remoteSays('1.0.0');
    await isUpdateAvailable('1.0.0');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      'https://raw.githubusercontent.com/uni-tue-kn/DPDS/refs/heads/main/VERSION.md',
    );
    expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({ cache: 'no-store' });
  });

  it.each([
    ['1.0.0', '1.0.1', true],
    ['1.0.9', '1.1.0', true],
    ['1.9.9', '2.0.0', true],
    ['1.2.3', '1.2.3', false],
    ['1.2.3', '1.2.2', false],
    ['2.0.0', '1.9.9', false],
    ['1.9.0', '1.10.0', true], // numbers, not strings
    ['v1.0.0', ' v1.0.1 ', true], // an optional v and white space
  ])('local %s, remote %j: %s', async (local, remote, expected) => {
    remoteSays(`${remote}\n`);
    await expect(isUpdateAvailable(local)).resolves.toBe(expected);
  });

  it('reads only the first line of the remote file, LF or CRLF', async () => {
    remoteSays('1.0.1\r\nfurther notes 9.9.9\n');
    await expect(isUpdateAvailable('1.0.0')).resolves.toBe(true);
    remoteSays('1.0.0\n9.9.9\n');
    await expect(isUpdateAvailable('1.0.0')).resolves.toBe(false);
  });

  it.each([
    ['an empty remote', ''],
    ['a remote with a suffix', '1.0.1-rc1'],
    ['a remote with four parts', '1.0.1.0'],
    ['an HTML error page', '<html>404</html>'],
  ])('%s is no update', async (_name, body) => {
    remoteSays(body);
    await expect(isUpdateAvailable('1.0.0')).resolves.toBe(false);
  });

  it('a malformed local version is no update and asks nobody', async () => {
    remoteSays('9.9.9');
    for (const local of ['dev', '', '1.0', 'v1.0.0.0']) await expect(isUpdateAvailable(local)).resolves.toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('a failing answer is no update, even with a newer version in its body', async () => {
    remoteSays('9.9.9', { ok: false, status: 429 });
    await expect(isUpdateAvailable('1.0.0')).resolves.toBe(false);
  });

  it.each([
    ['a network error', () => fetchMock.mockRejectedValue(new TypeError('Failed to fetch'))],
    [
      'a fetch that throws at once',
      () =>
        fetchMock.mockImplementation(() => {
          throw new Error('no fetch');
        }),
    ],
    [
      'a body that cannot be read',
      () =>
        fetchMock.mockResolvedValue({
          ok: true,
          status: 200,
          text: async () => {
            throw new Error('aborted');
          },
        }),
    ],
  ])('%s is no update', async (_name, arrange) => {
    arrange();
    await expect(isUpdateAvailable('1.0.0')).resolves.toBe(false);
  });
});

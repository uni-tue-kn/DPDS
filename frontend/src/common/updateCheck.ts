const VERSION_URL = 'https://raw.githubusercontent.com/uni-tue-kn/DPDS/refs/heads/main/VERSION.md';

/** The DPDS repository on GitHub. */
export const REPO_URL = 'https://github.com/uni-tue-kn/DPDS';
export const RELEASES_URL = `${REPO_URL}/releases`;

/** `x.y.z` (optionally `v`-prefixed, surrounding white space allowed) as numbers, or null. */
function toParts(v: string): number[] | null {
  const m = /^\s*v?(\d+)\.(\d+)\.(\d+)\s*$/.exec(v);
  return m ? m.slice(1).map(Number) : null;
}

/** Is the VERSION.md on main newer than `localVersion`? Any failure (network, bad status, malformed version on either side) means "no update". */
export async function isUpdateAvailable(localVersion: string): Promise<boolean> {
  const local = toParts(localVersion);
  if (!local) return false;

  let remoteText: string;
  try {
    const response = await fetch(VERSION_URL, { cache: 'no-store' });
    if (!response.ok) return false;
    remoteText = await response.text();
  } catch {
    return false;
  }

  const remote = toParts(remoteText.split(/\r?\n/)[0] ?? '');
  if (!remote) return false;

  for (let i = 0; i < 3; i++) {
    if (local[i] < remote[i]) return true;
    if (local[i] > remote[i]) return false;
  }
  return false;
}

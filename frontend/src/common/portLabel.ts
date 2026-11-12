import { useEffect, useState } from 'react';
import * as api from '../api/client';
import type { Interface } from '../api/types';

/** The Linux interface name (netdev) of a port; a port without one (vdev, vfio-bound) keeps its device name. */
export type PortLabeler = (port?: string | null) => string | undefined;

/**
 * Loads the interface list once per mount and returns a labeler for ports. Never fails and never blocks: until the
 * list arrives (or if it cannot be fetched) ports keep their plain device names.
 */
export function usePortLabeler(): PortLabeler {
  const [list, setList] = useState<Interface[]>();
  useEffect(() => {
    let alive = true;
    api.getInterfaces().then(
      (l) => {
        if (alive) setList(l);
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);
  return labelerOf(list);
}

/** A labeler over an interface list that is already loaded (none yet: plain device names). */
export function labelerOf(list?: Interface[]): PortLabeler {
  const names = new Map<string, string>();
  for (const i of list ?? []) if (i.netdev) names.set(i.port, i.netdev);
  return (port) => (port ? (names.get(port) ?? port) : (port ?? undefined));
}

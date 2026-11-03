import { useEffect, useState } from 'react';
import * as api from '../api/client';

/** "0000:65:00.0 (enp101s0f0)", or just the device name for a port without a kernel netdev (vdev, vfio-bound). */
export type PortLabeler = (port?: string | null) => string | undefined;

/**
 * Loads the interface list once per mount and returns a labeler for ports. Never fails and never blocks: until the
 * list arrives (or if it cannot be fetched) ports keep their plain device names.
 */
export function usePortLabeler(): PortLabeler {
  const [names, setNames] = useState<Map<string, string>>(new Map());
  useEffect(() => {
    let alive = true;
    api.getInterfaces().then(
      (list) => {
        if (!alive) return;
        const m = new Map<string, string>();
        for (const i of list) if (i.netdev) m.set(i.pci, i.netdev);
        setNames(m);
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);
  return (port) => {
    if (!port) return port ?? undefined;
    const netdev = names.get(port);
    return netdev ? `${port} (${netdev})` : port;
  };
}

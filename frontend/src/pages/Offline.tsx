import { useEffect } from 'react';
import { Row } from 'react-bootstrap';
import * as api from '../api/client';
import { startPolling } from '../common/polling';

const OFFLINE_POLL_MS = 2000;

/** Shown instead of the routed page while the controller is unreachable. */
export default function Offline({ onOnline }: { onOnline: () => void }) {
  useEffect(() => {
    let alive = true;
    const stop = startPolling(
      async () => {
        await api.getOnline();
        if (alive) onOnline();
      },
      OFFLINE_POLL_MS,
      true,
    );
    return () => {
      alive = false;
      stop();
    };
  }, [onOnline]);
  return (
    <Row className="text-center">
      <i className="bi bi-wifi-off" aria-hidden="true" style={{ fontSize: '100px', marginTop: 'calc(30vh)' }} />
      <p>DPDS device not reachable</p>
    </Row>
  );
}

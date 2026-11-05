import { Button } from 'react-bootstrap';
import { useLocalConfig, writeConfig } from '../common/localConfig';
import type { EmulationView } from '../api/types';

/**
 * Shown while an emulation runs and this browser has no links (a fresh browser, or one that was reset): copies the
 * running configuration into the local document.
 */
export default function LoadRunning({ emulation }: { emulation?: EmulationView }) {
  const { doc, corrupt } = useLocalConfig();
  const running = emulation?.config;
  if (!running || corrupt || doc.links.length > 0) return null;
  const load = () =>
    writeConfig({
      ...doc,
      links: running.links.map((l, i) => ({
        id: i + 1,
        name: l.name,
        ingress: l.ingress,
        egress: l.egress,
        config: l.config,
      })),
    });
  return (
    <div className="alert alert-info d-flex align-items-center gap-3">
      <span>An emulation is running, but this browser has no links.</span>
      <Button size="sm" variant="primary" onClick={load}>
        Load running configuration
      </Button>
    </div>
  );
}

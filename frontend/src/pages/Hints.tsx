import { Badge } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { useLoad } from '../common/useLoad';
import { startRequestOf, useLocalConfig } from '../common/localConfig';
import type { Hint, HintLevel } from '../api/types';

const GROUPS: { level: HintLevel; title: string; bg: string }[] = [
  { level: 'error', title: 'Errors', bg: 'danger' },
  { level: 'warn', title: 'Warnings', bg: 'warning' },
  { level: 'info', title: 'Info', bg: 'info' },
  { level: 'ok', title: 'OK', bg: 'success' },
];

export default function Hints() {
  const { doc, corrupt } = useLocalConfig();
  // Host-only hints, plus the ones that depend on the links of this browser's configuration (nothing is started).
  const host = useLoad(api.getHostHints);
  const forLinks = useLoad(() =>
    corrupt || doc.links.length === 0 ? Promise.resolve([]) : api.getHostHintsFor(startRequestOf(doc)),
  );
  const settled = (host.data ?? host.error) !== undefined && (forLinks.data ?? forLinks.error) !== undefined;
  // By hint id; the answer for the links wins.
  const data: Hint[] | undefined = settled
    ? [...new Map([...(host.data ?? []), ...(forLinks.data ?? [])].map((h) => [h.id, h])).values()]
    : undefined;
  const failed = host.error !== undefined || forLinks.error !== undefined;
  return (
    <>
      <h1 className="mb-3">Host hints</h1>
      <ErrorAlert message={host.error} />
      <ErrorAlert message={forLinks.error} />
      {data && data.length === 0 && !failed && <p>No hints.</p>}
      {data &&
        GROUPS.map((g) => {
          const items = data.filter((h) => h.level === g.level);
          if (items.length === 0) return null;
          const id = `hints-${g.level}`;
          return (
            <section key={g.level} className="hint-group" data-level={g.level} aria-labelledby={id}>
              <h2 id={id} className="h4">{`${g.title} (${items.length})`}</h2>
              <ul className="hint-list">
                {items.map((h) => (
                  <li key={h.id} className="hint-item" data-level={g.level}>
                    <Badge pill bg={g.bg} className="me-2">
                      {g.level}
                    </Badge>
                    <strong>{h.title}</strong>
                    <div>{h.detail}</div>
                    <div className="text-body-secondary">
                      <i className="bi bi-arrow-right-short" aria-hidden="true" />
                      {h.suggestion}
                    </div>
                  </li>
                ))}
              </ul>
            </section>
          );
        })}
    </>
  );
}

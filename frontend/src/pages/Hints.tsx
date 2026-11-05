import { useEffect, useId, useRef, useState } from 'react';
import { Badge, Button, OverlayTrigger, Tooltip } from 'react-bootstrap';
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

/**
 * The commands of a hint: a terminal icon whose tooltip (hover or focus) lists them, and a button that copies them.
 * DPDS never runs them; an administrator does, as root.
 */
function Commands({ hint, commands }: { hint: Hint; commands: string[] }) {
  const id = useId();
  // A fresh object per successful copy, so that a second copy restarts the 2 s of the "Copied" confirmation.
  const [copied, setCopied] = useState<object | null>(null);
  // Plain http has no clipboard; then the commands are shown for selecting by hand.
  const [manual, setManual] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(null), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  const text = commands.join('\n');
  // Read at click time: the clipboard can be missing, throw or refuse the write, and none of that may escape.
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied({});
    } catch {
      setCopied(null);
      setManual(true);
    }
  };
  return (
    <div className="mt-1">
      <OverlayTrigger
        placement="top"
        trigger={['hover', 'focus']}
        overlay={
          <Tooltip id={id} className="commands-tip">
            <pre className="text-start mb-1">
              <code>{text}</code>
            </pre>
            Run as root on the DPDS host.
          </Tooltip>
        }
      >
        <Button
          type="button"
          size="sm"
          variant="outline-secondary"
          className="me-2"
          aria-label={`Commands for ${hint.title}`}
        >
          <i className="bi bi-terminal" aria-hidden="true" />
        </Button>
      </OverlayTrigger>
      <Button type="button" size="sm" variant="outline-primary" onClick={copy}>
        Copy commands
      </Button>
      {/* Always mounted: screen readers announce changes of a live region, not one inserted together with its text. */}
      <output className="ms-2 text-success">{copied ? 'Copied' : ''}</output>
      {manual && (
        <div className="mt-2">
          <div>Clipboard not available here (plain http): select the commands and copy them.</div>
          <pre className="text-start mb-0 commands-fallback">{text}</pre>
        </div>
      )}
    </div>
  );
}

export default function Hints() {
  const { doc, corrupt } = useLocalConfig();
  // Host-only hints, plus the ones that depend on the links of this browser's configuration (nothing is started).
  const host = useLoad(api.getHostHints);
  // Set only around the reloads of a click on Recheck, which read the loaders synchronously.
  const recheck = useRef(false);
  const forLinks = useLoad(() =>
    corrupt || doc.links.length === 0
      ? Promise.resolve([])
      : api.getHostHintsFor(startRequestOf(doc), recheck.current ? { recheck: true } : undefined),
  );
  const busy = host.loading || forLinks.loading;
  const recheckNow = () => {
    if (busy) return;
    recheck.current = true;
    host.reload();
    forLinks.reload();
    recheck.current = false;
  };

  const settled = (host.data ?? host.error) !== undefined && (forLinks.data ?? forLinks.error) !== undefined;
  // By hint id; the answer for the links wins.
  const data: Hint[] | undefined = settled
    ? [...new Map([...(host.data ?? []), ...(forLinks.data ?? [])].map((h) => [h.id, h])).values()]
    : undefined;
  const failed = host.error !== undefined || forLinks.error !== undefined;
  return (
    <>
      <h1 className="mb-3 d-inline-block me-3 align-middle">Host hints</h1>
      <Button
        type="button"
        size="sm"
        variant="outline-primary"
        className="me-3 align-middle"
        disabled={busy}
        onClick={recheckNow}
      >
        <i className={`bi bi-arrow-clockwise me-1${busy ? ' icon-spin' : ''}`} aria-hidden="true" />
        Recheck
      </Button>
      {busy && (
        <output className="text-body-secondary align-middle">
          <span className="spinner-border spinner-border-sm me-2" aria-hidden="true" />
          Checking host…
        </output>
      )}
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
                    {h.commands && h.commands.length > 0 && <Commands hint={h} commands={h.commands} />}
                  </li>
                ))}
              </ul>
            </section>
          );
        })}
    </>
  );
}

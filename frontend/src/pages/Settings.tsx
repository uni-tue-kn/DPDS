import { useRef, useState } from 'react';
import type { ChangeEvent } from 'react';
import { Badge, Button, Modal, Table } from 'react-bootstrap';
import { Link as RouterLink, useNavigate } from 'react-router';
import * as api from '../api/client';
import {
  deleteLink,
  isLinksFile,
  linksProblem,
  parseStored,
  readRaw,
  replaceLinks,
  useLocalConfig,
} from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import LoadRunning from './LoadRunning';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import { usePortLabeler } from '../common/portLabel';
import LinkInspect from './LinkInspect';
import { delayCell, limiterCell, lossCell, reorderingCell } from './linkSummary';
import type { Link } from '../api/types';

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error ?? new Error('Cannot read the file'));
    r.readAsText(file);
  });
}

const pad = (n: number) => String(n).padStart(2, '0');
const localDate = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

/**
 * Downloads `{version: 1, links}` as `dpds-links-<local date>.json`: the links as stored, ids included, no settings.
 * An unreadable store is the rescue case: its stored text is downloaded unchanged.
 */
function downloadLinks() {
  const raw = readRaw();
  const stored = parseStored(raw);
  const text = stored.corrupt ? (raw ?? '') : JSON.stringify({ version: 1, links: stored.doc.links }, null, 2);
  const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = `dpds-links-${localDate(new Date())}.json`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export default function Settings() {
  const { doc, corrupt } = useLocalConfig();
  const emulation = useLoad(api.getEmulation);
  const label = usePortLabeler();
  const navigate = useNavigate();
  const [target, setTarget] = useState<Link | null>(null);
  const [inspected, setInspected] = useState<Link | null>(null);
  const [actionError, setActionError] = useState<string>();
  const [pendingImport, setPendingImport] = useState<Link[]>();
  const [imported, setImported] = useState<string>();
  const fileInput = useRef<HTMLInputElement>(null);

  const pending = pendingRestart(doc, emulation.data);
  const links = corrupt ? [] : doc.links;

  const confirmDelete = () => {
    if (!target) return;
    try {
      deleteLink(target.id);
    } catch (e) {
      setActionError(errorMessage(e));
    }
    setTarget(null);
  };

  const exportLinks = () => {
    setActionError(undefined);
    try {
      downloadLinks();
    } catch (e) {
      setActionError(errorMessage(e));
    }
  };

  const chooseFile = async (ev: ChangeEvent<HTMLInputElement>) => {
    const file = ev.target.files?.[0];
    ev.target.value = ''; // the same file can be chosen again
    if (!file) return;
    setActionError(undefined);
    setImported(undefined);
    let parsed: unknown;
    try {
      parsed = JSON.parse(await readText(file));
    } catch (e) {
      setActionError(`"${file.name}" is not a valid JSON links file: ${errorMessage(e)}`);
      return;
    }
    if (!isLinksFile(parsed)) {
      setActionError(`"${file.name}" is not a DPDS links file (a version 1 document with a list of links)`);
      return;
    }
    const problem = linksProblem(parsed.links);
    if (problem !== null) setActionError(`"${file.name}" cannot be imported. ${problem}`);
    else setPendingImport(parsed.links as Link[]);
  };

  const confirmImport = () => {
    if (!pendingImport) return;
    try {
      replaceLinks(pendingImport);
      setImported(`Imported ${pendingImport.length} links`);
    } catch (e) {
      setActionError(errorMessage(e));
    }
    setPendingImport(undefined);
  };

  return (
    <>
      <div className="d-flex justify-content-between align-items-center mb-3">
        <h1 className="mb-0">Settings</h1>
        <div className="text-end">
          <input
            ref={fileInput}
            type="file"
            accept=".json"
            aria-label="Import links file"
            className="d-none"
            onChange={chooseFile}
          />
          <Button variant="success" className="me-2" onClick={() => fileInput.current?.click()}>
            <i className="bi bi-cloud-arrow-down-fill" aria-hidden="true" /> Import
          </Button>
          <Button variant="danger" className="me-2" onClick={exportLinks}>
            <i className="bi bi-cloud-arrow-up-fill" aria-hidden="true" /> Export
          </Button>
          <RouterLink to="/settings/new" className="btn btn-primary">
            <i className="bi bi-plus-lg" aria-hidden="true" /> New link
          </RouterLink>
        </div>
      </div>
      <LoadRunning emulation={emulation.data} />
      {pending.any && (
        <div data-testid="pending-restart" className="alert alert-warning">
          <i className="bi bi-exclamation-triangle" aria-hidden="true" /> Links changed since the emulation started.
          Stop and start it again (restart) to apply them.
        </div>
      )}
      <ErrorAlert message={actionError} />
      {imported && (
        <div role="status" className="alert alert-success">
          {imported}
        </div>
      )}
      {!corrupt && links.length === 0 && <p>No links configured yet.</p>}
      {links.length > 0 && (
        <Table striped size="sm" responsive>
          <thead className="table-dark">
            <tr>
              <th>Name</th>
              <th>Interfaces</th>
              <th>Delay</th>
              <th>Reordering</th>
              <th>Limiter</th>
              <th>Loss</th>
              <th>
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {links.map((l) => (
              <tr key={l.id}>
                <td>
                  {l.name}
                  {pending.links.has(l.name) && (
                    <Badge pill bg="warning" text="dark" className="ms-2" data-testid="link-pending-restart">
                      pending restart
                    </Badge>
                  )}
                </td>
                <td>{`${label(l.ingress)} → ${label(l.egress)}`}</td>
                <td>{delayCell(l.config)}</td>
                <td>{reorderingCell(l.config)}</td>
                <td>{limiterCell(l.config)}</td>
                <td>{lossCell(l.config)}</td>
                <td className="text-end text-nowrap">
                  <Button
                    size="sm"
                    variant="outline-secondary"
                    className="me-1"
                    aria-label={`Inspect ${l.name}`}
                    onClick={() => setInspected(l)}
                  >
                    <i className="bi bi-eye" aria-hidden="true" />
                  </Button>
                  <RouterLink
                    to={`/settings/${l.id}`}
                    className="btn btn-sm btn-outline-secondary me-1"
                    aria-label={`Edit ${l.name}`}
                  >
                    <i className="bi bi-pencil" aria-hidden="true" />
                  </RouterLink>
                  <Button
                    size="sm"
                    variant="outline-secondary"
                    className="me-1"
                    aria-label={`Duplicate ${l.name}`}
                    onClick={() => navigate('/settings/new', { state: { copyOf: l.id } })}
                  >
                    <i className="bi bi-files" aria-hidden="true" />
                  </Button>
                  <Button
                    size="sm"
                    variant="outline-danger"
                    aria-label={`Delete ${l.name}`}
                    onClick={() => setTarget(l)}
                  >
                    <i className="bi bi-trash" aria-hidden="true" />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <LinkInspect link={inspected} label={label} onClose={() => setInspected(null)} />
      <Modal show={pendingImport !== undefined} onHide={() => setPendingImport(undefined)} animation={false} centered>
        <Modal.Header closeButton>
          <Modal.Title>Import links</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          Importing will replace all existing links with the links of the file. This cannot be undone.
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setPendingImport(undefined)}>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirmImport}>
            Confirm import
          </Button>
        </Modal.Footer>
      </Modal>
      <Modal show={target !== null} onHide={() => setTarget(null)} animation={false} centered>
        <Modal.Header closeButton>
          <Modal.Title>Delete link</Modal.Title>
        </Modal.Header>
        <Modal.Body>Delete link {target?.name}? This cannot be undone.</Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setTarget(null)}>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirmDelete}>
            Confirm delete
          </Button>
        </Modal.Footer>
      </Modal>
    </>
  );
}

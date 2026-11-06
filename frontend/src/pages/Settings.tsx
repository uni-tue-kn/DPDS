import { useCallback, useEffect, useRef, useState } from 'react';
import type { ChangeEvent } from 'react';
import { Badge, Button, Col, Form, Modal, Row, Table } from 'react-bootstrap';
import * as api from '../api/client';
import { isLinksFile, linksProblem, parseStored, readRaw, replaceLinks, useLocalConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import LoadRunning from './LoadRunning';
import InfoBox from '../components/InfoBox';
import PageTitle from '../components/PageTitle';
import { errorMessage } from '../common/format';
import { useToast } from '../common/toast';
import { useLoad } from '../common/useLoad';
import HistogramModal from './HistogramModal';
import LinkSettingsModal from './LinkSettingsModal';
import { HELP } from './fieldHelp';
import { addedRow, copiedRow, copyName, draftLinks, portUsers, rowsOf } from './linkDraft';
import type { DraftRow, Role } from './linkDraft';
import { rowSummary } from './linkSummary';
import type { Interface, Link } from '../api/types';

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

const COLUMNS = ['#', 'Name', 'Ingress', 'Egress', 'Buffer', 'Delay', 'Limiter', 'Loss', 'Histogram', 'Options'];
/** The columns whose heading has an info button. */
const HEAD_HELP: Record<string, keyof typeof HELP> = { Name: 'Link name', Ingress: 'Ingress', Egress: 'Egress' };

function optionLabel(i: Interface, usedBy: string | undefined, role: Role): string {
  const base = i.netdev ?? i.pci;
  const reasons: string[] = [];
  if (!i.dpds_usable || i.uplink) reasons.push(i.reason ?? (i.uplink ? 'uplink' : 'not usable'));
  if (usedBy !== undefined) reasons.push(`already used as ${role} by ${usedBy}`);
  return reasons.length === 0 ? base : `${base} - ${reasons.join('; ')}`;
}

/** A picker over the discovered interfaces, shown by netdev (device name without one). */
function PortSelect({
  role,
  n,
  value,
  interfaces,
  users,
  onChange,
}: {
  role: Role;
  n: number;
  value: string;
  interfaces: Interface[];
  users: Map<string, string>;
  onChange: (v: string) => void;
}) {
  // A stored port that is not a discovered interface (e.g. a vdev) stays selectable as its own option.
  const unknown = value !== '' && !interfaces.some((i) => i.pci === value);
  return (
    <Form.Select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label={`${role === 'ingress' ? 'Ingress' : 'Egress'} of link ${n}`}
    >
      <option value="">Select a port...</option>
      {unknown && <option value={value}>{value}</option>}
      {interfaces.map((i) => (
        <option key={i.pci} value={i.pci} disabled={!i.dpds_usable || i.uplink || users.has(i.pci)}>
          {optionLabel(i, users.get(i.pci), role)}
        </option>
      ))}
    </Form.Select>
  );
}

function SummaryCell({ text }: { text: string | null }) {
  return <td>{text ?? <span className="text-danger">invalid</span>}</td>;
}

type Open = { kind: 'link' | 'histogram'; key: number };

export default function Settings() {
  const { doc, corrupt } = useLocalConfig();
  const emulation = useLoad(api.getEmulation);
  const interfaces = useLoad(api.getInterfaces);
  const showToast = useToast();
  // The draft: the stored links, edited here and stored only by Save. Save, Reset and Import replace it explicitly; a
  // change of the store by something else (another tab, Load running configuration) replaces it only while it is clean.
  const [rows, setRows] = useState<DraftRow[]>(() => rowsOf(doc.links));
  // The stored text and the rows the draft was last seeded from; the draft is clean while it still equals these rows.
  const seeded = useRef({ raw: readRaw(), rows });
  const [open, setOpen] = useState<Open | null>(null);
  const [pendingImport, setPendingImport] = useState<Link[]>();
  const fileInput = useRef<HTMLInputElement>(null);

  const seed = useCallback((links: Link[]) => {
    const fresh = rowsOf(links);
    seeded.current = { raw: readRaw(), rows: fresh };
    setRows(fresh);
    setOpen(null); // a modal on a row of the old draft has nothing to edit any more
  }, []);

  useEffect(() => {
    if (readRaw() === seeded.current.raw) return; // already seeded: the first render, or our own Save / Import
    if (JSON.stringify(rows) === JSON.stringify(seeded.current.rows)) seed(doc.links);
  }, [doc, rows, seed]);

  useEffect(() => {
    if (interfaces.error) showToast(interfaces.error, 'danger');
  }, [interfaces.error, showToast]);

  const pending = pendingRestart(doc, emulation.data);
  const update = (key: number, patch: Partial<DraftRow>) =>
    setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...patch } : r)));
  const modalAt = open === null ? -1 : rows.findIndex((r) => r.key === open.key);
  const modalRow = modalAt < 0 ? undefined : rows[modalAt];
  const modalName = modalRow?.name.trim() ? `: ${modalRow.name.trim()}` : '';

  const duplicate = (at: number) => {
    const copy = copyName(rows[at].name, rows);
    setRows([...rows.slice(0, at + 1), copiedRow(rows[at], copy), ...rows.slice(at + 1)]);
    showToast(`Cloned "${rows[at].name}" to "${copy}"`, 'success');
  };

  const save = () => {
    const draft = draftLinks(rows);
    if ('problem' in draft) {
      showToast(draft.problem, 'warning');
      return;
    }
    try {
      replaceLinks(draft.links);
      seed(draft.links);
      showToast('Settings saved successfully.', 'success');
    } catch (e) {
      showToast(errorMessage(e), 'danger');
    }
  };

  const reset = () => {
    seed(doc.links);
    showToast('Settings reset successfully.', 'success');
  };

  const exportLinks = () => {
    try {
      downloadLinks();
    } catch (e) {
      showToast(errorMessage(e), 'danger');
    }
  };

  const chooseFile = async (ev: ChangeEvent<HTMLInputElement>) => {
    const file = ev.target.files?.[0];
    ev.target.value = ''; // the same file can be chosen again
    if (!file) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(await readText(file));
    } catch (e) {
      showToast(`"${file.name}" is not a valid JSON links file: ${errorMessage(e)}`, 'danger');
      return;
    }
    if (!isLinksFile(parsed)) {
      showToast(`"${file.name}" is not a DPDS links file (a version 1 document with a list of links)`, 'danger');
      return;
    }
    const problem = linksProblem(parsed.links);
    if (problem !== null) showToast(`"${file.name}" cannot be imported. ${problem}`, 'danger');
    else setPendingImport(parsed.links as Link[]);
  };

  const confirmImport = () => {
    if (!pendingImport) return;
    try {
      replaceLinks(pendingImport);
      seed(pendingImport);
      showToast('Settings imported successfully.', 'success');
    } catch (e) {
      showToast(errorMessage(e), 'danger');
    }
    setPendingImport(undefined);
  };

  return (
    <>
      <PageTitle title="Settings" />
      <Row className="align-items-center">
        <Col className="text-end">
          <input
            ref={fileInput}
            type="file"
            accept=".json"
            aria-label="Import links file"
            className="d-none"
            onChange={chooseFile}
          />
          <Button variant="primary" className="me-2" disabled={corrupt} onClick={() => fileInput.current?.click()}>
            <i className="bi bi-cloud-arrow-down-fill" aria-hidden="true" /> Import
          </Button>
          <Button variant="danger" onClick={exportLinks}>
            <i className="bi bi-cloud-arrow-up-fill" aria-hidden="true" /> Export
          </Button>
        </Col>
      </Row>
      <LoadRunning emulation={emulation.data} />
      {pending.any && (
        <div data-testid="pending-restart" className="alert alert-warning mt-3">
          <i className="bi bi-exclamation-triangle" aria-hidden="true" /> Links changed since the emulation started.
          Stop and start it again (restart) to apply them.
        </div>
      )}
      <Table striped bordered hover size="sm" className="mt-3 mb-3 text-center link-table">
        <thead className="table-dark">
          <tr>
            {COLUMNS.map((c) => (
              <th key={c}>
                {c}
                {HEAD_HELP[c] && (
                  <>
                    {' '}
                    <InfoBox label={c}>{HELP[HEAD_HELP[c]]}</InfoBox>
                  </>
                )}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, at) => {
            const n = at + 1;
            const s = rowSummary(r.config);
            return (
              <tr key={r.key}>
                <td>{n}</td>
                <td>
                  <Form.Control
                    type="text"
                    value={r.name}
                    aria-label={`Name of link ${n}`}
                    onChange={(e) => update(r.key, { name: e.target.value })}
                  />
                  {pending.links.has(r.name) && (
                    <Badge pill bg="warning" text="dark" className="mt-1" data-testid="link-pending-restart">
                      pending restart
                    </Badge>
                  )}
                </td>
                {(['ingress', 'egress'] as const).map((role) => (
                  <td key={role}>
                    <PortSelect
                      role={role}
                      n={n}
                      value={r[role]}
                      interfaces={interfaces.data ?? []}
                      users={portUsers(rows, r.key, role)}
                      onChange={(v) => update(r.key, { [role]: v })}
                    />
                  </td>
                ))}
                <SummaryCell text={s.buffer} />
                <SummaryCell text={s.delay} />
                <SummaryCell text={s.limiter} />
                <SummaryCell text={s.loss} />
                <td>
                  <button
                    type="button"
                    className="btn btn-config border-0 p-0 me-2"
                    aria-label={`Histogram of link ${n}`}
                    onClick={() => setOpen({ kind: 'histogram', key: r.key })}
                  >
                    <i className="bi bi-bar-chart-line-fill" aria-hidden="true" />
                  </button>
                  {s.histogram ?? <span className="text-danger">invalid</span>}
                </td>
                <td>
                  <div className="d-flex align-items-center justify-content-end gap-2">
                    <Button
                      size="sm"
                      variant="outline-secondary"
                      aria-label={`Settings of link ${n}`}
                      onClick={() => setOpen({ kind: 'link', key: r.key })}
                    >
                      <i className="bi bi-sliders" aria-hidden="true" />
                    </Button>
                    <Button
                      size="sm"
                      variant="outline-secondary"
                      aria-label={`Duplicate link ${n}`}
                      onClick={() => duplicate(at)}
                    >
                      <i className="bi bi-files" aria-hidden="true" />
                    </Button>
                    <Button
                      size="sm"
                      variant="outline-secondary"
                      aria-label={`Delete link ${n}`}
                      onClick={() => setRows(rows.filter((o) => o.key !== r.key))}
                    >
                      <i className="bi bi-trash2-fill" aria-hidden="true" />
                    </Button>
                  </div>
                </td>
              </tr>
            );
          })}
          {rows.length === 0 && !corrupt && (
            <tr>
              <td colSpan={COLUMNS.length}>No links configured yet.</td>
            </tr>
          )}
        </tbody>
      </Table>
      <Row className="mb-3">
        <Col className="text-start">
          <Button variant="primary" disabled={corrupt} onClick={() => setRows([...rows, addedRow(rows)])}>
            <i className="bi bi-plus" aria-hidden="true" /> Add link
          </Button>
        </Col>
      </Row>
      <Row>
        <Col>
          <Button variant="primary" disabled={corrupt} onClick={save}>
            <i className="bi bi-check" aria-hidden="true" /> Save
          </Button>{' '}
          <Button variant="danger" disabled={corrupt} onClick={reset}>
            <i className="bi bi-x-octagon-fill" aria-hidden="true" /> Reset
          </Button>
        </Col>
      </Row>
      {modalRow && open?.kind === 'link' && (
        <LinkSettingsModal
          key={open.key}
          title={`Link #${modalAt + 1}${modalName}`}
          initial={modalRow.config}
          onConfirm={(config) => {
            update(modalRow.key, { config });
            setOpen(null);
          }}
          onClose={() => setOpen(null)}
        />
      )}
      {modalRow && open?.kind === 'histogram' && (
        <HistogramModal
          key={open.key}
          title={`Histograms of link #${modalAt + 1}${modalName}`}
          initial={modalRow.config}
          onConfirm={(config) => {
            update(modalRow.key, { config });
            setOpen(null);
          }}
          onClose={() => setOpen(null)}
        />
      )}
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
    </>
  );
}

import { useState } from 'react';
import { Button, Modal, Table } from 'react-bootstrap';
import { Link as RouterLink } from 'react-router';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import { useLoad } from '../common/useLoad';
import { usePortLabeler } from '../common/portLabel';
import LinkInspect from './LinkInspect';
import { delayCell, limiterCell, lossCell, reorderingCell } from './linkSummary';
import type { Link } from '../api/types';

export default function Links() {
  const links = useLoad(api.getLinks);
  const emulation = useLoad(api.getEmulation);
  const label = usePortLabeler();
  const [target, setTarget] = useState<Link | null>(null);
  const [inspected, setInspected] = useState<Link | null>(null);
  const [actionError, setActionError] = useState<string>();
  const [deleting, setDeleting] = useState(false);

  const confirmDelete = async () => {
    if (!target || deleting) return;
    setDeleting(true);
    setActionError(undefined);
    try {
      await api.deleteLink(target.id);
      links.reload();
    } catch (e) {
      setActionError(errorMessage(e));
    } finally {
      setDeleting(false);
      setTarget(null);
    }
  };

  return (
    <>
      <div className="d-flex justify-content-between align-items-center mb-3">
        <h1 className="mb-0">Links</h1>
        <RouterLink to="/links/new" className="btn btn-primary">
          <i className="bi bi-plus-lg" aria-hidden="true" /> New link
        </RouterLink>
      </div>
      {emulation.data?.pending_restart && (
        <div data-testid="pending-restart" className="alert alert-warning">
          <i className="bi bi-exclamation-triangle" aria-hidden="true" /> Links or settings changed since the emulation started.
          Stop and start it again (restart) to apply them.
        </div>
      )}
      <ErrorAlert message={links.error ?? actionError} />
      {links.data && links.data.length === 0 && <p>No links configured yet.</p>}
      {links.data && links.data.length > 0 && (
        <Table striped size="sm" responsive>
          <thead className="table-dark">
            <tr><th>Name</th><th>Interfaces</th><th>Delay</th><th>Reordering</th><th>Limiter</th><th>Loss</th><th /></tr>
          </thead>
          <tbody>
            {links.data.map((l) => (
              <tr key={l.id}>
                <td>{l.name}</td>
                <td>{`${label(l.ingress)} → ${label(l.egress)}`}</td>
                <td>{delayCell(l.config)}</td>
                <td>{reorderingCell(l.config)}</td>
                <td>{limiterCell(l.config)}</td>
                <td>{lossCell(l.config)}</td>
                <td className="text-end text-nowrap">
                  <Button size="sm" variant="outline-secondary" className="me-1" aria-label={`Inspect ${l.name}`} onClick={() => setInspected(l)}>
                    <i className="bi bi-eye" aria-hidden="true" />
                  </Button>
                  <RouterLink to={`/links/${l.id}`} className="btn btn-sm btn-outline-secondary me-1" aria-label={`Edit ${l.name}`}>
                    <i className="bi bi-pencil" aria-hidden="true" />
                  </RouterLink>
                  <Button size="sm" variant="outline-danger" aria-label={`Delete ${l.name}`} onClick={() => setTarget(l)}>
                    <i className="bi bi-trash" aria-hidden="true" />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <LinkInspect link={inspected} label={label} onClose={() => setInspected(null)} />
      <Modal show={target !== null} onHide={() => setTarget(null)} animation={false} centered>
        <Modal.Header closeButton><Modal.Title>Delete link</Modal.Title></Modal.Header>
        <Modal.Body>Delete link {target?.name}? This cannot be undone.</Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setTarget(null)}>Cancel</Button>
          <Button variant="danger" disabled={deleting} onClick={confirmDelete}>Confirm delete</Button>
        </Modal.Footer>
      </Modal>
    </>
  );
}

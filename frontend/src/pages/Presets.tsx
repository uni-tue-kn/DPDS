import { useState } from 'react';
import { Button, Modal, Table } from 'react-bootstrap';
import { Link as RouterLink } from 'react-router';
import { deletePreset, useLocalConfig } from '../common/localConfig';
import ErrorAlert from '../components/ErrorAlert';
import { errorMessage } from '../common/format';
import type { Preset } from '../api/types';

export default function Presets() {
  const { doc, corrupt } = useLocalConfig();
  const presets = corrupt ? [] : doc.presets;
  const [target, setTarget] = useState<Preset | null>(null);
  const [actionError, setActionError] = useState<string>();

  const confirmDelete = () => {
    if (!target) return;
    try {
      deletePreset(target.id);
    } catch (e) {
      setActionError(errorMessage(e));
    }
    setTarget(null);
  };

  return (
    <>
      <div className="d-flex justify-content-between align-items-center mb-3">
        <h1 className="mb-0">Presets</h1>
        <RouterLink to="/presets/new" className="btn btn-primary">
          <i className="bi bi-plus-lg" aria-hidden="true" /> New preset
        </RouterLink>
      </div>
      <ErrorAlert message={actionError} />
      {!corrupt && presets.length === 0 && <p>No presets yet.</p>}
      {presets.length > 0 && (
        <Table striped size="sm" responsive>
          <thead className="table-dark">
            <tr>
              <th>Name</th>
              <th>Description</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {presets.map((p) => (
              <tr key={p.id}>
                <td>{p.name}</td>
                <td>{p.description}</td>
                <td className="text-end text-nowrap">
                  <RouterLink
                    to={`/presets/${p.id}`}
                    className="btn btn-sm btn-outline-secondary me-1"
                    aria-label={`Edit ${p.name}`}
                  >
                    <i className="bi bi-pencil" aria-hidden="true" />
                  </RouterLink>
                  <Button
                    size="sm"
                    variant="outline-danger"
                    aria-label={`Delete ${p.name}`}
                    onClick={() => setTarget(p)}
                  >
                    <i className="bi bi-trash" aria-hidden="true" />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <Modal show={target !== null} onHide={() => setTarget(null)} animation={false} centered>
        <Modal.Header closeButton>
          <Modal.Title>Delete preset</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          Delete preset {target?.name}? Links that used it keep their settings. This cannot be undone.
        </Modal.Body>
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

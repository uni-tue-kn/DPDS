import { useState } from 'react';
import type { ReactNode } from 'react';
import { Button, Modal } from 'react-bootstrap';

/** A question-mark button next to a label; it opens a centered modal with the explanation `children`. */
export default function InfoBox({ label, children }: { label: string; children: ReactNode }) {
  const [show, setShow] = useState(false);
  return (
    <>
      <button type="button" className="info-tip" aria-label={`About ${label}`} onClick={() => setShow(true)}>
        <i className="bi bi-question-circle-fill" aria-hidden="true" />
      </button>
      <Modal centered show={show} onHide={() => setShow(false)} animation={false}>
        <Modal.Header closeButton>
          <Modal.Title>Info</Modal.Title>
        </Modal.Header>
        <Modal.Body>
          <h5>{label}</h5>
          <p>{children}</p>
        </Modal.Body>
        <Modal.Footer>
          <Button variant="danger" onClick={() => setShow(false)}>
            Close
          </Button>
        </Modal.Footer>
      </Modal>
    </>
  );
}

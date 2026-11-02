import { Alert } from 'react-bootstrap';

/** Renders nothing without a message; otherwise a `role="alert"` danger box. */
export default function ErrorAlert({ message }: { message?: string | null }) {
  if (!message) return null;
  return <Alert variant="danger" role="alert">{message}</Alert>;
}

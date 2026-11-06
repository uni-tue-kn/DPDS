import type { ReactNode } from 'react';
import { Spinner } from 'react-bootstrap';

/** The spinner with the "DPDS" wordmark below it. */
function Spinning() {
  return (
    <div className="loader-spinner-box">
      <Spinner animation="border" role="status" aria-label="Loading" className="loader-spinner" />
      <p>
        <span className="loader-text">DPDS</span>
      </p>
    </div>
  );
}

/**
 * The children when `loaded`, else the centred loader block; `overlay` adds a layer over the
 * content (the children stay mounted) until it is cleared.
 */
export default function Loader({
  children,
  loaded = true,
  overlay = false,
}: {
  children: ReactNode;
  loaded?: boolean;
  overlay?: boolean;
}) {
  return (
    <div className="loader-content">
      {loaded ? (
        children
      ) : (
        <div className="loader-block">
          <Spinning />
        </div>
      )}
      {overlay && (
        <div className="loader-overlay">
          <Spinning />
        </div>
      )}
    </div>
  );
}

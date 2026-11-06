import { Toast, ToastContainer } from 'react-bootstrap';
import type { ToastVariant } from '../common/toast';

const VARIANTS: Record<ToastVariant, { icon: string; title: string }> = {
  success: { icon: 'bi-check-circle-fill', title: 'Success' },
  danger: { icon: 'bi-x-octagon-fill', title: 'Error' },
  info: { icon: 'bi-info-circle-fill', title: 'Info' },
  warning: { icon: 'bi-exclamation-triangle-fill', title: 'Warning' },
};

const DELAY_MS = 4000;
const ERROR_DELAY_MS = 12_000;

/** The toast at the bottom centre of the screen; colours and layout are in index.css (`.toast-floating`). */
export default function ToastMessage({
  message,
  show,
  bg,
  onClose,
}: {
  message: string;
  show: boolean;
  bg: ToastVariant;
  onClose: () => void;
}) {
  const variant = VARIANTS[bg];
  const danger = bg === 'danger';
  return (
    <ToastContainer className="toast-floating-container">
      <Toast
        className={`toast-floating toast-${bg}`}
        onClose={onClose}
        show={show}
        delay={danger ? ERROR_DELAY_MS : DELAY_MS}
        autohide
        role={danger ? 'alert' : 'status'}
        aria-live={danger ? 'assertive' : 'polite'}
        aria-atomic="true"
      >
        <div className="toast-floating-content">
          <div className="toast-floating-icon" aria-hidden="true">
            <i className={`bi ${variant.icon}`} />
          </div>
          <div>
            <div className="toast-floating-title">{variant.title}</div>
            <div className="toast-floating-text">{message}</div>
          </div>
          <button type="button" className="btn-close" onClick={onClose} aria-label="Close notification" />
        </div>
      </Toast>
    </ToastContainer>
  );
}

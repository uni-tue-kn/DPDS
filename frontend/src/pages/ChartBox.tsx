import type { ReactNode } from 'react';

/** A chart as an image for assistive technology: the canvas itself has no text. */
export default function ChartBox({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="chart-box" role="img" aria-label={label}>
      {children}
    </div>
  );
}

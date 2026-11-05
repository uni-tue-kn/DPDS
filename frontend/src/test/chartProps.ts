// The chart stubs of setup.ts keep the props of what they render here, so a test can reach what JSON cannot carry: the
// Chart.js tooltip callbacks (functions are dropped from `data-options`). Spec 35: the Doughnut's hover text.
export interface StubProps {
  data?: unknown;
  options?: unknown;
}

const byElement = new WeakMap<Element, StubProps>();

/** Called by the stubs' ref callback with the element and the props of the latest render. */
export function remember(el: Element | null, props: StubProps): void {
  if (el) byElement.set(el, props);
}

/** The props of the latest render of the chart stub `el` (a `chart-*` element). */
export function propsOf(el: Element): StubProps {
  const p = byElement.get(el);
  if (!p) throw new Error('no chart stub props remembered for this element');
  return p;
}

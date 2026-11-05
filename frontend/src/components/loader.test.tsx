// Spec 54 L2 (user 2026-11-05: "display some loading like P4TG does ... a 'DPDS' logo as text below the loading"): the Loader
// component, a port of P4TG's Loader.tsx (components/Loader.tsx, default export `Loader({ children, loaded = true, overlay = false })`).
// Its styles are CSS classes in index.css (assets/stylesSpec54.test.ts). The class names are the tester's CHOICE (the spec only says
// "as CSS classes"); they mirror P4TG's styled components:
//   .loader-content       P4TG ContentWrapper (position: relative), the root; holds the children, the block and the overlay
//   .loader-block         P4TG LoadWrapper, the non-overlay loader (`loaded = false`); replaces the children
//   .loader-overlay       P4TG LoadWrapperAbsolute, the layer over the children (`overlay`)
//   .loader-spinner-box   P4TG SpinnerWrapper, in the block and in the overlay
//   .loader-spinner       P4TG StyledSpinner, the react-bootstrap `<Spinner animation="border">` (.spinner-border), role "status", label "Loading"
//   .loader-text          the wordmark "DPDS" instead of P4TG's logo image, inside a <p> below the spinner, in the spinner box
import { render, screen } from '@testing-library/react';
import { must } from '../test/must';
import Loader from './Loader';

const q = (sel: string) => document.querySelector(sel);

function expectLoaderParts(box: Element | null) {
  expect(box).not.toBeNull();
  const spinnerBox = box?.querySelector('.loader-spinner-box');
  expect(spinnerBox).not.toBeNull();
  const spinner = spinnerBox?.querySelector('.spinner-border.loader-spinner');
  expect(spinner).not.toBeNull();
  const text = spinnerBox?.querySelector('p > .loader-text');
  expect(text).not.toBeNull();
  expect(text?.textContent).toBe('DPDS');
  // below the spinner: the spinner comes first in document order
  expect(must(spinner).compareDocumentPosition(must(text))).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  // no logo image, the text is the logo
  expect(box?.querySelector('img')).toBeNull();
}

describe('Loader (spec 54 L2)', () => {
  it('loaded (the default) renders its children inside .loader-content and nothing else', () => {
    const { container } = render(
      <Loader>
        <span data-testid="kid">content</span>
      </Loader>,
    );
    const root = container.firstElementChild;
    expect(root).toHaveClass('loader-content');
    expect(root).toContainElement(screen.getByTestId('kid'));
    expect(q('.loader-block')).toBeNull();
    expect(q('.loader-overlay')).toBeNull();
    expect(q('.spinner-border')).toBeNull();
    expect(screen.queryByText('DPDS')).not.toBeInTheDocument();
  });

  it('loaded={false} replaces the children by the centred block: spinner, then "DPDS" in a paragraph', () => {
    const { container } = render(
      <Loader loaded={false}>
        <span data-testid="kid">content</span>
      </Loader>,
    );
    expect(screen.queryByTestId('kid')).not.toBeInTheDocument();
    const root = container.firstElementChild;
    expect(root).toHaveClass('loader-content');
    const block = q('.loader-block');
    expect(root).toContainElement(must(block) as HTMLElement);
    expectLoaderParts(block);
    expect(q('.loader-overlay')).toBeNull();
  });

  it('overlay keeps the children and adds the layer after them, in the same root', () => {
    const { container } = render(
      <Loader overlay>
        <span data-testid="kid">content</span>
      </Loader>,
    );
    const root = must(container.firstElementChild);
    expect(root).toHaveClass('loader-content');
    expect(root).toContainElement(screen.getByTestId('kid'));
    const layer = must(q('.loader-overlay'));
    expect(root).toContainElement(layer as HTMLElement);
    expect(layer).not.toContainElement(screen.getByTestId('kid'));
    expect(screen.getByTestId('kid').compareDocumentPosition(layer)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expectLoaderParts(layer);
    expect(q('.loader-block')).toBeNull();
  });

  it('overlay={false} is the loaded case', () => {
    render(
      <Loader overlay={false}>
        <span data-testid="kid">content</span>
      </Loader>,
    );
    expect(screen.getByTestId('kid')).toBeInTheDocument();
    expect(q('.loader-overlay')).toBeNull();
    expect(q('.spinner-border')).toBeNull();
  });

  it('like P4TG, loaded={false} with overlay shows both the block and the layer (and not the children)', () => {
    render(
      <Loader loaded={false} overlay>
        <span data-testid="kid">content</span>
      </Loader>,
    );
    expect(screen.queryByTestId('kid')).not.toBeInTheDocument();
    expectLoaderParts(q('.loader-block'));
    expectLoaderParts(q('.loader-overlay'));
  });

  it('the spinner is a border spinner with the accessible label "Loading"', () => {
    render(<Loader loaded={false}>x</Loader>);
    const spinner = screen.getByRole('status', { name: /^Loading/ });
    expect(spinner).toHaveClass('spinner-border');
    expect(spinner).toHaveClass('loader-spinner');
  });

  it('every spinner has the label: the overlay has one, and so do both with loaded={false} overlay', () => {
    const { rerender } = render(<Loader overlay>x</Loader>);
    expect(screen.getAllByRole('status', { name: /^Loading/ })).toHaveLength(1);
    rerender(
      <Loader loaded={false} overlay>
        x
      </Loader>,
    );
    expect(screen.getAllByRole('status', { name: /^Loading/ })).toHaveLength(2);
  });

  it('the children come and go with `loaded` without a second root', () => {
    const { container, rerender } = render(<Loader loaded={false}>x</Loader>);
    expect(container.children).toHaveLength(1);
    rerender(<Loader>x</Loader>);
    expect(container.children).toHaveLength(1);
    expect(container).toHaveTextContent('x');
    expect(q('.loader-block')).toBeNull();
  });
});

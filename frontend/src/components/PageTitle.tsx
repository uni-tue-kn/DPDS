import logo from '../assets/logo.png';

/** The page's h1: the DPDS logo, with the page name as its visually hidden accessible name. */
export default function PageTitle({ title, className }: { title: string; className?: string }) {
  return (
    <h1 className={className}>
      <img src={logo} alt="" className="page-logo" draggable={false} />
      <span className="visually-hidden">{title}</span>
    </h1>
  );
}

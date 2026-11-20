/** The page's h1: visually hidden, it gives the page its accessible name. */
export default function PageTitle({ title, className }: { title: string; className?: string }) {
  return (
    <h1 className={className}>
      <span className="visually-hidden">{title}</span>
    </h1>
  );
}

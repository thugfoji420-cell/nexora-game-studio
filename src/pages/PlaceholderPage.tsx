export function PlaceholderPage({ description }: { description: string }) {
  return (
    <section className="empty-state">
      <div className="empty-state__glyph">NX</div>
      <h2>Workspace reserved</h2>
      <p>{description}</p>
      <span>No functionality is enabled in Phase 1.</span>
    </section>
  );
}

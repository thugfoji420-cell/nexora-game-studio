export function DiagnosticsPage({ logLocation }: { logLocation: string }) {
  return (
    <section className="diagnostic-card">
      <div className="health-line"><span className="status-dot" /><div><strong>Application core available</strong><p>The Phase 1 command boundary initialized successfully.</p></div></div>
      <div className="path-block"><span>ACTIVE LOG</span><code>{logLocation}</code></div>
      <p className="muted">Logs use JSON Lines format and remain on this Windows account. Full diagnostic collection is not implemented in Phase 1.</p>
    </section>
  );
}

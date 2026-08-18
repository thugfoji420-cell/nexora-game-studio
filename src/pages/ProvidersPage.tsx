import { useEffect, useState } from "react";
import {
  canRunProviderDiagnostic,
  capabilities,
  createProviderDiagnosticJob,
  diagnosticCapability,
  filterProvidersByCapability,
  formatCapability,
  formatMemory,
  formatUnknownHardwareValue,
  getHardwareSnapshot,
  getRuntime,
  initializeRuntimes,
  listProviders,
  listRuntimes,
  presentFit,
  presentFitReason,
  presentHealth,
  presentLicense,
  refreshHardwareSnapshot,
  refreshProviderHealth,
  RuntimeState,
  RuntimeStatus,
} from "../services/providers";
import type { Capability, HardwareSnapshot, ProviderView } from "../types/core";

const presentError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "The provider operation could not be completed.";
};

const formatIdentifier = (value: string): string =>
  value.replace(/([a-z])([A-Z])/g, "$1 $2").replaceAll("_", " ").replace(/^./, (letter) => letter.toUpperCase());

const formatCheckedAt = (value: string): string => {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
};

export function ProvidersPage() {
  const [hardware, setHardware] = useState<HardwareSnapshot | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [runtimes, setRuntimes] = useState<RuntimeState[]>([]);
  const [capability, setCapability] = useState<Capability | "">("");
  const [loading, setLoading] = useState(true);
  const [refreshingHardware, setRefreshingHardware] = useState(false);
  const [refreshingHealth, setRefreshingHealth] = useState(false);
  const [initializingRuntimes, setInitializingRuntimes] = useState(false);
  const [diagnosticProviderId, setDiagnosticProviderId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  useEffect(() => {
    let mounted = true;
    Promise.all([getHardwareSnapshot(), listProviders(), listRuntimes()])
      .then(([nextHardware, nextProviders, nextRuntimes]) => {
        if (!mounted) return;
        setHardware(nextHardware);
        setProviders(nextProviders);
        setRuntimes(nextRuntimes);
      })
      .catch((nextError: unknown) => {
        if (mounted) setError(presentError(nextError));
      })
      .finally(() => {
        if (mounted) setLoading(false);
      });
    return () => { mounted = false; };
  }, []);

  const handleHardwareRefresh = async () => {
    setRefreshingHardware(true);
    setError("");
    setMessage("");
    try {
      const nextHardware = await refreshHardwareSnapshot();
      const nextProviders = await listProviders();
      setHardware(nextHardware);
      setProviders(nextProviders);
    } catch (nextError) {
      setError(presentError(nextError));
    } finally {
      setRefreshingHardware(false);
    }
  };

  const handleHealthRefresh = async () => {
    setRefreshingHealth(true);
    setError("");
    setMessage("");
    try {
      await refreshProviderHealth();
      setProviders(await listProviders());
    } catch (nextError) {
      setError(presentError(nextError));
    } finally {
      setRefreshingHealth(false);
    }
  };

  const handleInitializeRuntimes = async () => {
    setInitializingRuntimes(true);
    setError("");
    setMessage("");
    try {
      const results = await initializeRuntimes();
      const updatedRuntimes = await Promise.all(
        results.map(async ([runtimeId]) => await getRuntime(runtimeId))
      );
      setRuntimes(updatedRuntimes.filter((r): r is RuntimeState => r !== null));
      setMessage("Runtime initialization completed.");
    } catch (nextError) {
      setError(presentError(nextError));
    } finally {
      setInitializingRuntimes(false);
    }
  };

  const handleDiagnostic = async (provider: ProviderView) => {
    const selectedCapability = diagnosticCapability(provider);
    if (!selectedCapability) return;
    setDiagnosticProviderId(provider.manifest.providerId);
    setError("");
    setMessage("");
    try {
      const job = await createProviderDiagnosticJob({
        providerId: provider.manifest.providerId,
        capability: selectedCapability,
        durationMs: 1000,
      });
      setMessage(`Diagnostic job ${job.jobId} was ${job.status}. Open Jobs to monitor it.`);
    } catch (nextError) {
      setError(presentError(nextError));
    } finally {
      setDiagnosticProviderId(null);
    }
  };

  const visibleProviders = filterProvidersByCapability(providers, capability || undefined);

  const formatRuntimeStatus = (status: RuntimeStatus): { label: string; tone: "good" | "warning" | "bad" | "muted" } => {
    switch (status) {
      case "ready": return { label: "Ready", tone: "good" };
      case "starting": return { label: "Starting...", tone: "warning" };
      case "stopped": return { label: "Stopped", tone: "muted" };
      case "notInstalled": return { label: "Not Installed", tone: "bad" };
      case "notConfigured": return { label: "Not Configured", tone: "muted" };
      case "degraded": return { label: "Degraded", tone: "warning" };
      case "failed": return { label: "Failed", tone: "bad" };
      default: return { label: "Unknown", tone: "muted" };
    }
  };

  return (
    <section className="providers-page">
      <div className="providers-toolbar">
        <div>
          <div className="panel-label">LOCAL EXECUTION REGISTRY</div>
          <h2>Provider Manager</h2>
          <p>Inspect detected hardware, provider readiness, compatibility, and licensing.</p>
        </div>
        <div className="providers-toolbar__actions">
          <label>Capability<select value={capability} onChange={(event) => setCapability(event.currentTarget.value as Capability | "")}><option value="">All capabilities</option>{capabilities.map((item) => <option value={item} key={item}>{formatCapability(item)}</option>)}</select></label>
          <button className="btn btn--secondary" type="button" disabled={refreshingHardware} onClick={() => void handleHardwareRefresh()}>{refreshingHardware ? "Probing..." : "Refresh hardware"}</button>
          <button className="btn btn--secondary" type="button" disabled={refreshingHealth} onClick={() => void handleHealthRefresh()}>{refreshingHealth ? "Checking..." : "Refresh health"}</button>
        </div>
      </div>

      {error && <div className="error-banner" role="alert">{error}</div>}
      {message && <div className="success-banner" role="status">{message}</div>}

      <section className="hardware-panel" aria-labelledby="hardware-title">
        <div className="hardware-panel__header"><div><div className="panel-label">HARDWARE SNAPSHOT</div><h3 id="hardware-title">This machine</h3></div>{hardware && <small>Captured {formatCheckedAt(hardware.capturedAt)} · {formatIdentifier(hardware.confidence)} confidence</small>}</div>
        {loading && !hardware ? <div className="loading-placeholder">Loading hardware...</div> : hardware ? (
          <div className="hardware-grid">
            <dl className="hardware-stat"><dt>System</dt><dd><strong>{formatUnknownHardwareValue(hardware.os)}</strong><span>{formatUnknownHardwareValue(hardware.arch)}</span></dd></dl>
            <dl className="hardware-stat"><dt>CPU</dt><dd><strong>{formatUnknownHardwareValue(hardware.cpuModel)}</strong><span>{hardware.cpuLogicalCount > 0 ? hardware.cpuLogicalCount : "Unknown"} logical / {hardware.cpuPhysicalCount ?? "Unknown"} physical cores</span></dd></dl>
            <dl className="hardware-stat"><dt>Memory</dt><dd><strong>{formatMemory(hardware.ramTotalMib)}</strong><span>Total system RAM</span></dd></dl>
            <dl className="hardware-stat"><dt>Project disk</dt><dd><strong>{formatMemory(hardware.activeProjectDiskFreeMib)}</strong><span>{hardware.activeProjectDiskFreeMib === null ? "No open project or probe unavailable" : "Free space"}</span></dd></dl>
            <div className="hardware-gpus"><div className="hardware-gpus__label">GPU</div>{hardware.gpus.length === 0 ? <p>Not detected / probe unavailable</p> : hardware.gpus.map((gpu, index) => <article key={`${gpu.name}-${index}`}><strong>{formatUnknownHardwareValue(gpu.name)}</strong><span>{formatUnknownHardwareValue(gpu.vendor)} · VRAM {formatMemory(gpu.vramTotalMib)} · Free {formatMemory(gpu.vramFreeMib)}</span><small>Driver {formatUnknownHardwareValue(gpu.driver)} · {formatIdentifier(gpu.confidence)} confidence</small></article>)}</div>
          </div>
        ) : <div className="loading-placeholder">Hardware snapshot unavailable.</div>}
      </section>

      <section className="runtime-panel" aria-labelledby="runtime-title">
        <div className="runtime-panel__header"><div><div className="panel-label">LOCAL AI RUNTIMES</div><h3 id="runtime-title">Runtime Status</h3></div><button className="btn btn--primary" type="button" disabled={initializingRuntimes} onClick={() => void handleInitializeRuntimes()}>{initializingRuntimes ? "Initializing..." : "Initialize Runtimes"}</button></div>
        {runtimes.length === 0 ? <div className="loading-placeholder">No runtimes configured.</div> : (
          <div className="runtime-grid">
            {runtimes.map((runtime) => {
              const status = formatRuntimeStatus(runtime.status);
              return (
                <article className="runtime-card" key={runtime.config.runtimeId}>
                  <header className="runtime-card__header">
                    <div>
                      <h3>{runtime.config.displayName}</h3>
                      <code>{runtime.config.runtimeId} · {runtime.config.runtimeType}</code>
                    </div>
                    <span className={`status-badge status-badge--provider-${status.tone}`}>{status.label}</span>
                  </header>
                  <div className="runtime-card__meta">
                    <span>Kind: {runtime.config.kind}</span>
                    <span>Auto-start: {runtime.config.autoStart ? "Yes" : "No"}</span>
                    <span>Started by Nexora: {runtime.startedByNexora ? "Yes" : "No"}</span>
                    {runtime.processId && <span>PID: {runtime.processId}</span>}
                  </div>
                  {runtime.config.baseUrl && <div className="runtime-detail"><dt>Endpoint</dt><dd><code>{runtime.config.baseUrl}</code></dd></div>}
                  {runtime.config.installPath && <div className="runtime-detail"><dt>Install Path</dt><dd><code>{runtime.config.installPath}</code></dd></div>}
                  {runtime.lastHealthCheck && <div className="runtime-detail"><dt>Last Health Check</dt><dd>{formatCheckedAt(runtime.lastHealthCheck)}</dd></div>}
                  {runtime.readinessTimeMs && <div className="runtime-detail"><dt>Readiness Time</dt><dd>{(runtime.readinessTimeMs / 1000).toFixed(1)}s</dd></div>}
                  {runtime.error && <div className="runtime-detail"><dt>Error</dt><dd className="error">{runtime.error}</dd></div>}
                </article>
              );
            })}
          </div>
        )}
      </section>

      <div className="provider-section-heading"><div><div className="panel-label">REGISTERED PROVIDERS</div><h3>{visibleProviders.length} {visibleProviders.length === 1 ? "provider" : "providers"}</h3></div></div>
      {loading ? <div className="providers-panel loading-placeholder">Loading providers...</div> : visibleProviders.length === 0 ? <div className="providers-panel empty-state empty-state--inline"><div className="empty-state__glyph">PRV</div><p>No providers match this capability.</p></div> : (
        <div className="provider-grid">
          {visibleProviders.map((provider) => {
            const health = presentHealth(provider.health.state);
            const fit = presentFit(provider.fit.status);
            const eligible = canRunProviderDiagnostic(provider);
            const selectedCapability = diagnosticCapability(provider);
            return (
              <article className="provider-card" key={provider.manifest.providerId}>
                <header className="provider-card__header"><div><h3>{provider.manifest.displayName}</h3><code>{provider.manifest.providerId} · v{provider.manifest.version}</code></div><div className="provider-badges"><span className={`status-badge status-badge--provider-${health.tone}`}>{health.label}</span><span className={`status-badge status-badge--provider-${fit.tone}`}>{fit.label}</span></div></header>
                <div className="provider-card__meta"><span>{formatIdentifier(provider.manifest.providerType)}</span><span>{formatIdentifier(provider.manifest.executionMode)}</span><span>{formatIdentifier(provider.manifest.classification)}</span><span>{provider.manifest.enabled ? "Enabled" : "Disabled"}</span></div>
                <div className="capability-list">{provider.manifest.capabilities.map((item) => <span key={item}>{formatCapability(item)}</span>)}</div>
                <dl className="provider-details">
                  <div><dt>Health check</dt><dd>{formatIdentifier(provider.manifest.healthCheck)} · {formatCheckedAt(provider.health.checkedAt)}</dd></div>
                  {provider.health.detail && <div><dt>Health detail</dt><dd>{provider.health.detail}</dd></div>}
                  <div><dt>Requirements</dt><dd>{formatIdentifier(provider.manifest.requirements.resourceClass)} resources · {provider.manifest.requirements.gpuRequired ? "GPU required" : "GPU optional"}{provider.manifest.requirements.exclusive ? " · Exclusive" : ""}</dd></div>
                  <div><dt>License</dt><dd>{presentLicense(provider.license)}{provider.license.sourceReference && <small>{provider.license.sourceReference}</small>}</dd></div>
                  <div><dt>Permissions</dt><dd>{provider.manifest.permissions.length ? provider.manifest.permissions.map(formatIdentifier).join(", ") : "None"}</dd></div>
                </dl>
                {provider.fit.reasonCodes.length > 0 && <ul className="provider-warnings">{provider.fit.reasonCodes.map((reason) => <li key={reason}>{presentFitReason(reason)}</li>)}</ul>}
                <footer className="provider-card__footer"><span>{eligible ? `Safe diagnostic: ${selectedCapability ? formatCapability(selectedCapability) : "Unavailable"}, 1000 ms` : "Safe diagnostic unavailable for this provider"}</span>{eligible && <button className="btn btn--primary" type="button" disabled={diagnosticProviderId === provider.manifest.providerId} onClick={() => void handleDiagnostic(provider)}>{diagnosticProviderId === provider.manifest.providerId ? "Starting..." : "Run diagnostic"}</button>}</footer>
              </article>
            );
          })}
        </div>
      )}
    </section>
  );
}

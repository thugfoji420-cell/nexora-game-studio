import { useEffect, useState, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  capabilities,
  formatCapability,
  formatMemory,
  formatUnknownHardwareValue,
  getHardwareSnapshot,
  getRuntime,
  getA1111Config,
  saveA1111Config,
  getHunyuanConfig,
  saveHunyuanConfig,
  initializeRuntimes,
  listProviders,
  listRuntimes,
  presentHealth,
  refreshHardwareSnapshot,
  refreshProviderHealth,
  RuntimeState,
  RuntimeStatus,
} from "../services/providers";
import {
  getOpenRouterConfig,
  testOpenRouterConnection,
} from "../services/openrouter";
import type {
  Capability,
  HardwareSnapshot,
  MaskedOpenRouterConfig,
  ProviderView,
  RuntimeStatus as RuntimeStatusType,
  A1111Config,
  HunyuanConfig,
} from "../types/core";

const presentError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "The operation could not be completed.";
};

const formatCheckedAt = (value: string): string => {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleTimeString();
};

type EngineStatus = "ready" | "attention" | "offline";

function getEngineStatus(runtime: RuntimeState | undefined): EngineStatus {
  if (!runtime) return "offline";
  switch (runtime.status) {
    case "ready": return "ready";
    case "starting":
    case "degraded": return "attention";
    default: return "offline";
  }
}

function getStatusLabel(status: EngineStatus): string {
  switch (status) {
    case "ready": return "Ready";
    case "attention": return "Needs Attention";
    case "offline": return "Offline";
  }
}

function getStatusClass(status: EngineStatus): string {
  switch (status) {
    case "ready": return "badge--success";
    case "attention": return "badge--warning";
    case "offline": return "badge--neutral";
  }
}

function getRuntimeStatusLabel(status: RuntimeStatus): string {
  switch (status) {
    case "ready": return "Operational";
    case "starting": return "Starting...";
    case "stopped": return "Stopped";
    case "notInstalled": return "Not Installed";
    case "notConfigured": return "Not Configured";
    case "degraded": return "Degraded";
    case "failed": return "Failed to Start";
    default: return "Unknown";
  }
}

export function ProvidersPage() {
  const [hardware, setHardware] = useState<HardwareSnapshot | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [runtimes, setRuntimes] = useState<RuntimeState[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [initializing, setInitializing] = useState(false);
  const [openRouterConfig, setOpenRouterConfig] = useState<MaskedOpenRouterConfig | null>(null);
  const [openRouterTesting, setOpenRouterTesting] = useState(false);
  const [openRouterStatus, setOpenRouterStatus] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [showDetails, setShowDetails] = useState<string | null>(null);
  const [a1111Config, setA1111Config] = useState<A1111Config | null>(null);
  const [hunyuanConfig, setHunyuanConfig] = useState<HunyuanConfig | null>(null);
  const [savingConfig, setSavingConfig] = useState<string | null>(null);
  const [configMessage, setConfigMessage] = useState("");

  const loadRuntimes = useCallback(async () => {
    try {
      const nextRuntimes = await listRuntimes();
      setRuntimes(nextRuntimes);
    } catch {
      // Ignore errors during auto-refresh
    }
  }, []);

  useEffect(() => {
    let mounted = true;
    Promise.all([getHardwareSnapshot(), listProviders(), listRuntimes(), getOpenRouterConfig(), getA1111Config(), getHunyuanConfig()])
      .then(([nextHardware, nextProviders, nextRuntimes, nextOpenRouterConfig, nextA1111Config, nextHunyuanConfig]) => {
        if (!mounted) return;
        setHardware(nextHardware);
        setProviders(nextProviders);
        setRuntimes(nextRuntimes);
        setOpenRouterConfig(nextOpenRouterConfig);
        setA1111Config(nextA1111Config);
        setHunyuanConfig(nextHunyuanConfig);
      })
      .catch((nextError: unknown) => {
        if (mounted) setError(presentError(nextError));
      })
      .finally(() => {
        if (mounted) setLoading(false);
      });

    const unlisten = listen<{ runtimeId: string; status: RuntimeStatusType }>(
      "runtime://status-changed",
      ({ payload }) => {
        setRuntimes((prev) =>
          prev.map((rt) =>
            rt.config.runtimeId === payload.runtimeId
              ? { ...rt, status: payload.status }
              : rt
          )
        );
      }
    );

    return () => {
      mounted = false;
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleRefresh = async () => {
    setRefreshing(true);
    setError("");
    setMessage("");
    try {
      const [nextHardware, nextProviders] = await Promise.all([
        refreshHardwareSnapshot(),
        refreshProviderHealth().then(() => listProviders()),
      ]);
      setHardware(nextHardware);
      setProviders(nextProviders);
      setMessage("Provider status refreshed.");
    } catch (nextError) {
      setError(presentError(nextError));
    } finally {
      setRefreshing(false);
    }
  };

  const handleInitialize = async () => {
    setInitializing(true);
    setError("");
    setMessage("");
    try {
      const results = await initializeRuntimes();
      const updatedRuntimes = await Promise.all(
        results.map(async ([runtimeId]) => await getRuntime(runtimeId))
      );
      setRuntimes(updatedRuntimes.filter((r): r is RuntimeState => r !== null));
      setMessage("Runtimes reinitialized.");
    } catch (nextError) {
      setError(presentError(nextError));
    } finally {
      setInitializing(false);
    }
  };

  const handleOpenRouterTest = async () => {
    setOpenRouterTesting(true);
    setOpenRouterStatus(null);
    try {
      const result = await testOpenRouterConnection();
      setOpenRouterStatus(
        result.state === "healthy"
          ? `Connected: ${result.detail ?? "OK"}`
          : `Unavailable: ${result.detail ?? "check configuration"}`
      );
    } catch (nextError) {
      setOpenRouterStatus(presentError(nextError));
    } finally {
      setOpenRouterTesting(false);
    }
  };

  // Calculate overall studio health
  const a1111 = runtimes.find(r => r.config.runtimeId === "automatic1111");
  const hunyuan = runtimes.find(r => r.config.runtimeId === "hunyuan3d");
  const blender = runtimes.find(r => r.config.runtimeId === "blender");

  const a1111Status = getEngineStatus(a1111);
  const hunyuanStatus = getEngineStatus(hunyuan);
  const blenderStatus = getEngineStatus(blender);

  const readyCount = [a1111Status, hunyuanStatus, blenderStatus].filter(s => s === "ready").length;
  const attentionCount = [a1111Status, hunyuanStatus, blenderStatus].filter(s => s === "attention").length;

  const overallStatus: EngineStatus = attentionCount > 0 ? "attention" : readyCount === 3 ? "ready" : "offline";

  // Get unique capabilities from providers
  const uniqueCapabilities = [...new Set(providers.flatMap(p => p.manifest.capabilities))];

  if (loading) {
    return (
      <div className="providers-page">
        <div className="loading-placeholder">Loading studio status...</div>
      </div>
    );
  }

  return (
    <section className="providers-page">
      {/* Studio Health Overview */}
      <div className="providers-health">
        <div className="providers-health__header">
          <div>
            <div className="panel-label">STUDIO HEALTH</div>
            <h2>Provider Status</h2>
          </div>
          <div className="providers-health__actions">
            <button 
              className="btn btn--secondary" 
              type="button" 
              disabled={refreshing} 
              onClick={() => void handleRefresh()}
            >
              {refreshing ? "Refreshing..." : "Refresh"}
            </button>
            <button 
              className="btn btn--primary" 
              type="button" 
              disabled={initializing} 
              onClick={() => void handleInitialize()}
            >
              {initializing ? "Initializing..." : "Reinitialize All"}
            </button>
          </div>
        </div>

        {error && <div className="error-banner" role="alert">{error}</div>}
        {message && <div className="success-banner" role="status">{message}</div>}

        <div className="providers-health__summary">
          <div className={`health-summary-card health-summary-card--${overallStatus}`}>
            <div className="health-summary-card__icon">
              {overallStatus === "ready" ? "✓" : overallStatus === "attention" ? "!" : "○"}
            </div>
            <div className="health-summary-card__info">
              <div className="health-summary-card__label">Overall Status</div>
              <div className="health-summary-card__value">{getStatusLabel(overallStatus)}</div>
            </div>
            <div className="health-summary-card__stats">
              <span>{readyCount} Ready</span>
              {attentionCount > 0 && <span>{attentionCount} Need Attention</span>}
            </div>
          </div>
        </div>
      </div>

      {/* Local Engines */}
      <div className="providers-section">
        <div className="providers-section__header">
          <div className="panel-label">LOCAL ENGINES</div>
          <h3>AI & 3D Services</h3>
        </div>

        <div className="engine-grid">
          {/* A1111 */}
          <div className={`engine-card engine-card--${a1111Status}`}>
            <div className="engine-card__header">
              <div className="engine-card__icon">🎨</div>
              <div className="engine-card__info">
                <h4>Automatic1111</h4>
                <p>Image Generation</p>
              </div>
              <span className={`badge ${getStatusClass(a1111Status)}`}>
                {getStatusLabel(a1111Status)}
              </span>
            </div>
            <div className="engine-card__details">
              <span>Stable Diffusion WebUI</span>
              <span>Port 7860</span>
              {a1111?.processId && <span>PID: {a1111.processId}</span>}
            </div>
            {a1111?.error && (
              <div className="engine-card__error">
                <p>{a1111.error}</p>
                <button className="btn btn--sm btn--secondary" onClick={() => void handleInitialize()}>
                  Retry
                </button>
              </div>
            )}
            <button 
              className="engine-card__details-toggle"
              onClick={() => setShowDetails(showDetails === "a1111" ? null : "a1111")}
            >
              {showDetails === "a1111" ? "Hide Details" : "View Details"}
            </button>
            {showDetails === "a1111" && a1111 && (
              <div className="engine-card__expanded">
                <div className="detail-row">
                  <span>Status</span>
                  <span>{getRuntimeStatusLabel(a1111.status)}</span>
                </div>
                <div className="detail-row">
                  <span>Auto-start</span>
                  <span>{a1111.config.autoStart ? "Yes" : "No"}</span>
                </div>
                <div className="detail-row">
                  <span>Started by Nexora</span>
                  <span>{a1111.startedByNexora ? "Yes" : "No"}</span>
                </div>
                {a1111.lastHealthCheck && (
                  <div className="detail-row">
                    <span>Last Health Check</span>
                    <span>{formatCheckedAt(a1111.lastHealthCheck)}</span>
                  </div>
                )}
                {a1111.readinessTimeMs && (
                  <div className="detail-row">
                    <span>Readiness Time</span>
                    <span>{(a1111.readinessTimeMs / 1000).toFixed(1)}s</span>
                  </div>
                )}
              </div>
            )}
          </div>

          {/* Hunyuan3D */}
          <div className={`engine-card engine-card--${hunyuanStatus}`}>
            <div className="engine-card__header">
              <div className="engine-card__icon">🧊</div>
              <div className="engine-card__info">
                <h4>Hunyuan3D</h4>
                <p>3D Generation</p>
              </div>
              <span className={`badge ${getStatusClass(hunyuanStatus)}`}>
                {getStatusLabel(hunyuanStatus)}
              </span>
            </div>
            <div className="engine-card__details">
              <span>Hunyuan3D-2mini</span>
              <span>Port 8081</span>
              {hunyuan?.processId && <span>PID: {hunyuan.processId}</span>}
            </div>
            {hunyuan?.error && (
              <div className="engine-card__error">
                <p>{hunyuan.error}</p>
                <button className="btn btn--sm btn--secondary" onClick={() => void handleInitialize()}>
                  Retry
                </button>
              </div>
            )}
            <button 
              className="engine-card__details-toggle"
              onClick={() => setShowDetails(showDetails === "hunyuan" ? null : "hunyuan")}
            >
              {showDetails === "hunyuan" ? "Hide Details" : "View Details"}
            </button>
            {showDetails === "hunyuan" && hunyuan && (
              <div className="engine-card__expanded">
                <div className="detail-row">
                  <span>Status</span>
                  <span>{getRuntimeStatusLabel(hunyuan.status)}</span>
                </div>
                <div className="detail-row">
                  <span>Auto-start</span>
                  <span>{hunyuan.config.autoStart ? "Yes" : "No"}</span>
                </div>
                <div className="detail-row">
                  <span>Started by Nexora</span>
                  <span>{hunyuan.startedByNexora ? "Yes" : "No"}</span>
                </div>
                {hunyuan.lastHealthCheck && (
                  <div className="detail-row">
                    <span>Last Health Check</span>
                    <span>{formatCheckedAt(hunyuan.lastHealthCheck)}</span>
                  </div>
                )}
                {hunyuan.readinessTimeMs && (
                  <div className="detail-row">
                    <span>Readiness Time</span>
                    <span>{(hunyuan.readinessTimeMs / 1000).toFixed(1)}s</span>
                  </div>
                )}
              </div>
            )}
          </div>

          {/* Blender */}
          <div className={`engine-card engine-card--${blenderStatus}`}>
            <div className="engine-card__header">
              <div className="engine-card__icon">🔧</div>
              <div className="engine-card__info">
                <h4>Blender</h4>
                <p>Mesh Processing</p>
              </div>
              <span className={`badge ${getStatusClass(blenderStatus)}`}>
                {getStatusLabel(blenderStatus)}
              </span>
            </div>
            <div className="engine-card__details">
              <span>Blender 5.2</span>
              <span>On-Demand</span>
            </div>
            {blender?.error && (
              <div className="engine-card__error">
                <p>{blender.error}</p>
              </div>
            )}
            <button 
              className="engine-card__details-toggle"
              onClick={() => setShowDetails(showDetails === "blender" ? null : "blender")}
            >
              {showDetails === "blender" ? "Hide Details" : "View Details"}
            </button>
            {showDetails === "blender" && blender && (
              <div className="engine-card__expanded">
                <div className="detail-row">
                  <span>Status</span>
                  <span>{getRuntimeStatusLabel(blender.status)}</span>
                </div>
                <div className="detail-row">
                  <span>Kind</span>
                  <span>On-Demand Executable</span>
                </div>
                {blender.lastHealthCheck && (
                  <div className="detail-row">
                    <span>Last Health Check</span>
                    <span>{formatCheckedAt(blender.lastHealthCheck)}</span>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Cloud Engines */}
      <div className="providers-section">
        <div className="providers-section__header">
          <div className="panel-label">CLOUD ENGINES</div>
          <h3>Remote Services</h3>
        </div>

        <div className="engine-card engine-card--cloud">
          <div className="engine-card__header">
            <div className="engine-card__icon">☁️</div>
            <div className="engine-card__info">
              <h4>OpenRouter</h4>
              <p>AI Model Gateway</p>
            </div>
            <span className={`badge ${openRouterConfig?.apiKeyConfigured ? "badge--success" : "badge--neutral"}`}>
              {openRouterConfig?.apiKeyConfigured ? "Configured" : "Not Configured"}
            </span>
          </div>
          <div className="engine-card__details">
            <span>Remote API</span>
            <span>{openRouterConfig?.defaultModel || "No default model"}</span>
          </div>
          {openRouterStatus && (
            <div className="engine-card__status">
              {openRouterStatus}
            </div>
          )}
          <div className="engine-card__actions">
            <button 
              className="btn btn--sm btn--secondary" 
              disabled={openRouterTesting}
              onClick={() => void handleOpenRouterTest()}
            >
              {openRouterTesting ? "Testing..." : "Test Connection"}
            </button>
            <button 
              className="btn btn--sm btn--secondary"
              onClick={() => window.location.hash = "settings"}
            >
              Configure
            </button>
          </div>
        </div>
      </div>

      {/* Capabilities */}
      <div className="providers-section">
        <div className="providers-section__header">
          <div className="panel-label">CAPABILITIES</div>
          <h3>Available Features</h3>
        </div>

        <div className="capabilities-grid">
          {uniqueCapabilities.slice(0, 12).map((cap) => (
            <div key={cap} className="capability-chip">
              {formatCapability(cap)}
            </div>
          ))}
          {uniqueCapabilities.length > 12 && (
            <div className="capability-chip capability-chip--more">
              +{uniqueCapabilities.length - 12} more
            </div>
          )}
        </div>
      </div>

      {/* Hardware Summary */}
      {hardware && (
        <div className="providers-section">
          <div className="providers-section__header">
            <div className="panel-label">HARDWARE</div>
            <h3>System Resources</h3>
          </div>

          <div className="hardware-summary">
            <div className="hardware-stat">
              <span className="hardware-stat__label">CPU</span>
              <span className="hardware-stat__value">{formatUnknownHardwareValue(hardware.cpuModel)}</span>
              <span className="hardware-stat__detail">{hardware.cpuLogicalCount} cores</span>
            </div>
            <div className="hardware-stat">
              <span className="hardware-stat__label">Memory</span>
              <span className="hardware-stat__value">{formatMemory(hardware.ramTotalMib)}</span>
              <span className="hardware-stat__detail">System RAM</span>
            </div>
            {hardware.gpus.length > 0 && (
              <div className="hardware-stat">
                <span className="hardware-stat__label">GPU</span>
                <span className="hardware-stat__value">{formatUnknownHardwareValue(hardware.gpus[0].name)}</span>
                <span className="hardware-stat__detail">VRAM {formatMemory(hardware.gpus[0].vramTotalMib)}</span>
              </div>
            )}
            {hardware.activeProjectDiskFreeMib !== null && (
              <div className="hardware-stat">
                <span className="hardware-stat__label">Disk</span>
                <span className="hardware-stat__value">{formatMemory(hardware.activeProjectDiskFreeMib)}</span>
                <span className="hardware-stat__detail">Free space</span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Engine Configuration */}
      <div className="providers-section">
        <div className="providers-section__header">
          <div className="panel-label">ENGINE CONFIGURATION</div>
          <h3>Local Engine Paths &amp; Runtime Settings</h3>
        </div>

        {a1111Config && (
          <div className="engine-config-panel">
            <div className="engine-config-panel__header">
              <strong>Automatic1111</strong>
              <span className={`badge ${savingConfig === "a1111" ? "badge--info" : "badge--neutral"}`}>
                {savingConfig === "a1111" ? "Saving..." : "Configured"}
              </span>
            </div>
            <div className="engine-config-grid">
              <label>
                Install path
                <input
                  type="text"
                  value={a1111Config.installPath ?? ""}
                  onChange={(e) => setA1111Config({ ...a1111Config, installPath: e.target.value || null })}
                  placeholder="C:\\AI\\stable-diffusion-webui"
                />
              </label>
              <label>
                Launcher
                <input
                  type="text"
                  value={a1111Config.launcherPath ?? ""}
                  onChange={(e) => setA1111Config({ ...a1111Config, launcherPath: e.target.value || null })}
                  placeholder="webui-user.bat"
                />
              </label>
              <label>
                Base URL
                <input
                  type="text"
                  value={a1111Config.baseUrl}
                  onChange={(e) => setA1111Config({ ...a1111Config, baseUrl: e.target.value })}
                />
              </label>
              <label>
                Auto-start
                <button
                  className={a1111Config.autoStart ? "toggle toggle--on" : "toggle"}
                  onClick={() => setA1111Config({ ...a1111Config, autoStart: !a1111Config.autoStart })}
                  type="button"
                  aria-pressed={a1111Config.autoStart}
                ><span /></button>
              </label>
            </div>
            <button
              className="btn btn--primary btn--sm"
              type="button"
              disabled={savingConfig === "a1111"}
              onClick={async () => {
                setSavingConfig("a1111");
                setConfigMessage("");
                try {
                  await saveA1111Config(a1111Config);
                  setConfigMessage("A1111 configuration saved.");
                } catch {
                  setConfigMessage("Failed to save A1111 configuration.");
                } finally {
                  setSavingConfig(null);
                }
              }}
            >
              Save A1111 Config
            </button>
          </div>
        )}

        {hunyuanConfig && (
          <div className="engine-config-panel">
            <div className="engine-config-panel__header">
              <strong>Hunyuan3D</strong>
              <span className={`badge ${savingConfig === "hunyuan" ? "badge--info" : "badge--neutral"}`}>
                {savingConfig === "hunyuan" ? "Saving..." : "Configured"}
              </span>
            </div>
            <div className="engine-config-grid">
              <label>
                Root path
                <input
                  type="text"
                  value={hunyuanConfig.rootPath ?? ""}
                  onChange={(e) => setHunyuanConfig({ ...hunyuanConfig, rootPath: e.target.value || null })}
                  placeholder="C:\\AI\\Hunyuan3D"
                />
              </label>
              <label>
                Python executable
                <input
                  type="text"
                  value={hunyuanConfig.pythonExecutable ?? ""}
                  onChange={(e) => setHunyuanConfig({ ...hunyuanConfig, pythonExecutable: e.target.value || null })}
                  placeholder=".venv\\Scripts\\python.exe"
                />
              </label>
              <label>
                Base URL
                <input
                  type="text"
                  value={hunyuanConfig.baseUrl}
                  onChange={(e) => setHunyuanConfig({ ...hunyuanConfig, baseUrl: e.target.value })}
                />
              </label>
              <label>
                Auto-start
                <button
                  className={hunyuanConfig.autoStart ? "toggle toggle--on" : "toggle"}
                  onClick={() => setHunyuanConfig({ ...hunyuanConfig, autoStart: !hunyuanConfig.autoStart })}
                  type="button"
                  aria-pressed={hunyuanConfig.autoStart}
                ><span /></button>
              </label>
              <label>
                Concurrency
                <input
                  type="number"
                  min="1"
                  max="4"
                  value={hunyuanConfig.concurrency}
                  onChange={(e) => setHunyuanConfig({ ...hunyuanConfig, concurrency: parseInt(e.target.value, 10) || 1 })}
                />
              </label>
              <label>
                Texture generation
                <button
                  className={hunyuanConfig.textureGeneration ? "toggle toggle--on" : "toggle"}
                  onClick={() => setHunyuanConfig({ ...hunyuanConfig, textureGeneration: !hunyuanConfig.textureGeneration })}
                  type="button"
                  aria-pressed={hunyuanConfig.textureGeneration}
                ><span /></button>
              </label>
            </div>
            <button
              className="btn btn--primary btn--sm"
              type="button"
              disabled={savingConfig === "hunyuan"}
              onClick={async () => {
                setSavingConfig("hunyuan");
                setConfigMessage("");
                try {
                  await saveHunyuanConfig(hunyuanConfig);
                  setConfigMessage("Hunyuan3D configuration saved.");
                } catch {
                  setConfigMessage("Failed to save Hunyuan3D configuration.");
                } finally {
                  setSavingConfig(null);
                }
              }}
            >
              Save Hunyuan3D Config
            </button>
          </div>
        )}
        {configMessage && <div className="success-banner" role="status">{configMessage}</div>}
      </div>
    </section>
  );
}

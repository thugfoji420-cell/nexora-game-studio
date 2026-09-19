import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import type { AppInfo } from "../types/core";
import type { AssetInfo } from "../types/core";
import {
  listRuntimes,
  checkRuntimeHealth,
  startRuntime,
  stopRuntime,
  checkAndUpdateEngines,
} from "../services/providers";
import { listAssets } from "../services/assets";
import type { RuntimeState, RuntimeStatus } from "../types/providers";
import { navigateTo, studioRoutes } from "./navigation";

export function HomePage({
  info,
}: {
  info: AppInfo;
}) {
  const [runtimes, setRuntimes] = useState<RuntimeState[]>([]);
  const [assets, setAssets] = useState<AssetInfo[]>([]);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  const refreshRuntimes = async () => {
    try {
      const data = await listRuntimes();
      setRuntimes(data);
    } catch {
      // ignore
    }
  };

  useEffect(() => {
    refreshRuntimes();
    checkRuntimeHealth("hunyuan3d").then(() => refreshRuntimes()).catch(() => {});
    checkRuntimeHealth("automatic1111").then(() => refreshRuntimes()).catch(() => {});
    checkRuntimeHealth("blender").then(() => refreshRuntimes()).catch(() => {});

    listAssets().then(setAssets).catch(() => {});

    let updateMounted = true;
    const runBackgroundEngineUpdate = () => {
      checkAndUpdateEngines()
        .then((updates) => {
          if (!updateMounted) return;
          const updated = updates.filter((update) => update.status === "updated");
          if (updated.length > 0) {
            setStatusMessage(`Background update complete: ${updated.map((update) => update.displayName).join(", ")}.`);
          }
        })
        .catch(() => {});
    };
    runBackgroundEngineUpdate();
    const updateInterval = window.setInterval(runBackgroundEngineUpdate, 6 * 60 * 60 * 1000);

    const unlistenPromise = listen<{ runtimeId: string; status: RuntimeStatus }>(
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

    // Other studios can start or health-check the same runtime while this
    // dashboard remains mounted in the background. Keep the health card in
    // sync instead of showing the old status captured at first mount.
    const refreshOnFocus = () => { void refreshRuntimes(); };
    window.addEventListener("focus", refreshOnFocus);
    const runtimePollInterval = window.setInterval(() => { void refreshRuntimes(); }, 2000);

    return () => {
      updateMounted = false;
      window.clearInterval(updateInterval);
      window.clearInterval(runtimePollInterval);
      window.removeEventListener("focus", refreshOnFocus);
      unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  const handleCheckHealth = async (runtimeId: string) => {
    setActionLoading(`check-${runtimeId}`);
    setStatusMessage(null);
    try {
      const status = await checkRuntimeHealth(runtimeId);
      setStatusMessage(`${runtimeId} health status: ${status.toUpperCase()}`);
      await refreshRuntimes();
    } catch (err) {
      setStatusMessage(`Error checking ${runtimeId}: ${String(err)}`);
    } finally {
      setActionLoading(null);
      setTimeout(() => setStatusMessage(null), 4000);
    }
  };

  const handleStartRuntime = async (runtimeId: string) => {
    setActionLoading(`start-${runtimeId}`);
    setStatusMessage(null);
    try {
      await startRuntime(runtimeId);
      setStatusMessage(`Started ${runtimeId} successfully.`);
      await refreshRuntimes();
    } catch (err) {
      setStatusMessage(`Failed to start ${runtimeId}: ${String(err)}`);
    } finally {
      setActionLoading(null);
      setTimeout(() => setStatusMessage(null), 4000);
    }
  };

  const handleStopRuntime = async (runtimeId: string) => {
    setActionLoading(`stop-${runtimeId}`);
    setStatusMessage(null);
    try {
      await stopRuntime(runtimeId);
      setStatusMessage(`Stopped ${runtimeId}.`);
      await refreshRuntimes();
    } catch (err) {
      setStatusMessage(`Failed to stop ${runtimeId}: ${String(err)}`);
    } finally {
      setActionLoading(null);
      setTimeout(() => setStatusMessage(null), 4000);
    }
  };

  const a1111Runtime = runtimes.find(
    (r) => r.config?.runtimeId === "automatic1111" || r.config?.runtimeId === "a1111"
  );
  const hunyuanRuntime = runtimes.find(
    (r) => r.config?.runtimeId === "hunyuan3d" || r.config?.runtimeId === "hunyuan"
  );
  const blenderRuntime = runtimes.find((r) => r.config?.runtimeId === "blender");

  const a1111Status = a1111Runtime?.status ?? "unknown";
  const hunyuanStatus = hunyuanRuntime?.status ?? "unknown";
  const blenderStatus = blenderRuntime?.status ?? "unknown";

  const totalAssetsCount = assets.length;
  const model3dCount = assets.filter((a) => a.mediaKind === "model3d").length;
  const imageCount = assets.filter((a) => a.mediaKind === "image").length;
  const videoCount = assets.filter((a) => a.mediaKind === "video").length;

  const providerList = [
    {
      id: "hunyuan3d",
      name: "Hunyuan 3D Engine",
      category: "3d",
      runtimeId: "hunyuan3d",
      version: "Hunyuan3D-2 Turbo Mini",
      status: hunyuanStatus,
      endpoint: hunyuanRuntime?.config?.baseUrl ?? "http://127.0.0.1:8081",
      description: "Direct standalone text-to-3D and image-to-3D synthesis for structural GLB generation.",
      capabilities: ["Text-to-3D", "Image-to-3D", "Structural GLB", "Direct Synthesis"],
      workspaceRoute: "model3d-generator" as const,
      workspaceLabel: "Open 3D Engine",
      icon: "🧊",
      managed: hunyuanRuntime?.startedByNexora ?? false,
    },
    {
      id: "automatic1111",
      name: "Automatic1111 Image Engine",
      category: "image",
      runtimeId: "automatic1111",
      version: "SD WebUI / SDXL",
      status: a1111Status,
      endpoint: a1111Runtime?.config?.baseUrl ?? "http://127.0.0.1:7860",
      description: "Local Stable Diffusion generation runtime for high-res concept art and game textures.",
      capabilities: ["Text-to-Image", "Image-to-Image", "ControlNet", "Concept Ideation"],
      workspaceRoute: studioRoutes.imageGeneration,
      workspaceLabel: "Open Concept Lab",
      icon: "🎨",
      managed: a1111Runtime?.startedByNexora ?? false,
    },
    {
      id: "blender",
      name: "Blender Mesh Pipeline",
      category: "mesh",
      runtimeId: "blender",
      version: "Blender 5.2.0 LTS",
      status: blenderStatus,
      endpoint: blenderRuntime?.config?.launcherPath ?? "C:\\Program Files\\Blender Foundation\\Blender 5.2\\blender.exe",
      description: "Automated headless 3D mesh decimation, quad-retopology, UV unwrapping, and LOD generation.",
      capabilities: ["LOD0-LOD2 Generation", "Quad Retopology", "UV Unwrapping", "Normal Recalculation"],
      workspaceRoute: studioRoutes.model3dReview,
      workspaceLabel: "Open Mesh Review",
      icon: "⚙️",
      managed: false,
    },
    {
      id: "wan_video",
      name: "Wan 2.1 Video Generator",
      category: "video",
      runtimeId: "wan_video",
      version: "Wan 2.1 T2V (1.3B Low-VRAM)",
      status: "ready",
      endpoint: "Local Pipeline Worker",
      description: "High-efficiency video generation transforming concept stills into motion sequences.",
      capabilities: ["Text-to-Video", "Motion Sequences", "VRAM Optimized"],
      workspaceRoute: studioRoutes.videoGeneration,
      workspaceLabel: "Open Video Generator",
      icon: "🎬",
      managed: true,
    },
  ];

  const isTabReady = (category: "3d" | "image" | "mesh" | "video") =>
    providerList.find((provider) => provider.category === category)?.status === "ready";

  const openEngine = async (category: "3d" | "image" | "mesh" | "video") => {
    const provider = providerList.find((item) => item.category === category);
    if (!provider) return;

    // Open the selected studio immediately. A cold 3D runtime can take time
    // to boot, so startup continues in the background instead of blocking UI
    // navigation.
    navigateTo(provider.workspaceRoute);

    // The 3D runtime is user-startable from the dashboard. Attempt startup
    // before entering the engine so a cold install does not look permanently
    // offline after the first click.
    if (category === "3d" && provider.status !== "ready") {
      setActionLoading("start-hunyuan3d");
      setStatusMessage("Starting 3D Engine...");
      try {
        await startRuntime("hunyuan3d");
        await checkRuntimeHealth("hunyuan3d");
        await refreshRuntimes();
        setStatusMessage("3D Engine started.");
      } catch (err) {
        setStatusMessage(`3D Engine could not start: ${String(err)}`);
      } finally {
        setActionLoading(null);
      }
    }
  };

  return (
    <div className="home-grid">
      {/* Main Area: Clean, focused Provider Control Hub */}
      <div className="home-main-col">
        {/* Hub Header */}
        <section className="studio-hero-premium" style={{ marginBottom: "var(--nx-space-xl)" }}>
          <div className="studio-hero-glow" />
          <div className="home-hero__badge studio-pulse-badge">
            <span className="status-indicator__dot studio-pulse-dot" />
            <span>AI ENGINES & PROVIDERS CONTROL HUB</span>
            <span className="studio-badge-pill">LOCAL-FIRST</span>
          </div>

          <h1 className="home-hero__title studio-3d-title" style={{ fontSize: "32px", marginBottom: "8px" }}>
            Production <span className="studio-gradient-text">Engines.</span>
          </h1>

          <p className="studio-hero-subtext" style={{ maxWidth: "680px", marginBottom: "16px" }}>
            Inspect, control, test health, and launch all visual AI generation runtimes freely.
            Every local provider operates independently with zero external cloud dependencies.
          </p>

          {/* Quick Engine Filter Tabs */}
          <div className="studio-mode-tabs" style={{ marginTop: "12px", marginBottom: 0 }}>
            <button
              className={`studio-tab ${isTabReady("3d") ? "studio-tab--ready" : "studio-tab--not-ready"}`}
              type="button"
              onClick={() => { void openEngine("3d"); }}
              title="Open the 3D Engine"
            >
              <span className="studio-tab__icon">🧊</span>
              <span className="studio-tab__text">
                <strong>3D Engine</strong>
                <small>Hunyuan3D-2</small>
              </span>
            </button>
            <button
              className={`studio-tab ${isTabReady("image") ? "studio-tab--ready" : "studio-tab--not-ready"}`}
              type="button"
              onClick={() => { void openEngine("image"); }}
              title="Open the Image Engine"
            >
              <span className="studio-tab__icon">🎨</span>
              <span className="studio-tab__text">
                <strong>Image Engine</strong>
                <small>Automatic1111</small>
              </span>
            </button>
            <button
              className={`studio-tab ${isTabReady("mesh") ? "studio-tab--ready" : "studio-tab--not-ready"}`}
              type="button"
              onClick={() => { void openEngine("mesh"); }}
              title="Open the Mesh Pipeline"
            >
              <span className="studio-tab__icon">⚙️</span>
              <span className="studio-tab__text">
                <strong>Mesh Pipeline</strong>
                <small>Blender 5.2</small>
              </span>
            </button>
            <button
              className={`studio-tab ${isTabReady("video") ? "studio-tab--ready" : "studio-tab--not-ready"}`}
              type="button"
              onClick={() => { void openEngine("video"); }}
              title="Open the Video Engine"
            >
              <span className="studio-tab__icon">🎬</span>
              <span className="studio-tab__text">
                <strong>Video Engine</strong>
                <small>Wan 2.1</small>
              </span>
            </button>
          </div>

          <div className="home-hero__projects-entry" style={{ marginTop: "16px" }}>
            <button
              className="btn btn--secondary"
              type="button"
              onClick={() => navigateTo(studioRoutes.projects)}
              title="Create, open, and manage all your Nexora projects across every engine"
            >
              <span>📁 Existing Projects</span>
              <span style={{ marginLeft: 8, opacity: 0.8 }}>Manage all projects →</span>
            </button>
          </div>
        </section>

        {statusMessage && (
          <div className="notice-banner" style={{ marginBottom: "var(--nx-space-md)" }}>
            <span>{statusMessage}</span>
          </div>
        )}

        {/* Freely Working Provider Cards Grid */}
        <div className="provider-interactive-grid">
          {providerList.map((provider) => {
            const isReady = provider.status === "ready";
            const isBusy =
              actionLoading === `check-${provider.runtimeId}` ||
              actionLoading === `start-${provider.runtimeId}` ||
              actionLoading === `stop-${provider.runtimeId}`;

            return (
              <div key={provider.id} className="provider-card-3d">
                <div className="provider-card-3d__header">
                  <div className="provider-card-3d__icon-title">
                    <span className="provider-card-3d__icon">{provider.icon}</span>
                    <div>
                      <h3 className="provider-card-3d__title">{provider.name}</h3>
                      <span className="provider-card-3d__sub">{provider.version}</span>
                    </div>
                  </div>
                  <div className="provider-card-3d__status">
                    <span
                      className={`badge ${
                        isReady ? "badge--success" : "badge--neutral"
                      }`}
                    >
                      <span
                        className={`status-indicator__dot ${
                          isReady ? "status-indicator--ready studio-pulse-dot" : "status-indicator--idle"
                        }`}
                        style={{ width: 7, height: 7, marginRight: 6 }}
                      />
                      {isReady ? "Ready" : provider.status === "unknown" ? "Standby" : "Offline"}
                    </span>
                  </div>
                </div>

                <p className="provider-card-3d__desc">{provider.description}</p>

                {/* Endpoint & Endpoint Info */}
                <div className="provider-card-3d__endpoint">
                  <span className="provider-endpoint-label">Endpoint:</span>
                  <code className="provider-endpoint-value">{provider.endpoint}</code>
                </div>

                {/* Capabilities Badges */}
                <div className="provider-card-3d__caps">
                  {provider.capabilities.map((cap) => (
                    <span key={cap} className="provider-cap-tag">
                      {cap}
                    </span>
                  ))}
                </div>

                {/* Direct Freely-Working Controls */}
                <div className="provider-card-3d__actions">
                  <button
                    className="btn btn--secondary btn--sm"
                    type="button"
                    disabled={isBusy}
                    onClick={() => handleCheckHealth(provider.runtimeId)}
                    title="Test engine connectivity and ping"
                  >
                    {actionLoading === `check-${provider.runtimeId}` ? "Checking..." : "🔄 Check Health"}
                  </button>

                  {provider.runtimeId !== "blender" && (
                    <button
                      className="btn btn--secondary btn--sm"
                      type="button"
                      disabled={isBusy}
                      onClick={() =>
                        isReady
                          ? handleStopRuntime(provider.runtimeId)
                          : handleStartRuntime(provider.runtimeId)
                      }
                      title={isReady ? "Stop engine process" : "Start local engine process"}
                    >
                      {isReady ? "⏹ Stop" : "▶ Start Engine"}
                    </button>
                  )}

                  <button
                    className="btn btn--primary btn--sm studio-btn-launch"
                    type="button"
                    onClick={() => navigateTo(provider.workspaceRoute)}
                  >
                    <span>{provider.workspaceLabel}</span>
                    <span style={{ fontSize: "14px" }}>→</span>
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Right Sidebar: EXACTLY the 4 requested items/bars */}
      <div className="home-sidebar">
        {/* 1. STUDIO HEALTH */}
        <div className="home-status-card studio-status-card-3d">
          <div className="home-status-card__header">
            <div className="home-status-card__title">STUDIO HEALTH</div>
            <span className="studio-live-indicator">LIVE</span>
          </div>
          <div className="home-status-card__body">
            <div className="status-row">
              <span className="status-row__label">
                <span className="status-indicator">
                  <span
                    className={`status-indicator__dot ${
                      a1111Status === "ready"
                        ? "status-indicator--ready studio-pulse-dot"
                        : "status-indicator--idle"
                    }`}
                  />
                  Image Engine
                </span>
              </span>
              <span className="status-row__value">
                <span
                  className={`badge ${
                    a1111Status === "ready" ? "badge--success" : "badge--neutral"
                  }`}
                >
                  {a1111Status === "ready" ? "Ready" : "Offline"}
                </span>
              </span>
            </div>

            <div className="status-row">
              <span className="status-row__label">
                <span className="status-indicator">
                  <span
                    className={`status-indicator__dot ${
                      hunyuanStatus === "ready"
                        ? "status-indicator--ready studio-pulse-dot"
                        : "status-indicator--idle"
                    }`}
                  />
                  3D Engine
                </span>
              </span>
              <span className="status-row__value">
                <span
                  className={`badge ${
                    hunyuanStatus === "ready" ? "badge--success" : "badge--neutral"
                  }`}
                >
                  {hunyuanStatus === "ready" ? "Ready" : "Offline"}
                </span>
              </span>
            </div>

            <div className="status-row">
              <span className="status-row__label">
                <span className="status-indicator">
                  <span
                    className={`status-indicator__dot ${
                      blenderStatus === "ready"
                        ? "status-indicator--ready studio-pulse-dot"
                        : "status-indicator--idle"
                    }`}
                  />
                  Mesh Pipeline
                </span>
              </span>
              <span className="status-row__value">
                <span
                  className={`badge ${
                    blenderStatus === "ready" ? "badge--success" : "badge--neutral"
                  }`}
                >
                  {blenderStatus === "ready" ? "Ready" : "Offline"}
                </span>
              </span>
            </div>
          </div>
        </div>

        {/* PRODUCTION TELEMETRY */}
        <div className="home-status-card studio-status-card-3d">
          <div className="home-status-card__header">
            <div className="home-status-card__title">PRODUCTION TELEMETRY</div>
          </div>
          <div className="home-status-card__body">
            <div className="status-row">
              <span className="status-row__label">3D Meshes</span>
              <span className="status-row__value studio-count-pill">{model3dCount}</span>
            </div>
            <div className="status-row">
              <span className="status-row__label">2D Concepts</span>
              <span className="status-row__value studio-count-pill">{imageCount}</span>
            </div>
            <div className="status-row">
              <span className="status-row__label">Videos</span>
              <span className="status-row__value studio-count-pill">{videoCount}</span>
            </div>
            <div className="status-row">
              <span className="status-row__label">Total Assets</span>
              <span className="status-row__value studio-count-pill studio-count-pill--highlight">{totalAssetsCount}</span>
            </div>
          </div>
        </div>

        {/* 4. APPLICATION */}
        <div className="home-status-card studio-status-card-3d">
          <div className="home-status-card__header">
            <div className="home-status-card__title">APPLICATION</div>
          </div>
          <div className="home-status-card__body">
            <div className="status-row">
              <span className="status-row__label">Version</span>
              <span className="status-row__value">v{info.version}</span>
            </div>
            <div className="status-row">
              <span className="status-row__label">Architecture</span>
              <span className="status-row__value">
                <span className="status-indicator">
                  <span className="status-indicator__dot status-indicator--ready" />
                  Local-First Studio
                </span>
              </span>
            </div>
            <div className="status-row">
              <span className="status-row__label">Storage</span>
              <span className="status-row__value">{info.localFirst ? "100% Local" : "Cloud"}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

import { useEffect, useState, useCallback, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { getOrchestratorStatus, OrchestratorStatus, ProviderState } from "../services/providers"
import { getSkipRuntimeStartup } from "../services/core";

interface InitLogEntry {
  timestamp: string;
  level: string;
  message: string;
  providerId?: string;
}

interface StartupScreenProps {
  onComplete: () => void;
  onContinueToStudio: () => void;
}

function getStateLabel(state: ProviderState): string {
  switch (state) {
    case "idle": return "IDLE";
    case "queued": return "QUEUED";
    case "starting": return "STARTING";
    case "initializing": return "INITIALIZING";
    case "health_checking": return "CHECKING";
    case "ready": return "READY";
    case "degraded": return "DEGRADED";
    case "failed": return "FAILED";
    case "stopping": return "STOPPING";
    case "stopped": return "STOPPED";
    default: return "UNKNOWN";
  }
}

function getStateClass(state: ProviderState): string {
  switch (state) {
    case "ready": return "badge--success";
    case "starting":
    case "initializing":
    case "health_checking": return "badge--info";
    case "queued": return "badge--neutral";
    case "failed": return "badge--error";
    case "degraded": return "badge--warning";
    default: return "badge--neutral";
  }
}

  function getProviderIcon(providerId: string): string {
  switch (providerId) {
    case "automatic1111": return "A";
    case "hunyuan3d": return "3D";
    case "blender": return "B";
    default: return "N";
  }
}

function formatTime(timestamp: string): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function formatElapsed(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes.toString().padStart(2, "0")}:${seconds.toString().padStart(2, "0")}`;
}

export function StartupScreen({ onComplete, onContinueToStudio }: StartupScreenProps) {
  const [orchestratorStatus, setOrchestratorStatus] = useState<OrchestratorStatus | null>(null);
  const [initLog, setInitLog] = useState<InitLogEntry[]>([]);
  const [showLog, setShowLog] = useState(false);
  const [showSuccess, setShowSuccess] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [initializationStartTime, setInitializationStartTime] = useState<number | null>(null);
  const [activeProviderStartTime, setActiveProviderStartTime] = useState<number | null>(null);
  const successTransitionRef = useRef(false);
  const successTimeoutRef = useRef<number | null>(null);
  const lastSeenCurrentProvider = useRef<string | null>(null);
  const initializationStartedRef = useRef(false);

  const refreshStatus = useCallback(async () => {
    try {
      const status = await getOrchestratorStatus();
      setOrchestratorStatus(status);
      
      // Check if all providers are done
      const allDone = status.providers.every(p => 
        p.state === "ready" || p.state === "degraded" || p.state === "failed"
      );
      
      if (allDone && !successTransitionRef.current) {
        successTransitionRef.current = true;
        setShowSuccess(true);
        successTimeoutRef.current = window.setTimeout(() => {
          onComplete();
        }, 900);
      }
    } catch (e) {
      console.error("Failed to get orchestrator status:", e);
    }
  }, [onComplete]);

  useEffect(() => {
    let mounted = true;

    const init = async () => {
      try {
        let shouldSkip = false;
        try {
          shouldSkip = await getSkipRuntimeStartup();
        } catch {
          shouldSkip = false;
        }

        if (shouldSkip && mounted) {
          onComplete();
          return;
        }

        await refreshStatus();
      } catch {
        // Continue anyway
      }
    };

    init();

    // Listen for orchestrator events
    const unlistenLog = listen<InitLogEntry>(
      "orchestrator://log",
      ({ payload }) => {
        if (mounted) {
          setInitLog(prev => [...prev, payload].slice(-100));
        }
      }
    );

    const unlistenProgress = listen<OrchestratorStatus>(
      "orchestrator://progress",
      ({ payload }) => {
        if (mounted) {
          setOrchestratorStatus(payload);
          
          const allDone = payload.providers.every(p => 
            p.state === "ready" || p.state === "degraded" || p.state === "failed"
          );
          
          if (allDone && !successTransitionRef.current) {
            successTransitionRef.current = true;
            setShowSuccess(true);
            successTimeoutRef.current = window.setTimeout(() => {
              onComplete();
            }, 900);
          }
        }
      }
    );

    // Poll status periodically
    const interval = setInterval(() => {
      if (mounted) refreshStatus();
    }, 1000);

    // Update clock for elapsed time display
    const tickInterval = setInterval(() => {
      if (mounted) setNow(Date.now());
    }, 1000);

    return () => {
      mounted = false;
      unlistenLog.then(fn => fn());
      unlistenProgress.then(fn => fn());
      clearInterval(interval);
      clearInterval(tickInterval);
      if (successTimeoutRef.current) clearTimeout(successTimeoutRef.current);
    };
  }, [refreshStatus, onComplete]);

  const providers = orchestratorStatus?.providers ?? [];
  const completedCount = orchestratorStatus?.completedCount ?? 0;
  const totalCount = orchestratorStatus?.totalCount ?? 3;
  const progressPercent = totalCount > 0 ? (completedCount / totalCount) * 100 : 0;
  const currentProvider = orchestratorStatus?.currentProvider;

  // Track initialization start time and active provider start time using refs (no re-render)
  if (!initializationStartedRef.current && providers.length > 0) {
    initializationStartedRef.current = true;
    setInitializationStartTime(Date.now());
  }

  if (
    providers.length > 0 &&
    currentProvider &&
    lastSeenCurrentProvider.current !== currentProvider
  ) {
    lastSeenCurrentProvider.current = currentProvider;
    setActiveProviderStartTime(Date.now());
  }

  // Find the current provider info
  const currentProviderInfo = currentProvider
    ? providers.find(p => p.providerId === currentProvider)
    : null;

  // Compute elapsed time
  const totalElapsedMs = initializationStartTime ? now - initializationStartTime : 0;
  const activeElapsedMs = activeProviderStartTime ? now - activeProviderStartTime : totalElapsedMs;

  if (showSuccess) {
    return (
      <div className="startup-screen startup-screen--success-active">
        <div className="startup-screen__bg" />
        <div className="startup-screen__perspective" aria-hidden="true">
          <div className="startup-screen__grid" />
          <div className="startup-screen__ambient">
            <span className="ambient-orb ambient-orb--a" />
            <span className="ambient-orb ambient-orb--b" />
            <span className="ambient-orb ambient-orb--c" />
          </div>
        </div>
        <div className="startup-screen__content">
          <div className="startup-screen__success">
            <div className="startup-screen__success-icon" aria-hidden="true">
              <span className="success-check">✓</span>
            </div>
            <h2>NEXORA</h2>
            <h3>STUDIO READY</h3>
            <p>Your creative environment is ready.</p>
            <button
              type="button"
              className="btn btn--primary btn--lg startup-screen__enter"
              onClick={onComplete}
            >
              Enter Studio
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="startup-screen">
      <div className="startup-screen__bg" />
      <div className="startup-screen__perspective" aria-hidden="true">
        <div className="startup-screen__grid" />
        <div className="startup-screen__ambient">
          <span className="ambient-orb ambient-orb--a" />
          <span className="ambient-orb ambient-orb--b" />
          <span className="ambient-orb ambient-orb--c" />
        </div>
      </div>
      <div className="startup-screen__content">
        <div className="startup-screen__hero" aria-hidden="true">
          <div className="startup-screen__core">
            <span className="core-ring core-ring--1" />
            <span className="core-ring core-ring--2" />
            <span className="core-ring core-ring--3" />
            <span className="core-particle core-particle--1" />
            <span className="core-particle core-particle--2" />
            <span className="core-particle core-particle--3" />
            <span className="core-particle core-particle--4" />
            <span className="core-sweep" />
          </div>
          <div className="startup-screen__logo">N</div>
        </div>
        
        <div className="startup-screen__brand">
          <h1>NEXORA</h1>
          <p>GAMING / CREATIVE STUDIO</p>
        </div>

        <div className="startup-screen__status">
          <div className="startup-screen__status-title">SYSTEM INITIALIZATION</div>
          
          <div className="startup-screen__providers">
            {providers.length > 0 ? (
              providers.map((provider) => (
                <div 
                  key={provider.providerId}
                  className={`startup-provider ${
                    provider.state === "ready" ? "startup-provider--ready" :
                    provider.state === "starting" || provider.state === "initializing" || provider.state === "health_checking" 
                      ? "startup-provider--active" : ""
                  }`}
                >
                  <div className="startup-provider__icon">
                    {getProviderIcon(provider.providerId)}
                  </div>
                  <div className="startup-provider__info">
                    <div className="startup-provider__name">{provider.displayName}</div>
                    <div className="startup-provider__status">
                      {provider.state === "ready" ? "Operational" :
                       provider.state === "starting" ? "Starting service..." :
                       provider.state === "initializing" ? "Initializing..." :
                       provider.state === "health_checking" ? "Checking health..." :
                       provider.state === "queued" ? "Waiting in queue" :
                       provider.state === "failed" ? provider.error || "Failed to start" :
                       getStateLabel(provider.state)}
                    </div>
                  </div>
                  <span className={`badge ${getStateClass(provider.state)}`}>
                    {getStateLabel(provider.state)}
                  </span>
                </div>
              ))
            ) : (
              <div className="startup-provider startup-provider--pending">
                <div className="startup-provider__icon">N</div>
                <div className="startup-provider__info">
                  <div className="startup-provider__name">Provider queue</div>
                  <div className="startup-provider__status">Waiting for orchestrator status...</div>
                </div>
                <span className="badge badge--neutral">QUEUED</span>
              </div>
            )}
          </div>
        </div>

        <div className="startup-screen__progress">
          <div className="startup-screen__progress-bar">
            <div 
              className="startup-screen__progress-fill" 
              style={{ width: `${progressPercent}%` }} 
            />
          </div>
          <div className="startup-screen__progress-text">
            <span className="startup-screen__progress-percent">
              {Math.round(progressPercent)}%
            </span>
            <span className="startup-screen__progress-divider">·</span>
            <span>{completedCount} of {totalCount} engines ready</span>
            {currentProviderInfo && (
              <>
                <span className="startup-screen__progress-divider">·</span>
                <span className="startup-screen__progress-status">
                  {getStateLabel(currentProviderInfo.state)}
                </span>
              </>
            )}
          </div>
        </div>

        {currentProviderInfo && (
          <div className="startup-screen__current" aria-live="polite">
            <div className="startup-screen__current-label">CURRENT ENGINE</div>
            <div className="startup-screen__current-name">
              {currentProviderInfo.displayName}
            </div>
            <div className="startup-screen__current-detail">
              {currentProviderInfo.state === "starting" && "Launching service..."}
              {currentProviderInfo.state === "initializing" && "Initializing creative engine..."}
              {currentProviderInfo.state === "health_checking" && "Checking service health..."}
              {currentProviderInfo.state === "queued" && "Waiting in queue..."}
              {currentProviderInfo.state === "ready" && "Ready"}
              {currentProviderInfo.state === "failed" && (currentProviderInfo.error || "Failed to start")}
              {activeElapsedMs > 0 && (
                <span className="startup-screen__current-elapsed">
                  {" · "}{formatElapsed(activeElapsedMs)} elapsed
                </span>
              )}
            </div>
          </div>
        )}

        {!currentProviderInfo && providers.length > 0 && (
          <div className="startup-screen__message">
            Preparing your creative workspace...
          </div>
        )}

        <button 
          className="startup-screen__log-toggle"
          onClick={() => setShowLog(!showLog)}
        >
          <span className="startup-screen__log-toggle-icon">{showLog ? "−" : "+"}</span>
          <span>{showLog ? "Hide" : "View"} Initialization Log ({initLog.length} entries)</span>
        </button>

        {showLog && (
          <div className="startup-screen__log">
            {initLog.length > 0 ? (
              initLog.map((entry, index) => (
                <div key={index} className={`startup-log-entry startup-log-entry--${entry.level}`}>
                  <span className="startup-log-entry__time">{formatTime(entry.timestamp)}</span>
                  <span className="startup-log-entry__message">{entry.message}</span>
                </div>
              ))
            ) : (
              <div className="startup-log-entry">
                <span className="startup-log-entry__time">--:--:--</span>
                <span className="startup-log-entry__message">Waiting for initialization events...</span>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

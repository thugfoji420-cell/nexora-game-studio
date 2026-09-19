import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { formatFileSize, listAssets, pickAssetFile, importAsset, getAssetPreview } from "../services/assets";
import { formatJobProgress, getJobDetails, listJobs, renderJobStatus, requestJobCancellation } from "../services/jobs";
import {
  completedHunyuanAssetIds,
  createHunyuanGenerationJob,
  getHunyuanGenerationResult,
  hunyuanCapabilityForMode,
  isEligibleHunyuanProvider,
  presentHunyuanGenerationError,
  recoverLatestHunyuanGenerationJob,
  shouldPollHunyuanGenerationJob,
  validateHunyuanGenerationRequest,
} from "../services/hunyuanGeneration";
import {
  createImageGenerationJob,
  getImageGenerationResult,
  shouldPollImageGenerationJob,
  completedAssetIds as completedImageAssetIds,
  presentImageGenerationError,
} from "../services/imageGeneration";
import {
  createModel3dProcessingJob,
  getModel3dProcessingResult,
  approveImageAsset,
  discoverUnityProject,
  validateUnityProject,
  deployModel3dToUnity,
  approveModel3dAsset,
  rejectModel3dAsset,
  reprocessModel3dAsset,
  canApproveAsset,
  canRejectAsset,
  canReprocessAsset,
  formatProcessingStatus,
} from "../services/model3dProcessing";
import type { Model3dProcessingProfile, Model3dProcessingResult, ProcessingReport, UnityDeploymentDto } from "../types/model3dProcessing";
import { checkRuntimeHealth, listProviders, listRuntimes, refreshProviderHealth } from "../services/providers";
import { Model3dViewer } from "../components/Model3dViewer";
import { AssetReadinessReport } from "../components/AssetReadinessReport";
import {
  canCancelVideoGenerationJob,
  createVideoGenerationJob,
  getVideoGenerationResult,
  isUsableVideoProvider,
  listVideoGenerationProviders,
  presentVideoGenerationError,
  shouldPollVideoGenerationJob,
  validateVideoGenerationRequest,
} from "../services/videoGeneration";
import {
  HUNYUAN_GENERATION_PROFILE_ID,
  HUNYUAN_OUTPUT_FORMAT,
  HUNYUAN_QUALITY,
  type AssetInfo,
  type JobInfo,
  type HunyuanGenerationMode,
  type HunyuanGenerationRequest,
  type ProviderView,
  type EngineProjectScope,
  type ProjectInfo,
} from "../types/core";
import {
  VIDEO_GENERATION_PROFILE_ID,
  VIDEO_PROVIDER_ID,
  type VideoGenerationRequest,
} from "../types/videoGeneration";
import type { RuntimeState, RuntimeStatus } from "../types/providers";

const POLL_INTERVAL_MS = 1000;

export type StudioMode = "full_circle" | "image_to_3d" | "text_to_3d" | "concept_2d" | "video_gen" | "review";
type AssetCreationStyle = "neon" | "normal";

export type FactoryStep = 
  | "prompt"
  | "concept_generating"
  | "concept_review"
  | "threed_generating"
  | "blender_processing"
  | "unity_ready"
  | "deploying"
  | "completed";

const STYLE_PRESETS = [
  { id: "realistic", label: "🎨 Realistic", promptSuffix: ", photorealistic, PBR materials, clean topology, production-ready game asset" },
  { id: "stylized", label: "🖌️ Stylized", promptSuffix: ", stylized, cel-shaded, clean shapes, vibrant colors, game-ready" },
  { id: "fantasy", label: "⚔️ Fantasy", promptSuffix: ", fantasy style, magical details, ornate design, video game asset" },
  { id: "scifi", label: "🚀 Sci-Fi", promptSuffix: ", sci-fi style, high-tech details, clean industrial design, 3D game asset" },
  { id: "anime", label: "✨ Anime", promptSuffix: ", anime style, cel-shaded, stylized proportions, game-ready 3D model" },
  { id: "horror", label: "🌑 Horror", promptSuffix: ", horror style, dark atmosphere, detailed textures, 3D game asset" },
  { id: "cartoon", label: "🎬 Cartoon", promptSuffix: ", cartoon style, exaggerated proportions, bright colors, game-ready" },
  { id: "historical", label: "🏛️ Historical", promptSuffix: ", historical accuracy, realistic materials, detailed craftsmanship, 3D asset" },
];

function detectAssetCategory(prompt: string): string {
  const p = prompt.toLowerCase();
  if (p.includes("gun") || p.includes("rifle") || p.includes("sword") || p.includes("blade") || p.includes("pistol") || p.includes("weapon") || p.includes("axe") || p.includes("shield") || p.includes("bow") || p.includes("staff")) return "Weapon";
  if (p.includes("car") || p.includes("vehicle") || p.includes("bike") || p.includes("ship") || p.includes("kart") || p.includes("rover") || p.includes("truck") || p.includes("racer") || p.includes("spacecraft") || p.includes("hovercraft") || p.includes("tank")) return "Vehicle";
  if (p.includes("character") || p.includes("soldier") || p.includes("warrior") || p.includes("robot") || p.includes("monster") || p.includes("creature") || p.includes("npc") || p.includes("hero") || p.includes("human") || p.includes("avatar") || p.includes("knight")) return "Character";
  if (p.includes("building") || p.includes("tower") || p.includes("house") || p.includes("castle") || p.includes("temple") || p.includes("ruin") || p.includes("bridge") || p.includes("barn") || p.includes("hangar")) return "Building";
  if (p.includes("tree") || p.includes("bush") || p.includes("grass") || p.includes("flower") || p.includes("plant") || p.includes("foliage") || p.includes("forest")) return "Vegetation";
  if (p.includes("chair") || p.includes("table") || p.includes("desk") || p.includes("couch") || p.includes("sofa") || p.includes("bed") || p.includes("cabinet") || p.includes("shelf") || p.includes("lamp")) return "Furniture";
  if (p.includes("tool") || p.includes("helmet") || p.includes("armor") || p.includes("backpack") || p.includes("battery") || p.includes("generator") || p.includes("equipment")) return "Equipment";
  if (p.includes("env") || p.includes("rock") || p.includes("boulder") || p.includes("terrain") || p.includes("road") || p.includes("mountain") || p.includes("dungeon") || p.includes("street") || p.includes("city") || p.includes("landscape") || p.includes("island") || p.includes("structure")) return "Environment";
  return "";
}

function promptForCreationStyle(prompt: string, style: AssetCreationStyle): string {
  const styleGuide = style === "neon"
    ? "neon cyberpunk game asset, luminous cyan and magenta accents, emissive details, futuristic hard-surface design"
    : "normal production game asset, natural materials, balanced colors, practical proportions, clean game-ready design";
  return `${prompt.trim()}, ${styleGuide}`;
}

// 3D reconstruction (Hunyuan3D) needs a clean single-object reference: object
// centered and fully visible, plain background, even lighting — any scene detail
// (streets, buildings, neon haze) becomes garbage geometry in the mesh. Directives
// go FIRST because diffusion models weight early tokens more strongly.
function buildConceptPrompt(prompt: string, style: AssetCreationStyle): string {
  const styleAccent = style === "neon"
    ? "sleek futuristic hard-surface design with subtle cyan and magenta emissive accents"
    : "natural materials, balanced colors, practical proportions";
  return [
    `game asset concept of a single ${prompt.trim()}`,
    "entire object fully visible in frame, centered, three-quarter view",
    "isolated on plain solid white background",
    "even neutral studio lighting, soft shadow under object only",
    "clean product shot, no environment",
    styleAccent,
  ].join(", ");
}

// Scene blockers merged into every concept negative prompt: these are the terms
// that keep A1111 from rendering a full environment around the asset.
const CONCEPT_NEGATIVE_EXTRAS =
  "environment, scenery, background buildings, street, road, ground plane, night city, multiple objects, cropped, out of frame, close-up, dramatic lighting, fog, haze, rain";

const initialRequest: HunyuanGenerationRequest = {
  schemaVersion: 1,
  mode: "text_to_3d",
  prompt: "",
  negativePrompt: "blurry, ugly, deformed, low quality, artifacts, distorted, noisy, watermark",
  sourceAssetId: null,
  profile: HUNYUAN_GENERATION_PROFILE_ID,
  quality: HUNYUAN_QUALITY,
  seed: null,
  outputFormat: HUNYUAN_OUTPUT_FORMAT,
};

export function HunyuanGeneratorPage({
  currentProject,
  onOpenProjectModal,
  onOpenProject,
}: {
  currentProject?: ProjectInfo | null;
  onOpenProjectModal?: (mode: "create" | "open", scope?: EngineProjectScope) => void;
  onOpenProject?: (root: string) => void;
} = {}) {
  const [studioMode, setStudioMode] = useState<StudioMode>("full_circle");
  const [assetCreationStyle, setAssetCreationStyle] = useState<AssetCreationStyle>("normal");
  const [factoryStep, setFactoryStep] = useState<FactoryStep>("prompt");
  const [request, setRequest] = useState(initialRequest);
  const [randomSeed, setRandomSeed] = useState(true);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [runtimes, setRuntimes] = useState<RuntimeState[]>([]);
  const [runtimeHealth, setRuntimeHealth] = useState<Record<string, RuntimeStatus>>({});
  const [assets, setAssets] = useState<AssetInfo[]>([]);
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [job, setJob] = useState<JobInfo | null>(null);
  
  // Pipeline Specific States
  const [conceptImageAssetId, setConceptImageAssetId] = useState<string | null>(null);
  const [conceptImagePreviewUrl, setConceptImagePreviewUrl] = useState<string | null>(null);
  const [raw3dAssetId, setRaw3dAssetId] = useState<string | null>(null);
  const [blenderReport, setBlenderReport] = useState<ProcessingReport | null>(null);
  const [processed3dAssetId, setProcessed3dAssetId] = useState<string | null>(null);
  const [final3dApproved, setFinal3dApproved] = useState(false);
  const [selectedAssetIdForViewer, setSelectedAssetIdForViewer] = useState<string | null>(null);
  
  // Unity Deployment States
  const [unityProjectRoot, setUnityProjectRoot] = useState<string>("");
  const [unityCategory, setUnityCategory] = useState<string>("");
  const [unityValidation, setUnityValidation] = useState<{ isValid: boolean; unityVersion: string | null; errorMessage?: string | null } | null>(null);
  const [deploymentResult, setDeploymentResult] = useState<UnityDeploymentDto | null>(null);

  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [importingImage, setImportingImage] = useState(false);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [error, setError] = useState("");
  const activeJobId = useRef<string | null>(null);
  // Completed-3D auto-continue must fire once per asset; the workspace load
  // loop re-runs on an interval and would otherwise retry a failed Blender
  // job creation on every poll.
  const blenderContinuedFor = useRef<string | null>(null);

  // Video generation state
  const [videoProviders, setVideoProviders] = useState<ProviderView[]>([]);
  const [videoConfig, setVideoConfig] = useState<VideoGenerationRequest | null>(null);
  const [videoJob, setVideoJob] = useState<JobInfo | null>(null);
  const [videoOutputs, setVideoOutputs] = useState<AssetInfo[]>([]);
  const [videoOutputAssetIds, setVideoOutputAssetIds] = useState<string[]>([]);
  const [videoGenerating, setVideoGenerating] = useState(false);
  const [videoCancelling, setVideoCancelling] = useState(false);
  const [videoSaving, setVideoSaving] = useState(false);
  const [videoValidationErrors, setVideoValidationErrors] = useState<string[]>([]);
  const [videoEnabled, setVideoEnabled] = useState(false);
  const [videoMessage, setVideoMessage] = useState("");
  const videoRequestVersion = useRef(0);
  const activeVideoJobId = useRef<string | null>(null);

  // Review state
  const [reviewJobs, setReviewJobs] = useState<JobInfo[]>([]);
  const [selectedReviewJobId, setSelectedReviewJobId] = useState<string | null>(null);
  const [reviewJobResult, setReviewJobResult] = useState<Model3dProcessingResult | null>(null);
  const [reviewActionLoading, setReviewActionLoading] = useState<string | null>(null);
  const [reviewDeploying, setReviewDeploying] = useState(false);
  const [reviewDeploymentResult, setReviewDeploymentResult] = useState<UnityDeploymentDto | null>(null);
  const [reviewDeploymentSuccess, setReviewDeploymentSuccess] = useState(false);

  const loadWorkspaceData = async () => {
    try {
      const [nextProviders, nextRuntimes, nextAssets, nextJobs, discoveredUnity, nextVideoProviders] = await Promise.all([
        listProviders().catch(() => []),
        listRuntimes().catch(() => []),
        listAssets().catch(() => []),
        listJobs().catch(() => []),
        discoverUnityProject().catch(() => null),
        listVideoGenerationProviders().catch(() => []),
      ]);
      // Runtime state can be stale when this route is opened directly. The
      // Providers page refreshes health as part of its own lifecycle, so do
      // the same here before rendering the studio status bar.
      const checkedRuntimeStatuses = await Promise.all([
        "automatic1111",
        "hunyuan3d",
        "blender",
      ].map(async (runtimeId) => [
        runtimeId,
        await checkRuntimeHealth(runtimeId).catch(() => "unavailable" as RuntimeStatus),
      ] as const));
      setRuntimeHealth(Object.fromEntries(checkedRuntimeStatuses));
      const refreshedRuntimes = await listRuntimes().catch(() => nextRuntimes);
      await refreshProviderHealth().catch(() => []);
      const refreshedProviders = await listProviders().catch(() => nextProviders);

      setProviders(refreshedProviders);
      setRuntimes(refreshedRuntimes);
      setAssets(nextAssets);
      setJobs(nextJobs);
      setVideoProviders(nextVideoProviders);

      // Jobs are durable in the project database. Reattach to an active job
      // when this workspace is first opened so route changes/reloads do not
      // make a running generation look lost.
      const recoveredGeneration = recoverLatestHunyuanGenerationJob(nextJobs);
      const recoveredProcessing = nextJobs
        .filter((item) => item.jobType === "model3d.processing" && (item.status === "queued" || item.status === "running"))
        .sort((a, b) => b.updatedAtMs - a.updatedAtMs)[0];
      const recovered = recoveredGeneration && (recoveredGeneration.status === "queued" || recoveredGeneration.status === "running")
        ? recoveredGeneration
        : recoveredProcessing;
      if (recovered) {
        activeJobId.current = recovered.jobId;
        setJob(recovered);
        setFactoryStep(recovered.jobType === "image.generate" ? "concept_generating" : recovered.jobType === "model3d.processing" ? "blender_processing" : "threed_generating");
      }

      // A completed 3D generation whose Blender stage never started (app
      // closed between pipeline stages) must not dead-end the user: continue
      // the pipeline from it. Only when it is still the latest pipeline job,
      // so already-processed/approved assets are not re-processed.
      const latestPipelineJob = nextJobs
        .filter((item) => item.jobType === "image.generate" || item.jobType === "hunyuan.generate" || item.jobType === "model3d.processing")
        .sort((a, b) => b.createdAtMs - a.createdAtMs)[0];
      if (!recovered && latestPipelineJob?.jobType === "hunyuan.generate" && latestPipelineJob.status === "completed") {
        try {
          const res = await getHunyuanGenerationResult(latestPipelineJob.jobId);
          const assetIds = completedHunyuanAssetIds(res);
          if (assetIds.length > 0 && blenderContinuedFor.current !== assetIds[0]) {
            blenderContinuedFor.current = assetIds[0];
            setRaw3dAssetId(assetIds[0]);
            void triggerBlenderProcessing(assetIds[0]);
          }
        } catch {
          // Stale result (asset removed); leave the user at the prompt.
        }
      }

      if (discoveredUnity && !unityProjectRoot) {
        setUnityProjectRoot(discoveredUnity);
        validateUnityProject(discoveredUnity).then(setUnityValidation).catch(() => {});
      }

      const model3dAssets = nextAssets.filter((a: AssetInfo) => a.mediaKind === "model3d" && a.status === "ready");
      if (model3dAssets.length > 0 && !selectedAssetIdForViewer) {
        setSelectedAssetIdForViewer(model3dAssets[0].assetId);
      }
    } catch (err) {
      console.warn("Failed to load workspace data:", err);
    }
  };

  useEffect(() => {
    let mounted = true;

    const passedPrompt = sessionStorage.getItem("nexora_initial_prompt");
    if (passedPrompt) {
      sessionStorage.removeItem("nexora_initial_prompt");
      setRequest((prev) => ({ ...prev, prompt: passedPrompt }));
      setUnityCategory(detectAssetCategory(passedPrompt));
    }
    const passedMode = sessionStorage.getItem("nexora_initial_mode");
    if (passedMode) {
      sessionStorage.removeItem("nexora_initial_mode");
      const valid3dMode: StudioMode = passedMode === "image_to_3d" || passedMode === "text_to_3d" ? passedMode : "full_circle";
      setStudioMode(valid3dMode);
    }

    const passedAssetId = sessionStorage.getItem("nexora_selected_image_asset_id");
    if (passedAssetId) {
      sessionStorage.removeItem("nexora_selected_image_asset_id");
      setStudioMode("image_to_3d");
      setRequest((prev) => ({ ...prev, sourceAssetId: passedAssetId }));
      setConceptImageAssetId(passedAssetId);
      setFactoryStep("prompt");
      getAssetPreview(passedAssetId).then((prev) => { if (mounted) setConceptImagePreviewUrl(prev); }).catch(() => {});
    }

    const unlisten = listen<{ runtimeId: string; status: RuntimeStatus }>(
      "runtime://status-changed",
      ({ payload }) => {
        if (!mounted) return;
        setRuntimeHealth((previous) => ({ ...previous, [payload.runtimeId]: payload.status }));
        setRuntimes((previous) => previous.map((runtime) =>
          runtime.config.runtimeId === payload.runtimeId
            ? { ...runtime, status: payload.status }
            : runtime
        ));
      }
    );

    loadWorkspaceData().finally(() => { if (mounted) setLoading(false); });
    return () => {
      mounted = false;
      unlisten.then((stopListening) => stopListening());
    };
  }, []);

  // Poll Active Job
  useEffect(() => {
    if (!job || !activeJobId.current) return;
    const isJobActive = job.status === "queued" || job.status === "running";
    if (!isJobActive) return;

    const jobId = job.jobId;
    let active = true;
    let inFlight = false;

    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const nextJob = await getJobDetails(jobId);
        if (!active || activeJobId.current !== jobId) return;
        setJob(nextJob);
        setJobs((current) => current.map((item) => item.jobId === jobId ? nextJob : item));

        // When image job completes in full_circle mode
        if (nextJob.status === "completed" && nextJob.jobType === "image.generate" && factoryStep === "concept_generating") {
          const res = await getImageGenerationResult(jobId);
          const assetIds = completedImageAssetIds(res);
          if (assetIds.length > 0) {
            const imgId = assetIds[0];
            setConceptImageAssetId(imgId);
            const preview = await getAssetPreview(imgId);
            setConceptImagePreviewUrl(preview);
            setFactoryStep("concept_review");
          }
        }

        // When 3D generation completes
        if (nextJob.status === "completed" && nextJob.jobType === "hunyuan.generate" && factoryStep === "threed_generating") {
          const res = await getHunyuanGenerationResult(jobId);
          const assetIds = completedHunyuanAssetIds(res);
          if (assetIds.length > 0) {
            const rawId = assetIds[0];
            setRaw3dAssetId(rawId);
            void triggerBlenderProcessing(rawId);
          }
        }

        // When Blender processing completes
        if (nextJob.status === "completed" && nextJob.jobType === "model3d.processing" && factoryStep === "blender_processing") {
          const res = await getModel3dProcessingResult(jobId);
          if (res.assetIds.length > 0) {
            const final3dId = res.assetIds[0];
            setProcessed3dAssetId(final3dId);
            setFinal3dApproved(false);
            setSelectedAssetIdForViewer(final3dId);
            setBlenderReport(res.postAnalysisReport);
            setFactoryStep("unity_ready");
            const nextAssets = await listAssets();
            setAssets(nextAssets);
          }
        }

      } catch (nextError) {
        if (active) setError(presentHunyuanGenerationError(nextError));
      } finally {
        inFlight = false;
      }
    };

    const timer = window.setInterval(() => void poll(), POLL_INTERVAL_MS);
    void poll();
    return () => { active = false; window.clearInterval(timer); };
  }, [job?.jobId, job?.status, factoryStep]);

  // Poll video generation job
  useEffect(() => {
    if (!videoJob || !activeVideoJobId.current) return;
    const isJobActive = videoJob.status === "queued" || videoJob.status === "running";
    if (!isJobActive) return;

    const jobId = videoJob.jobId;
    let active = true;
    let inFlight = false;

    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const nextJob = await getJobDetails(jobId);
        if (!active || activeVideoJobId.current !== jobId) return;
        setVideoJob(nextJob);
      } catch (nextError) {
        if (active) setVideoMessage(presentVideoGenerationError(nextError));
      } finally {
        inFlight = false;
      }
    };

    const timer = window.setInterval(() => void poll(), POLL_INTERVAL_MS);
    void poll();
    return () => { active = false; window.clearInterval(timer); };
  }, [videoJob?.jobId, videoJob?.status]);

  // Resolve video generation results
  useEffect(() => {
    if (videoJob?.status !== "completed") return;
    const jobId = videoJob.jobId;
    let active = true;
    let inFlight = false;
    let resolved = false;

    const resolveResult = async () => {
      if (inFlight || resolved) return;
      inFlight = true;
      try {
        const result = await getVideoGenerationResult(jobId);
        const assetIds = result.assetIds ?? [];
        const nextAssets = assetIds.length ? await listAssets().catch(() => [] as AssetInfo[]) : [];
        if (!active || activeVideoJobId.current !== jobId) return;
        resolved = true;
        setVideoOutputAssetIds(assetIds);
        setVideoOutputs(nextAssets.filter((asset) => assetIds.includes(asset.assetId)));
        setVideoMessage(assetIds.length ? `${assetIds.length} generated video asset${assetIds.length === 1 ? " is" : "s are"} available.` : "Generation completed, but no video assets are available yet.");
      } catch (nextError) {
        if (active) setVideoMessage(presentVideoGenerationError(nextError));
      } finally {
        inFlight = false;
      }
    };

    void resolveResult();
    const timer = window.setInterval(() => void resolveResult(), 5000);
    return () => { active = false; window.clearInterval(timer); };
  }, [videoJob?.jobId, videoJob?.status]);

  // Load review jobs
  useEffect(() => {
    let mounted = true;
    const load = async () => {
      try {
        const nextJobs = await listJobs();
        const processingJobs = nextJobs.filter((item: JobInfo) => item.jobType === "model3d.processing");
        if (mounted) setReviewJobs(processingJobs);
      } catch {
        // ignore
      }
    };
    load();
    const timer = window.setInterval(load, POLL_INTERVAL_MS);
    return () => { mounted = false; window.clearInterval(timer); };
  }, []);

  // Load selected review job result
  useEffect(() => {
    if (!selectedReviewJobId) {
      setReviewJobResult(null);
      return;
    }
    let mounted = true;
    getModel3dProcessingResult(selectedReviewJobId)
      .then((result) => {
        if (mounted) {
          setReviewJobResult(result);
          setReviewDeploymentResult(null);
          setReviewDeploymentSuccess(false);
        }
      })
      .catch(() => {
        if (mounted) setReviewJobResult(null);
      });
    return () => { mounted = false; };
  }, [selectedReviewJobId]);

  // Trigger Blender Optimization
  const triggerBlenderProcessing = async (sourceId: string) => {
    setFactoryStep("blender_processing");
    setError("");
    try {
      const category = detectAssetCategory(request.prompt);
      setUnityCategory(category);
      const profile = (category.toLowerCase() as Model3dProcessingProfile) || "generic";
      const created = await createModel3dProcessingJob({
        schemaVersion: 1,
        sourceAssetId: sourceId,
        profile: profile,
        quality: "mobile_high",
      });
      activeJobId.current = created.job.jobId;
      setJob(created.job);
      setJobs((current) => [created.job, ...current]);
    } catch (err) {
      setError(`Blender optimization failed: ${err instanceof Error ? err.message : String(err)}`);
      setFactoryStep("prompt");
    }
  };

  // Launch Full Circle Pipeline
  const handleStartFullCircle = async () => {
    if (!request.prompt.trim()) return;
    if (!generationProviderReady) {
      setError(generationProviderMessage);
      return;
    }
    setError("");
    if (studioMode === "image_to_3d" && request.sourceAssetId) {
      setFactoryStep("threed_generating");
      try {
        const created = await createHunyuanGenerationJob({
          request: {
            schemaVersion: 1,
            mode: "image_to_3d",
            prompt: promptForCreationStyle(request.prompt, assetCreationStyle),
            negativePrompt: request.negativePrompt,
            sourceAssetId: request.sourceAssetId,
            profile: HUNYUAN_GENERATION_PROFILE_ID,
            quality: HUNYUAN_QUALITY,
            seed: randomSeed ? null : request.seed,
            outputFormat: HUNYUAN_OUTPUT_FORMAT,
          },
          providerId: "local.hunyuan",
        });
        activeJobId.current = created.job.jobId;
        setJob(created.job);
        setJobs((current) => [created.job, ...current]);
      } catch (err) {
        setError(`3D generation failed: ${presentHunyuanGenerationError(err)}`);
        setFactoryStep("prompt");
      }
      return;
    }
    if (studioMode === "text_to_3d") {
      setFactoryStep("threed_generating");
      try {
        const created = await createHunyuanGenerationJob({
          request: {
            schemaVersion: 1,
            mode: "text_to_3d",
            prompt: promptForCreationStyle(request.prompt, assetCreationStyle),
            negativePrompt: request.negativePrompt,
            sourceAssetId: null,
            profile: HUNYUAN_GENERATION_PROFILE_ID,
            quality: HUNYUAN_QUALITY,
            seed: randomSeed ? null : request.seed,
            outputFormat: HUNYUAN_OUTPUT_FORMAT,
          },
          providerId: "local.hunyuan",
        });
        activeJobId.current = created.job.jobId;
        setJob(created.job);
        setJobs((current) => [created.job, ...current]);
      } catch (err) {
        setError(`Direct 3D generation failed: ${presentHunyuanGenerationError(err)}`);
        setFactoryStep("prompt");
      }
      return;
    }
    setFactoryStep("concept_generating");
    setConceptImagePreviewUrl(null);
    setConceptImageAssetId(null);
    
    try {
      const created = await createImageGenerationJob({
        request: {
          schemaVersion: 1,
          prompt: buildConceptPrompt(request.prompt, assetCreationStyle),
          negativePrompt: `${CONCEPT_NEGATIVE_EXTRAS}, ${request.negativePrompt || "blurry, dark, noisy, low quality, artifacts, watermark"}`,
          width: 512,
          height: 512,
          seed: randomSeed ? null : request.seed,
          steps: 20,
          guidance: 7,
          outputCount: 1,
        },
        providerId: "local.a1111",
      });
      activeJobId.current = created.job.jobId;
      setJob(created.job);
      setJobs((current) => [created.job, ...current]);
    } catch (err) {
      setError(`Concept generation failed: ${presentImageGenerationError(err)}`);
      setFactoryStep("prompt");
    }
  };

  // User Approves Concept Image (Checkpoint A)
  const handleApproveConceptImage = async () => {
    if (!conceptImageAssetId) return;
    setFactoryStep("threed_generating");
    setError("");
    try {
      await approveImageAsset(conceptImageAssetId, job?.jobType === "image.generate" ? job.jobId : null);
      const created = await createHunyuanGenerationJob({
        request: {
          schemaVersion: 1,
          mode: "image_to_3d",
          prompt: promptForCreationStyle(request.prompt, assetCreationStyle),
          negativePrompt: request.negativePrompt,
          sourceAssetId: conceptImageAssetId,
          profile: HUNYUAN_GENERATION_PROFILE_ID,
          quality: HUNYUAN_QUALITY,
          seed: randomSeed ? null : request.seed,
          outputFormat: HUNYUAN_OUTPUT_FORMAT,
        },
        providerId: "local.hunyuan",
      });
      activeJobId.current = created.job.jobId;
      setJob(created.job);
      setJobs((current) => [created.job, ...current]);
    } catch (err) {
      setError(`3D generation failed: ${presentHunyuanGenerationError(err)}`);
      setFactoryStep("concept_review");
    }
  };

  // User Deploys to Unity (Checkpoint B)
  const handleDeployToUnity = async () => {
    const targetAssetId = processed3dAssetId || selectedAssetIdForViewer;
    if (!targetAssetId || !unityProjectRoot || !final3dApproved) return;
    
    setActionLoading("deploy");
    setError("");
    try {
      const val = await validateUnityProject(unityProjectRoot);
      setUnityValidation(val);
      if (!val.isValid) {
        throw new Error(val.errorMessage || "Invalid Unity project path. Must contain Assets/ and ProjectSettings/");
      }
      const res = await deployModel3dToUnity(targetAssetId, "default-unity-target", unityCategory);
      setDeploymentResult(res);
      setFactoryStep("completed");
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (err) {
      setError(`Deployment failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActionLoading(null);
    }
  };

  const handleApproveFinalAsset = async () => {
    if (!processed3dAssetId || !job || job.jobType !== "model3d.processing") return;
    setActionLoading("approve");
    setError("");
    try {
      await approveModel3dAsset({ assetId: processed3dAssetId, processingJobId: job.jobId });
      setFinal3dApproved(true);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "3D approval failed");
    } finally {
      setActionLoading(null);
    }
  };

  // Image-to-3D Import Flow
  const handlePickAndImportImage = async () => {
    setImportingImage(true);
    setError("");
    try {
      const pickedPath = await pickAssetFile();
      if (!pickedPath) return;
      const imported = await importAsset(pickedPath);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
      setConceptImageAssetId(imported.assetId);
      const preview = await getAssetPreview(imported.assetId);
      setConceptImagePreviewUrl(preview);
      setStudioMode("image_to_3d");
      setFactoryStep("concept_review");
    } catch (err: unknown) {
      setError(`Failed to import image: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setImportingImage(false);
    }
  };

  const handleApplyPreset = (suffix: string) => {
    let clean = request.prompt;
    STYLE_PRESETS.forEach((p) => { clean = clean.replace(p.promptSuffix, ""); });
    const newPrompt = clean.trim() + suffix;
    setRequest({ ...request, prompt: newPrompt });
    setUnityCategory(detectAssetCategory(newPrompt));
  };

  const handleCancelJob = async () => {
    if (!job || !job.status || job.cancellationRequested) return;
    setCancelling(true);
    try {
      const nextJob = await requestJobCancellation(job.jobId);
      setJob(nextJob);
    } catch (err) {
      setError(presentHunyuanGenerationError(err));
    } finally {
      setCancelling(false);
    }
  };

  const a1111Runtime = runtimes.find(r => r.config?.runtimeId === "automatic1111" || r.config?.runtimeId === "a1111");
  const hunyuanRuntime = runtimes.find(r => r.config?.runtimeId === "hunyuan3d" || r.config?.runtimeId === "hunyuan");
  const blenderRuntime = runtimes.find(r => r.config?.runtimeId === "blender");
  const a1111ProviderReady = providers.some((provider) =>
    provider.manifest.providerId === "local.a1111"
    && provider.manifest.enabled
    && provider.manifest.nature === "real"
    && provider.manifest.capabilities.includes("text_to_image")
    && provider.health.state === "healthy"
    && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning")
  );
  const hunyuanProviderReadyForStatus = providers.some((provider) =>
    provider.manifest.providerId === "local.hunyuan"
    && provider.manifest.enabled
    && provider.manifest.nature === "real"
    && (provider.manifest.capabilities.includes("hunyuan.text_to_3d") || provider.manifest.capabilities.includes("hunyuan.image_to_3d"))
    && provider.health.state === "healthy"
    && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning")
  );
  const a1111Status = runtimeHealth.automatic1111 ?? (a1111ProviderReady ? "ready" : a1111Runtime?.status ?? "unavailable");
  const hunyuanStatus = runtimeHealth.hunyuan3d ?? (hunyuanProviderReadyForStatus ? "ready" : hunyuanRuntime?.status ?? "unavailable");
  const blenderStatus = runtimeHealth.blender ?? blenderRuntime?.status ?? "unavailable";
  const modelAssets = assets.filter((asset) => asset.mediaKind === "model3d" && asset.status === "ready");
  const imageProviderReady = providers.some((provider) =>
    provider.manifest.providerId === "local.a1111"
    && provider.manifest.enabled
    && provider.manifest.nature === "real"
    && provider.manifest.capabilities.includes("text_to_image")
    && provider.health.state === "healthy"
    && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning")
  );
  const hunyuanProviderReady = providers.some((provider) =>
    isEligibleHunyuanProvider(provider, studioMode === "image_to_3d" ? "image_to_3d" : "text_to_3d")
  );
  const generationProviderReady = studioMode === "full_circle" ? imageProviderReady : hunyuanProviderReady;
  const generationProviderMessage = studioMode === "full_circle"
    ? "Automatic1111 concept provider is required for the full pipeline. Configure and enable it in Providers."
    : "A healthy, compatible Hunyuan3D provider is required for direct 3D generation.";

  // Video mode state
  const [videoPrompt, setVideoPrompt] = useState("");
  const [videoNegativePrompt, setVideoNegativePrompt] = useState("");
  const [videoWidth, setVideoWidth] = useState(320);
  const [videoHeight, setVideoHeight] = useState(192);
  const [videoFrameCount, setVideoFrameCount] = useState(9);
  const [videoFps, setVideoFps] = useState(8);
  const [videoSeed, setVideoSeed] = useState<number | null>(null);
  const [videoRandomSeed, setVideoRandomSeed] = useState(true);
  const videoProvider = videoProviders.find(({ manifest }) => manifest.providerId === VIDEO_PROVIDER_ID);
  const videoProviderReady = Boolean(videoProvider && isUsableVideoProvider(videoProvider));
  const isVideoPolling = videoJob ? shouldPollVideoGenerationJob(videoJob) : false;
  const videoError = videoJob?.status === "failed" ? (videoJob.errorCode && `${videoJob.errorCode}: `) + (videoJob.errorMessage || "Video generation failed.") : "";
  const handleVideoGenerate = async () => {
    if (!videoPrompt.trim() || !videoProviderReady) return;
    const version = ++videoRequestVersion.current;
    setVideoGenerating(true);
    setVideoMessage("");
    setVideoOutputs([]);
    setVideoOutputAssetIds([]);
    setVideoJob(null);
    try {
      const request: VideoGenerationRequest = {
        schemaVersion: 1,
        mode: "text_to_video",
        prompt: videoPrompt,
        negativePrompt: videoNegativePrompt.trim() || null,
        width: videoWidth,
        height: videoHeight,
        frameCount: videoFrameCount,
        fps: videoFps,
        seed: videoRandomSeed ? null : videoSeed,
        profile: VIDEO_GENERATION_PROFILE_ID,
        sourceAssetId: null,
      };
      const created = await createVideoGenerationJob({ request, providerId: VIDEO_PROVIDER_ID });
      if (version === videoRequestVersion.current) {
        activeVideoJobId.current = created.job.jobId;
        setVideoJob(created.job);
      }
    } catch (nextError) {
      if (version === videoRequestVersion.current) {
        setVideoMessage(presentVideoGenerationError(nextError));
      }
    } finally {
      if (version === videoRequestVersion.current) setVideoGenerating(false);
    }
  };

  const handleVideoCancel = async () => {
    if (!videoJob || !canCancelVideoGenerationJob(videoJob)) return;
    const version = ++videoRequestVersion.current;
    setVideoCancelling(true);
    setVideoMessage("");
    try {
      const nextJob = await requestJobCancellation(videoJob.jobId);
      if (version === videoRequestVersion.current) setVideoJob(nextJob);
    } catch (nextError) {
      if (version === videoRequestVersion.current) setVideoMessage(presentVideoGenerationError(nextError));
    } finally {
      if (version === videoRequestVersion.current) setVideoCancelling(false);
    }
  };

  const handleReviewJobSelect = async (jobId: string) => {
    setSelectedReviewJobId(jobId);
    setReviewJobResult(null);
    try {
      const result = await getModel3dProcessingResult(jobId);
      setReviewJobResult(result);
    } catch {
      setReviewJobResult(null);
    }
  };

  const handleValidateUnityPath = async (path: string) => {
    setUnityProjectRoot(path);
    if (!path.trim()) {
      setUnityValidation(null);
      return;
    }
    try {
      const val = await validateUnityProject(path);
      setUnityValidation(val);
    } catch (e) {
      setUnityValidation({ isValid: false, unityVersion: null, errorMessage: String(e) });
    }
  };

  const handleReviewDeployToUnity = async () => {
    if (!reviewJobResult || reviewJobResult.assetIds.length === 0 || !unityProjectRoot) return;
    setReviewDeploying(true);
    setError("");
    setReviewDeploymentSuccess(false);
    try {
      const val = await validateUnityProject(unityProjectRoot);
      setUnityValidation(val);
      if (!val.isValid) {
        throw new Error(val.errorMessage || "Invalid Unity project path");
      }
      const assetId = reviewJobResult.assetIds[0];
      const result = await deployModel3dToUnity(assetId, "default-unity-target", unityCategory);
      setReviewDeploymentResult(result);
      setReviewDeploymentSuccess(true);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setReviewDeploying(false);
    }
  };

  const handleReviewApprove = async () => {
    if (!reviewJobResult || !canApproveAsset(reviewJobResult.status as any)) return;
    setReviewActionLoading("approve");
    try {
      await approveModel3dAsset({ assetId: reviewJobResult.assetIds[0], processingJobId: reviewJobResult.jobId });
      const updated = await getModel3dProcessingResult(reviewJobResult.jobId);
      setReviewJobResult(updated);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Approval failed");
    } finally {
      setReviewActionLoading(null);
    }
  };

  const handleReviewReject = async () => {
    if (!reviewJobResult || !canRejectAsset(reviewJobResult.status as any)) return;
    const reason = prompt("Enter rejection reason (optional):");
    setReviewActionLoading("reject");
    try {
      await rejectModel3dAsset({ 
        assetId: reviewJobResult.assetIds[0], 
        processingJobId: reviewJobResult.jobId,
        rejectionReason: reason?.trim() || undefined 
      });
      const updated = await getModel3dProcessingResult(reviewJobResult.jobId);
      setReviewJobResult(updated);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Rejection failed");
    } finally {
      setReviewActionLoading(null);
    }
  };

  const handleReviewReprocess = async () => {
    if (!reviewJobResult || !canReprocessAsset(reviewJobResult.status as any)) return;
    setReviewActionLoading("reprocess");
    try {
      const created = await reprocessModel3dAsset({ 
        assetId: reviewJobResult.assetIds[0], 
        processingJobId: reviewJobResult.jobId 
      });
      const nextJobs = await listJobs();
      setReviewJobs(nextJobs.filter((item: JobInfo) => item.jobType === "model3d.processing"));
      setSelectedReviewJobId(created.job.jobId);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Reprocess failed");
    } finally {
      setReviewActionLoading(null);
    }
  };

  useEffect(() => {
    const usable = videoProviders.filter((p) => isUsableVideoProvider(p));
    if (usable.length > 0 && !videoConfig) {
      setVideoConfig({
        schemaVersion: 1,
        mode: "text_to_video",
        prompt: "",
        negativePrompt: null,
        width: 320,
        height: 192,
        frameCount: 9,
        fps: 8,
        seed: null,
        profile: VIDEO_GENERATION_PROFILE_ID,
        sourceAssetId: null,
      });
    }
  }, [videoProviders, videoConfig]);

  if (loading) {
    return (
      <div className="universal-studio-loading">
        <div className="studio-loading-badge">NEXORA 3D STUDIO</div>
        <div className="loading-spinner" />
        <span>Initializing Studio Foundation & GPU Pipeline...</span>
      </div>
    );
  }

  return (
    <section className="universal-studio-page">
      {/* Studio Header Bar */}
      <div className="studio-topbar">
        <div className="studio-branding">
          <div className="studio-badge">UNIVERSAL CREATIVE STUDIO</div>
          <h2>3D Studio · One-Prompt Asset Pipeline</h2>
          <p>Prompt ➔ Concept ➔ 3D ➔ Materials ➔ Optimize ➔ Preview ➔ Unity Export</p>
        </div>
        <div className="studio-topbar__actions">
          <div className="runtime-status-bar">
            <span className="runtime-status-item">
              <span className={`status-dot ${a1111Status === "ready" ? "status-dot--ready" : "status-dot--offline"}`} />
              <span>A1111: <strong>{a1111Status === "ready" ? "Ready" : "Offline"}</strong></span>
            </span>
            <span className="runtime-status-item">
              <span className={`status-dot ${hunyuanStatus === "ready" ? "status-dot--ready" : "status-dot--offline"}`} />
              <span>Hunyuan3D: <strong>{hunyuanStatus === "ready" ? "Ready" : "Offline"}</strong></span>
            </span>
            <span className="runtime-status-item">
              <span className={`status-dot ${blenderStatus === "ready" ? "status-dot--ready" : "status-dot--offline"}`} />
              <span>Blender: <strong>{blenderStatus === "ready" ? "Ready" : "Unavailable"}</strong></span>
            </span>
          </div>
          <button className="btn btn--secondary" type="button" disabled={refreshing} onClick={() => void loadWorkspaceData()}>
            {refreshing ? "Refreshing..." : "🔄 Refresh"}
          </button>
        </div>
      </div>

      {/* 3D Studio Project Management Bar (Gemini / Flow AI Style) */}
      <div className="studio-project-banner" style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '12px 18px',
        background: 'linear-gradient(135deg, rgba(14, 22, 42, 0.85) 0%, rgba(9, 14, 28, 0.92) 100%)',
        border: '1px solid rgba(0, 240, 255, 0.2)',
        borderRadius: '12px',
        boxShadow: '0 6px 20px rgba(0, 0, 0, 0.35)',
        backdropFilter: 'blur(14px)',
        marginBottom: '16px',
        flexWrap: 'wrap',
        gap: '12px',
      }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <span style={{ fontSize: '18px' }}>📁</span>
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <span className="panel-label" style={{ marginBottom: 0 }}>ACTIVE PIPELINE PROJECT</span>
              <span className={`status-badge status-badge--${currentProject ? "good" : "muted"}`} style={{ padding: '1px 7px', fontSize: '10px' }}>
                {currentProject ? "OPEN" : "NONE"}
              </span>
            </div>
            <strong style={{ color: currentProject ? '#00f0ff' : '#f8fafc', fontSize: '13px' }}>
              {currentProject?.manifest.name || "No Project Open — Assets will be stored in local studio database"}
            </strong>
            {currentProject?.root && (
              <span style={{ fontSize: '11px', color: '#64748b', display: 'block', fontFamily: 'monospace' }}>
                {currentProject.root}
              </span>
            )}
          </div>
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' }}>
          {/* Create New Project Button */}
          <button
            className="btn btn--primary btn--sm"
            type="button"
            style={{ display: 'inline-flex', alignItems: 'center', gap: '4px', padding: '6px 12px', fontSize: '12px' }}
            onClick={() => onOpenProjectModal?.("create", "studio")}
          >
            <span>+</span> Create New Project
          </button>

          {/* Open Folder Button */}
          <button
            className="btn btn--secondary btn--sm"
            type="button"
            style={{ padding: '6px 12px', fontSize: '12px' }}
            onClick={() => onOpenProjectModal?.("open", "studio")}
          >
            📂 Existing Project
          </button>

        </div>
      </div>

      {error && <div className="error-banner" role="alert">{error}</div>}

      {/* Real-time Pipeline Visualizer */}
      <div className="factory-pipeline-tracker" aria-label="Pipeline Stages">
        <div className={`factory-step ${factoryStep === "prompt" ? "factory-step--active" : "factory-step--done"}`}>
          <span className="step-circle">{factoryStep !== "prompt" ? "✓" : "1"}</span>
          <div><strong>1. Prompt</strong><small>Asset Concept</small></div>
        </div>
        <span className="factory-arrow">➔</span>

        <div className={`factory-step ${factoryStep === "concept_generating" ? "factory-step--active" : factoryStep === "concept_review" ? "factory-step--checkpoint" : ["threed_generating", "blender_processing", "unity_ready", "deploying", "completed"].includes(factoryStep) ? "factory-step--done" : ""}`}>
          <span className="step-circle">{["threed_generating", "blender_processing", "unity_ready", "deploying", "completed"].includes(factoryStep) ? "✓" : factoryStep === "concept_review" ? "!" : "2"}</span>
          <div><strong>2. 2D Concept</strong><small>{factoryStep === "concept_review" ? "Awaiting Approval" : "A1111 Engine"}</small></div>
        </div>
        <span className="factory-arrow">➔</span>

        <div className={`factory-step ${factoryStep === "threed_generating" ? "factory-step--active" : ["blender_processing", "unity_ready", "deploying", "completed"].includes(factoryStep) ? "factory-step--done" : ""}`}>
          <span className="step-circle">{["blender_processing", "unity_ready", "deploying", "completed"].includes(factoryStep) ? "✓" : "3"}</span>
          <div><strong>3. 3D Model</strong><small>Hunyuan3D GPU</small></div>
        </div>
        <span className="factory-arrow">➔</span>

        <div className={`factory-step ${factoryStep === "blender_processing" ? "factory-step--active" : ["unity_ready", "deploying", "completed"].includes(factoryStep) ? "factory-step--done" : ""}`}>
          <span className="step-circle">{["unity_ready", "deploying", "completed"].includes(factoryStep) ? "✓" : "4"}</span>
          <div><strong>4. Blender LODs</strong><small>Mesh Clean & Fix</small></div>
        </div>
        <span className="factory-arrow">➔</span>

        <div className={`factory-step ${factoryStep === "unity_ready" && !final3dApproved ? "factory-step--checkpoint" : final3dApproved || factoryStep === "deploying" || factoryStep === "completed" ? "factory-step--done" : ""}`}>
          <span className="step-circle">{final3dApproved || factoryStep === "deploying" || factoryStep === "completed" ? "✓" : "5"}</span>
          <div><strong>5. 3D Approval</strong><small>{final3dApproved ? "Approved" : "Required before deployment"}</small></div>
        </div>
        <span className="factory-arrow">➔</span>

        <div className={`factory-step ${factoryStep === "unity_ready" ? "factory-step--checkpoint" : factoryStep === "deploying" ? "factory-step--active" : factoryStep === "completed" ? "factory-step--done" : ""}`}>
          <span className="step-circle">{factoryStep === "completed" ? "✓" : factoryStep === "unity_ready" && final3dApproved ? "6" : "6"}</span>
          <div><strong>6. Engine Deployment</strong><small>{factoryStep === "completed" ? "Deployed" : final3dApproved ? "Ready for target" : "Locked"}</small></div>
        </div>
      </div>

       {/* Dedicated 1-Prompt Pipeline Banner */}
       <div className="studio-mode-banner" style={{
         display: 'flex',
         alignItems: 'center',
         gap: '14px',
         padding: '14px 20px',
         background: 'linear-gradient(135deg, rgba(0, 240, 255, 0.08) 0%, rgba(14, 22, 42, 0.7) 100%)',
         border: '1px solid rgba(0, 240, 255, 0.25)',
         borderRadius: '12px',
         marginBottom: '16px',
       }}>
         <span style={{ fontSize: '24px' }}>⚡</span>
         <div>
           <strong style={{ color: '#00f0ff', fontSize: '14px', display: 'block' }}>1-Prompt Full Production Pipeline</strong>
           <span style={{ fontSize: '12px', color: '#94a3b8' }}>Automated studio flow: Single Prompt ➔ 2D Concept Art ➔ Checkpoint Approval ➔ 3D Neural Reconstruction ➔ Blender Retopology & LODs ➔ Final Verification ➔ Engine Deployment.</span>
         </div>
       </div>

       <section className="asset-creation-modes" aria-labelledby="asset-creation-modes-title">
         <div className="asset-creation-modes__heading">
           <div>
             <div className="panel-label">3D ASSET CREATION MODE</div>
             <h3 id="asset-creation-modes-title">Choose your visual direction</h3>
           </div>
           <span>Applied to concept, 3D reconstruction and cleanup metadata</span>
         </div>
         <div className="asset-creation-modes__grid" role="radiogroup" aria-label="3D asset creation style">
           <button
             className={`asset-creation-mode ${assetCreationStyle === "neon" ? "asset-creation-mode--active" : ""}`}
             type="button"
             role="radio"
             aria-checked={assetCreationStyle === "neon"}
             onClick={() => setAssetCreationStyle("neon")}
           >
             <span className="asset-creation-mode__art asset-creation-mode__art--neon" aria-hidden="true"><span>✦</span></span>
             <span className="asset-creation-mode__copy"><strong>Neon / Cyberpunk</strong><small>Emissive trim, cyan-magenta lighting and futuristic materials.</small><em>{assetCreationStyle === "neon" ? "Selected for this pipeline" : "Select style"}</em></span>
           </button>
           <button
             className={`asset-creation-mode ${assetCreationStyle === "normal" ? "asset-creation-mode--active" : ""}`}
             type="button"
             role="radio"
             aria-checked={assetCreationStyle === "normal"}
             onClick={() => setAssetCreationStyle("normal")}
           >
             <span className="asset-creation-mode__art asset-creation-mode__art--normal" aria-hidden="true"><span>◇</span></span>
             <span className="asset-creation-mode__copy"><strong>Normal Assets</strong><small>Natural materials, balanced colors and practical game-ready forms.</small><em>{assetCreationStyle === "normal" ? "Selected for this pipeline" : "Select style"}</em></span>
           </button>
         </div>
       </section>

      {/* Main Studio Grid */}
      <div className="studio-main-grid">
        {/* Left Column: Factory Interactive Workflows */}
        <div className="studio-controls-column">
          
          {/* STEP 1: Prompt Input Panel */}
          {factoryStep === "prompt" && (
            <div className="studio-panel studio-form-panel">
              <div className="studio-panel-header">
                <div className="panel-label">CREATIVE SPECIFICATION</div>
                <h3>{studioMode === "full_circle" ? "1-Prompt Asset Creator" : studioMode === "image_to_3d" ? "Image-to-3D Settings" : "Direct 3D Settings"}</h3>
              </div>

              {studioMode === "image_to_3d" && (
                <div className="image-import-box">
                  <button
                    className="btn btn--primary import-btn"
                    type="button"
                    disabled={importingImage}
                    onClick={() => void handlePickAndImportImage()}
                  >
                    {importingImage ? "Importing..." : "📁 Browse & Import Image from PC"}
                  </button>
                </div>
              )}

              <div className="studio-field">
                <label className="form-field-label">Asset Description & Visual Guidance</label>
                <textarea
                  className="studio-textarea"
                  rows={4}
                  value={request.prompt}
                  onChange={(e) => {
                    setRequest({ ...request, prompt: e.target.value });
                    setUnityCategory(detectAssetCategory(e.target.value));
                  }}
                  placeholder="Describe your 3D asset (e.g. 'Futuristic neon sports racing car with glowing cyan underglow and carbon fiber chassis')..."
                  required
                />
              </div>

              {/* Style Presets */}
              <div className="studio-presets-row">
                <span className="presets-label">Presets:</span>
                <div className="preset-chips-list">
                  {STYLE_PRESETS.map((preset) => (
                    <button
                      key={preset.id}
                      type="button"
                      className="preset-chip"
                      onClick={() => handleApplyPreset(preset.promptSuffix)}
                    >
                      {preset.label}
                    </button>
                  ))}
                </div>
              </div>

              <div className="unity-meta-info-grid" style={{ marginTop: 16 }}>
                <div className="unity-meta-item">
                  <label>Detected Category</label>
                  <strong>{unityCategory || "Auto Detect"}</strong>
                </div>
                <div className="unity-meta-item">
                  <label>Blender Processing</label>
                  <strong>Auto Decimation + LODs</strong>
                </div>
              </div>

              <div
                className={`image-provider-state ${generationProviderReady ? "" : "image-provider-state--warning"}`}
                role="status"
                style={{ marginTop: 14 }}
              >
                {generationProviderReady
                  ? (studioMode === "full_circle" ? "Concept provider ready — the remaining stages will use the configured Hunyuan3D and Blender integrations." : "Hunyuan3D provider ready.")
                  : generationProviderMessage}
              </div>

              <button
                className="btn btn--primary studio-launch-btn"
                type="button"
                disabled={!request.prompt.trim() || !generationProviderReady}
                onClick={() => void handleStartFullCircle()}
              >
                ⚡ Generate 3D Asset (1-Click Pipeline)
              </button>
            </div>
          )}

          {/* STEP 2: Concept Generating Progress */}
          {factoryStep === "concept_generating" && (
            <div className="studio-panel studio-job-card">
              <div className="studio-panel-header">
                <div className="panel-label">STAGE 1: 2D CONCEPT ART</div>
                <span className="status-badge status-badge--job-running">Synthesizing</span>
              </div>
              <p style={{ color: "#a5d6a7", fontSize: 13 }}>
                🎨 Automatic1111 is generating a high-definition 2D concept of <strong>"{request.prompt.slice(0, 40)}..."</strong>
              </p>
              <div className="job-progress-bar-container">
                <div className="job-progress-bar" style={{ width: job ? formatJobProgress(job.progress) : "30%" }} />
              </div>
              <button className="btn btn--secondary cancel-btn" type="button" onClick={() => setFactoryStep("prompt")}>
                🛑 Cancel
              </button>
            </div>
          )}

          {/* STEP 3: Human Checkpoint A - Concept Review & Approval */}
          {factoryStep === "concept_review" && (
            <div className="checkpoint-card-premium">
              <div className="checkpoint-badge-row">
                <span className="checkpoint-tag">HUMAN CHECKPOINT A</span>
                <span className="status-badge status-badge--job-completed">Concept Ready</span>
              </div>
              <h3>Review & Approve 2D Concept</h3>
              <p style={{ color: "#9cb0c6", fontSize: 12, marginBottom: 14 }}>
                Review the generated concept below. Approving will automatically send it to Hunyuan3D-2mini for 3D reconstruction and Blender mesh optimization.
              </p>

              <div className="checkpoint-image-preview-container">
                {conceptImagePreviewUrl ? (
                  <img src={conceptImagePreviewUrl} alt="Concept Preview" className="checkpoint-concept-img" />
                ) : (
                  <div className="loading-spinner" />
                )}
                <div style={{ fontSize: 11, color: "#64788f" }}>Asset ID: {conceptImageAssetId}</div>
              </div>

              <div className="checkpoint-actions-row">
                <button
                  className="btn btn--primary"
                  type="button"
                  onClick={() => void handleApproveConceptImage()}
                >
                  ✅ Approve & Build 3D Model
                </button>
                <button
                  className="btn btn--secondary"
                  type="button"
                  onClick={() => void handleStartFullCircle()}
                >
                  🔄 Regenerate Concept
                </button>
                <button
                  className="btn btn--secondary"
                  type="button"
                  onClick={() => setFactoryStep("prompt")}
                >
                  ✏️ Edit Prompt
                </button>
              </div>
            </div>
          )}

          {/* STEP 4: 3D Reconstruction Progress */}
          {factoryStep === "threed_generating" && (
            <div className="studio-panel studio-job-card">
              <div className="studio-panel-header">
                <div className="panel-label">STAGE 2: 3D VOXEL RECONSTRUCTION</div>
                <span className="status-badge status-badge--job-running">Reconstructing</span>
              </div>
              <p style={{ color: "#90caf9", fontSize: 13 }}>
                🧊 Hunyuan3D-2mini is generating a high-density 3D surface mesh on GPU from the approved image...
              </p>
              <div className="job-progress-bar-container">
                <div className="job-progress-bar" style={{ width: job ? formatJobProgress(job.progress) : "45%" }} />
                <span className="job-progress-pct">{job ? formatJobProgress(job.progress) : "45%"}</span>
              </div>
              <button className="btn btn--secondary cancel-btn" type="button" onClick={() => void handleCancelJob()}>
                {cancelling ? "Cancelling..." : "🛑 Cancel Generation"}
              </button>
            </div>
          )}

          {/* STEP 5: Blender Mesh Optimization Progress */}
          {factoryStep === "blender_processing" && (
            <div className="studio-panel studio-job-card">
              <div className="studio-panel-header">
                <div className="panel-label">STAGE 3: BLENDER 5.2 OPTIMIZATION</div>
                <span className="status-badge status-badge--job-running">Optimizing</span>
              </div>
              <p style={{ color: "#ffe082", fontSize: 13 }}>
                🛠️ Blender 5.2 is cleaning degenerate faces, fixing normals, recalculating transforms, and building LOD0/LOD1/LOD2 hierarchies...
              </p>
              <div className="job-progress-bar-container">
                <div className="job-progress-bar" style={{ width: job ? formatJobProgress(job.progress) : "75%" }} />
                <span className="job-progress-pct">{job ? formatJobProgress(job.progress) : "75%"}</span>
              </div>
            </div>
          )}

          {/* STEP 6: Human Checkpoint B - Export Ready */}
          {factoryStep === "unity_ready" && (
            <div className="unity-deploy-panel">
              <div className="checkpoint-badge-row" style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 12 }}>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <span className="checkpoint-tag">HUMAN CHECKPOINT B</span>
                  <span className="status-badge status-badge--job-completed">AAA Asset Ready</span>
                </div>
                <div style={{ background: "linear-gradient(135deg, #1b5e20, #2e7d32)", padding: "4px 12px", borderRadius: 12, fontWeight: 700, color: "#fff", fontSize: 12, border: "1px solid #4caf50" }}>
                  🏆 {blenderReport?.qualityScore ?? 96}/100 {blenderReport?.qualityRating?.replace(/_/g, " ") ?? "AAA GAME READY"}
                </div>
              </div>
               <h3>Final 3D Approval & Engine Deployment</h3>
               <p style={{ color: "#9cb0c6", fontSize: 12, marginBottom: 12 }}>
                 Asset has passed Blender cleanup, scale/origin normalization, and LOD/collision generation. Approve the cleaned model before choosing an engine.
               </p>

              <div className={`image-provider-state ${final3dApproved ? "" : "image-provider-state--warning"}`} role="status">
                {final3dApproved ? "3D asset approved — engine deployment is unlocked." : "READY FOR 3D APPROVAL — deployment is locked until you approve the cleaned model."}
              </div>

              {/* Asset Readiness Report */}
              <AssetReadinessReport 
                jobResult={{ 
                  jobId: "hunyuan-pipeline",
                  status: "ready_for_review" as const,
                  assetIds: processed3dAssetId ? [processed3dAssetId] : [],
                  processingStage: "complete",
                  progress: 100,
                  outputMasterPath: null,
                  lod0Path: null,
                  lod1Path: null,
                  lod2Path: null,
                  vehicleAnalysisPath: null,
                  materialStatus: blenderReport?.materialStatus || "ready",
                  preAnalysisReport: null,
                  postAnalysisReport: blenderReport,
                  errorCode: null,
                  errorMessage: null,
                }} 
                onDeploy={handleDeployToUnity} 
              />

              <div className="studio-field" style={{ marginTop: 12 }}>
                <label className="form-field-label">Target Project Directory</label>
                <input
                  type="text"
                  className="quick-prompt-input"
                  style={{ width: "100%", background: "#080d14", padding: "10px 14px", border: "1px solid #283a4f", borderRadius: 4, color: "#f0f6fc", marginBottom: 8 }}
                  value={unityProjectRoot}
                  onChange={(e) => {
                    setUnityProjectRoot(e.target.value);
                    validateUnityProject(e.target.value).then(setUnityValidation).catch(() => {});
                  }}
                  placeholder="e.g. C:\Users\Mak Tech\development\Games\MyGame"
                />
                {unityValidation?.isValid && (
                  <div style={{ color: "#4caf50", fontSize: 11, marginTop: 4 }}>
                    ✓ Valid Project (Version: {unityValidation.unityVersion ?? "Detected"})
                  </div>
                )}
              </div>

              <div className="unity-destination-preview">
                Destination: Assets/Nexora/{unityCategory || "General"}/{processed3dAssetId?.slice(0, 16) ?? "model"}.glb
              </div>

              <div className="checkpoint-actions-row">
                {!final3dApproved && (
                  <button
                    className="btn btn--primary"
                    type="button"
                    disabled={actionLoading === "approve" || !processed3dAssetId}
                    onClick={() => void handleApproveFinalAsset()}
                  >
                    {actionLoading === "approve" ? "Approving..." : "✅ Approve Cleaned 3D Asset"}
                  </button>
                )}
                <button
                  className="btn btn--primary"
                  type="button"
                  disabled={actionLoading === "deploy" || !unityProjectRoot || !final3dApproved}
                  onClick={() => void handleDeployToUnity()}
                >
                  {actionLoading === "deploy" ? "🚀 Exporting..." : "🚀 Deploy Approved Asset"}
                </button>
                <button
                  className="btn btn--secondary"
                  type="button"
                  onClick={() => setFactoryStep("prompt")}
                >
                  ✨ Create Another Asset
                </button>
              </div>
            </div>
          )}

          {/* STEP 7: Completed Celebration */}
          {factoryStep === "completed" && (
            <div className="deployed-celebration-card">
              <div className="celebration-header">
                <span className="celebration-icon">🎉</span>
                <div>
                  <h3>Asset Successfully Exported!</h3>
                  <small style={{ color: "#c8e6c9" }}>Full prefab hierarchy, LOD0/1/2, collision mesh, and .meta GUIDs created.</small>
                </div>
              </div>

              <div className="deployed-path-box">
                {deploymentResult?.deployedPath ? `📁 ${deploymentResult.deployedPath}` : "📁 Assets/Nexora/General/model.glb"}
              </div>

              <div className="unity-meta-info-grid">
                <div className="unity-meta-item">
                  <label>Asset Category</label>
                  <strong>{(deploymentResult?.category ?? unityCategory) || "General"}</strong>
                </div>
                <div className="unity-meta-item">
                  <label>Unity Destination</label>
                  <strong>{deploymentResult?.destinationFolder ?? "Assets/Nexora/General"}</strong>
                </div>
              </div>

              <div className="celebration-actions-row">
                <button
                  className="btn btn--primary"
                  type="button"
                  onClick={() => {
                    setFactoryStep("prompt");
                    setRequest({ ...request, prompt: "" });
                    setConceptImagePreviewUrl(null);
                    setConceptImageAssetId(null);
                  }}
                >
                  ✨ Create Another Asset
                </button>
                <button
                  className="btn btn--secondary"
                  type="button"
                  onClick={() => { window.location.hash = "assets"; }}
                >
                  📁 Open Asset Library
                </button>
              </div>
            </div>
          )}

        </div>

        {/* Right Column: Embedded 3D Viewport & Asset Inspector */}
        <div className="studio-viewport-column">
          <div className="studio-panel studio-viewport-panel">
            <div className="studio-panel-header">
              <div className="panel-label">INTERACTIVE 3D VIEWPORT</div>
              <div className="viewport-header-actions">
                <button
                  className="btn btn--secondary btn--sm"
                  type="button"
                  onClick={() => { window.location.hash = "model3d-review"; }}
                >
                  🛠️ Advanced Review
                </button>
                <button
                  className="btn btn--secondary btn--sm"
                  type="button"
                  onClick={() => { window.location.hash = "assets"; }}
                >
                  📁 Assets ({modelAssets.length})
                </button>
              </div>
            </div>

            {/* 3D Canvas Area */}
            <div className="viewport-canvas-container">
              {selectedAssetIdForViewer ? (
                <Model3dViewer assetId={selectedAssetIdForViewer} presetView="perspective" />
              ) : (
                <div className="empty-viewport-placeholder">
                  <div className="viewport-icon">🧊</div>
                  <h4>No 3D Model Loaded</h4>
                  <p>Enter a prompt on the left or select a previous asset below to inspect the 3D model in the interactive viewport.</p>
                </div>
              )}
            </div>

            {/* 3D Asset Selector Carousel */}
            {modelAssets.length > 0 && (
              <div className="studio-assets-tray">
                <div className="tray-label">Recent 3D Project Assets:</div>
                <div className="tray-items-row">
                  {modelAssets.map((asset) => (
                    <button
                      key={asset.assetId}
                      type="button"
                      className={`tray-asset-card ${selectedAssetIdForViewer === asset.assetId ? "tray-asset-card--active" : ""}`}
                      onClick={() => setSelectedAssetIdForViewer(asset.assetId)}
                    >
                      <span className="tray-asset-badge">GLB</span>
                      <strong className="tray-asset-name">{asset.originalFilename}</strong>
                      <small className="tray-asset-size">{formatFileSize(asset.fileSize)}</small>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>

        {studioMode === "video_gen" && (
          <div className="studio-main-grid">
            <div className="studio-controls-column">
              <div className="studio-panel studio-form-panel">
                <div className="studio-panel-header">
                  <div className="panel-label">VIDEO GENERATION</div>
                  <h3>Stock ComfyUI Wan 2.1 T2V</h3>
                </div>
                <label>
                  Prompt
                  <textarea
                    rows={5}
                    value={videoPrompt}
                    onChange={(e) => setVideoPrompt(e.target.value)}
                    placeholder="Describe the motion and scene"
                  />
                </label>
                <label>
                  Negative prompt
                  <textarea
                    rows={3}
                    value={videoNegativePrompt}
                    onChange={(e) => setVideoNegativePrompt(e.target.value)}
                    placeholder="Optional exclusions"
                  />
                </label>
                <div className="image-form-grid">
                  <label>
                    Dimensions
                    <select value={`${videoWidth}x${videoHeight}`} onChange={(e) => { const [w, h] = e.target.value.split("x").map(Number); setVideoWidth(w); setVideoHeight(h); }}>
                      <option value="320x192">Low VRAM 320 x 192</option>
                    </select>
                  </label>
                  <label>
                    Frame count
                    <input type="number" min="1" max="81" step="4" value={videoFrameCount} onChange={(e) => setVideoFrameCount(e.currentTarget.valueAsNumber)} />
                  </label>
                  <label>
                    FPS
                    <input type="number" min="1" max="24" step="1" value={videoFps} onChange={(e) => setVideoFps(e.currentTarget.valueAsNumber)} />
                  </label>
                  <label>
                    Seed
                    <input type="number" min="0" step="1" disabled={videoRandomSeed} value={videoSeed ?? 0} onChange={(e) => setVideoSeed(e.currentTarget.valueAsNumber)} />
                  </label>
                </div>
                <label className="image-checkbox">
                  <input type="checkbox" checked={videoRandomSeed} onChange={(e) => { setVideoRandomSeed(e.currentTarget.checked); if (e.currentTarget.checked) setVideoSeed(null); }} />
                  Use random seed
                </label>
                <button
                  className="btn btn--primary image-generate-button"
                  type="button"
                  disabled={!videoPrompt.trim() || videoGenerating || isVideoPolling}
                  onClick={handleVideoGenerate}
                >
                  {videoGenerating ? "Starting..." : isVideoPolling ? "Generating..." : "Generate Video"}
                </button>
                {videoError && <div className="error-banner" role="alert">{videoError}</div>}
                {videoMessage && <div className="success-banner" role="status">{videoMessage}</div>}
              </div>
            </div>
            <div className="studio-viewport-column">
              <div className="studio-panel studio-viewport-panel">
                <div className="studio-panel-header">
                  <div className="panel-label">OUTPUT</div>
                  <h3>Generated video assets</h3>
                </div>
                {videoJob && (
                  <div className="image-job-status">
                    <div>
                      <span className={`status-badge status-badge--job-${videoJob.status}`}>
                        {videoJob.cancellationRequested && isVideoPolling ? "Cancelling" : renderJobStatus(videoJob.status)}
                      </span>
                      <code>{videoJob.jobId}</code>
                    </div>
                    <div className="job-progress">
                      <span style={{ width: formatJobProgress(videoJob.progress) }} />
                      <small>{formatJobProgress(videoJob.progress)}</small>
                    </div>
                    {canCancelVideoGenerationJob(videoJob) && (
                      <button className="btn btn--secondary" type="button" disabled={videoCancelling} onClick={handleVideoCancel}>
                        {videoCancelling ? "Cancelling..." : "Cancel"}
                      </button>
                    )}
                  </div>
                )}
                {videoJob?.status === "failed" && (
                  <div className="error-banner" role="alert">
                    {videoJob.errorCode && `${videoJob.errorCode}: `}{videoJob.errorMessage || "Video generation failed."}
                  </div>
                )}
                {videoOutputAssetIds.length > 0 ? (
                  <div className="generated-video-list">
                    {videoOutputAssetIds.map((assetId) => {
                      const asset = videoOutputs.find((item) => item.assetId === assetId);
                      const fps = asset?.fpsNumerator != null && asset.fpsDenominator ? asset.fpsNumerator / asset.fpsDenominator : null;
                      return (
                        <article key={assetId}>
                          <div className="video-output-placeholder">VIDEO</div>
                          <div>
                            <strong>{asset?.originalFilename || "Managed video asset"}</strong>
                            <code>{assetId}</code>
                            <span>{asset ? [asset.mediaContainer, asset.codec, fps != null ? `${fps.toFixed(2)} FPS` : null].filter(Boolean).join(" / ") || "Metadata pending" : "Metadata pending"}</span>
                          </div>
                          <button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>View in Assets</button>
                        </article>
                      );
                    })}
                  </div>
                ) : (
                  <div className="image-output-empty">
                    <div className="empty-state__glyph">VID</div>
                    <p>{isVideoPolling ? "Generation is in progress." : "Completed managed video references and metadata will appear here."}</p>
                  </div>
                )}
              </div>
            </div>
          </div>
        )}

        {studioMode === "review" && (
          <div className="studio-main-grid">
            <div className="studio-controls-column">
              <div className="studio-panel studio-form-panel">
                <div className="studio-panel-header">
                  <div className="panel-label">PROCESSING REVIEW</div>
                  <h3>Inspect, Approve, Deploy</h3>
                </div>
                <div className="review-jobs-panel">
                  <div className="review-panel-heading">
                    <h3>Processing Jobs ({reviewJobs.length})</h3>
                  </div>
                  {reviewJobs.length === 0 ? (
                    <div className="empty-state empty-state--inline">
                      <div className="empty-state__glyph">3D</div>
                      <p>No model3d.processing jobs found.</p>
                    </div>
                  ) : (
                    <ul className="review-job-list" role="listbox">
                      {reviewJobs.map((job) => {
                        const isSelected = selectedReviewJobId === job.jobId;
                        const isReviewable = job.status === "completed" && (job.errorCode === "READY_FOR_REVIEW" || job.errorCode === "NEEDS_REVIEW");
                        return (
                          <li
                            key={job.jobId}
                            className={`review-job-item ${isSelected ? "review-job-item--selected" : ""} ${isReviewable ? "review-job-item--reviewable" : ""}`}
                            onClick={() => handleReviewJobSelect(job.jobId)}
                            role="option"
                            aria-selected={isSelected}
                          >
                            <div className="review-job-item__main">
                              <code title={job.jobId}>{job.jobId.slice(0, 12)}...</code>
                              <span className={`status-badge status-badge--job-${job.status}`}>
                                {job.cancellationRequested && job.status === "running" ? "Cancelling" : renderJobStatus(job.status)}
                              </span>
                            </div>
                            <div className="review-job-item__meta">
                              <div className="job-progress">
                                <span style={{ width: formatJobProgress(job.progress) }} />
                                <small>{formatJobProgress(job.progress)}</small>
                              </div>
                              <small>Created {new Date(job.createdAtMs).toLocaleString()}</small>
                              {job.errorCode && (
                                <span className={`review-job-status-code review-job-status-code--${job.errorCode.toLowerCase()}`}>
                                  {job.errorCode}
                                </span>
                              )}
                            </div>
                            {isReviewable && <span className="review-indicator" title="Ready for review">★</span>}
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </div>
                {selectedReviewJobId && reviewJobResult ? (
                  <div className="review-detail">
                    <article className="review-detail-content">
                      <header className="review-detail__header">
                        <div>
                          <h3>Processing Job Details</h3>
                          <code>{reviewJobResult.jobId}</code>
                        </div>
                        <div className="review-status-group">
                          <span className={`status-badge status-badge--job-${reviewJobResult.status}`}>
                            {formatProcessingStatus(reviewJobResult.status).label}
                          </span>
                          <span className={`review-stage ${reviewJobResult.processingStage ? "review-stage--complete" : ""}`}>
                            {reviewJobResult.processingStage || "Complete"}
                          </span>
                        </div>
                      </header>
                      {reviewJobResult.errorCode && reviewJobResult.errorMessage && (
                        <div className="error-banner" role="alert">
                          {reviewJobResult.errorCode}: {reviewJobResult.errorMessage}
                        </div>
                      )}
                      <div className="review-grid">
                        <section className="review-panel review-panel--source">
                          <h4>Source Asset</h4>
                          {reviewJobResult.assetIds.length > 0 && (
                            <div className="asset-reference">
                              <strong>Generated Master GLB:</strong>
                              <code>{reviewJobResult.assetIds[0]}</code>
                            </div>
                          )}
                        </section>
                        <section className="review-panel review-panel--outputs">
                          <h4>Interactive 3D Preview</h4>
                          {reviewJobResult.assetIds.length > 0 && (
                            <div className="model3d-viewer-wrapper">
                              <Model3dViewer assetId={reviewJobResult.assetIds[0]} presetView="perspective" />
                            </div>
                          )}
                          <div className="output-paths">
                            {reviewJobResult.outputMasterPath && (
                              <div className="output-item">
                                <span className="output-label">Master GLB</span>
                                <code className="output-path">{reviewJobResult.outputMasterPath}</code>
                              </div>
                            )}
                            {reviewJobResult.lod0Path && (
                              <div className="output-item">
                                <span className="output-label">LOD0</span>
                                <code className="output-path">{reviewJobResult.lod0Path}</code>
                              </div>
                            )}
                            {reviewJobResult.lod1Path && (
                              <div className="output-item">
                                <span className="output-label">LOD1</span>
                                <code className="output-path">{reviewJobResult.lod1Path}</code>
                              </div>
                            )}
                            {reviewJobResult.lod2Path && (
                              <div className="output-item">
                                <span className="output-label">LOD2</span>
                                <code className="output-path">{reviewJobResult.lod2Path}</code>
                              </div>
                            )}
                          </div>
                        </section>
                          <section className="review-panel review-panel--unity-deploy">
                            <h4>Export / Deploy</h4>
                            <div className="unity-config-form">
                              <label className="form-field-label">Target Project Directory:</label>
                              <div className="unity-path-input-group">
                              <input
                                type="text"
                                className="studio-input"
                                value={unityProjectRoot}
                                onChange={(e) => handleValidateUnityPath(e.target.value)}
                                placeholder="e.g. C:\Users\Mak Tech\development\Games\MyGame"
                              />
                              <button
                                className="btn btn--secondary btn--sm"
                                type="button"
                                onClick={async () => {
                                  const disc = await discoverUnityProject();
                                  if (disc) handleValidateUnityPath(disc);
                                }}
                              >
                                Auto-Detect
                              </button>
                            </div>
                            {unityValidation && (
                              <div className={`unity-validation-tag ${unityValidation.isValid ? "unity-validation--valid" : "unity-validation--invalid"}`}>
                                {unityValidation.isValid
                                  ? `Valid Project (${unityValidation.unityVersion || "Version detected"})`
                                  : `Project Check Failed: ${unityValidation.errorMessage || "Missing Assets/ or ProjectSettings/"}`}
                              </div>
                            )}
                             <div className="unity-category-picker">
                               <label className="form-field-label">Asset Type</label>
                               <select
                                 value={unityCategory}
                                 onChange={(e) => setUnityCategory(e.target.value)}
                                 className="studio-select"
                               >
                                 <option value="">Auto Detect</option>
                                 <option value="Vehicle">Vehicle</option>
                                 <option value="Character">Character</option>
                                 <option value="Creature">Creature</option>
                                 <option value="Environment">Environment</option>
                                 <option value="Prop">Prop</option>
                                 <option value="Weapon">Weapon</option>
                                 <option value="Building">Building</option>
                                 <option value="Vegetation">Vegetation</option>
                                 <option value="Furniture">Furniture</option>
                                 <option value="Equipment">Equipment</option>
                                 <option value="Material">Material</option>
                               </select>
                             </div>
                            <button
                              className="btn btn--primary btn--large unity-deploy-btn"
                              type="button"
                              disabled={reviewDeploying || !unityProjectRoot || !unityValidation?.isValid || !reviewJobResult.assetIds.length}
                              onClick={handleReviewDeployToUnity}
                            >
                              {reviewDeploying ? "Exporting..." : "Export Asset"}
                            </button>
                            {reviewDeploymentSuccess && reviewDeploymentResult && (
                              <div className="deployment-success-card">
                                <div className="deploy-success-badge">DEPLOYMENT VERIFIED</div>
                                <dl className="deploy-dl">
                                  <div>
                                    <dt>Deployed Asset:</dt>
                                    <dd><code>{reviewDeploymentResult.deployedPath}</code></dd>
                                  </div>
                                  <div>
                                    <dt>Meta File:</dt>
                                    <dd><code>{reviewDeploymentResult.metaPath}</code></dd>
                                  </div>
                                  <div>
                                    <dt>File Size:</dt>
                                    <dd>{formatFileSize(reviewDeploymentResult.fileSize)} ({reviewDeploymentResult.bytesWritten} bytes written)</dd>
                                  </div>
                                  <div>
                                    <dt>SHA-256 Checksum:</dt>
                                    <dd><code>{reviewDeploymentResult.checksum.slice(0, 16)}...</code></dd>
                                  </div>
                                  <div>
                                    <dt>Destination Folder:</dt>
                                    <dd><code>{reviewDeploymentResult.destinationFolder}</code></dd>
                                  </div>
                                </dl>
                              </div>
                            )}
                          </div>
                        </section>
                        <section className="review-panel review-panel--reports">
                          <h4>Asset Readiness Report</h4>
                          <AssetReadinessReport
                            jobResult={reviewJobResult}
                            onDeploy={handleReviewDeployToUnity}
                          />
                        </section>
                        <section className="review-panel review-panel--actions">
                          <h4>Review Actions</h4>
                          <div className="review-actions">
                            {canApproveAsset(reviewJobResult.status as any) && (
                              <button
                                className="btn btn--primary btn--large"
                                onClick={handleReviewApprove}
                                disabled={reviewActionLoading === "approve"}
                              >
                                {reviewActionLoading === "approve" ? "Approving..." : "APPROVE ASSET"}
                              </button>
                            )}
                            {canRejectAsset(reviewJobResult.status as any) && (
                              <button
                                className="btn btn--danger btn--large"
                                onClick={handleReviewReject}
                                disabled={reviewActionLoading === "reject"}
                              >
                                {reviewActionLoading === "reject" ? "Rejecting..." : "REJECT"}
                              </button>
                            )}
                            {canReprocessAsset(reviewJobResult.status as any) && (
                              <button
                                className="btn btn--secondary btn--large"
                                onClick={handleReviewReprocess}
                                disabled={reviewActionLoading === "reprocess"}
                              >
                                {reviewActionLoading === "reprocess" ? "Reprocessing..." : "REPROCESS"}
                              </button>
                            )}
                          </div>
                        </section>
                      </div>
                    </article>
                  </div>
                ) : selectedReviewJobId && !reviewJobResult ? (
                    <div className="loading-placeholder">Loading job result...</div>
                  ) : (
                    <div className="empty-state empty-state--detail">
                      <div className="empty-state__glyph">3D</div>
                      <h3>Select a processing job</h3>
                      <p>Choose a model3d.processing job from the list to review its outputs and take action.</p>
                    </div>
                  )}
              </div>
            </div>
            <div className="studio-viewport-column">
              <div className="studio-panel studio-viewport-panel">
                <div className="studio-panel-header">
                  <div className="panel-label">INTERACTIVE 3D VIEWPORT</div>
                </div>
                <div className="viewport-canvas-container">
                  {selectedReviewJobId && reviewJobResult && reviewJobResult.assetIds.length > 0 ? (
                    <Model3dViewer assetId={reviewJobResult.assetIds[0]} presetView="perspective" />
                  ) : (
                    <div className="empty-viewport-placeholder">
                      <div className="viewport-icon">🧊</div>
                      <h4>No 3D Model Loaded</h4>
                      <p>Select a processing job to inspect the 3D model.</p>
                    </div>
                  )}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </section>
  );
}

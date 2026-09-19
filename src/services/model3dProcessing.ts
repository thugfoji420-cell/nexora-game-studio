import { invoke } from "@tauri-apps/api/core";
import type {
  CreateModel3dProcessingJobInput,
  CreateModel3dProcessingJobResult,
  CreateUnityDeliveryInput,
  Model3dProcessingResult,
  Model3dProcessingProfile,
  Model3dProcessingQuality,
  UpdateUnityDeliveryStatusInput,
  UnityDelivery,
  UnityDeploymentDto,
  ApproveModel3dAssetInput,
  RejectModel3dAssetInput,
  ReprocessModel3dAssetInput,
  AssetProcessingStatus,
  AssetApproval,
  ProcessingReport,
} from "../types/model3dProcessing";
import type { EngineTargetInfo } from "../types/core";
import { MODEL3D_GENERATION_PROFILE_ID, MODEL3D_OUTPUT_FORMAT, MODEL3D_QUALITY } from "../types/core";

export const createModel3dProcessingJob = (input: CreateModel3dProcessingJobInput): Promise<CreateModel3dProcessingJobResult> =>
  invoke<CreateModel3dProcessingJobResult>("create_model3d_processing_job", { input });

export const getModel3dProcessingResult = (jobId: string): Promise<Model3dProcessingResult> =>
  invoke<Model3dProcessingResult>("get_model3d_processing_result", { jobId });

export const approveModel3dAsset = (input: ApproveModel3dAssetInput): Promise<void> =>
  invoke("approve_model3d_asset", { assetId: input.assetId, processingJobId: input.processingJobId });

export const approveImageAsset = (assetId: string, generationJobId?: string | null): Promise<void> =>
  invoke("approve_image_asset", { assetId, generationJobId: generationJobId ?? null });

export const rejectModel3dAsset = (input: RejectModel3dAssetInput): Promise<void> =>
  invoke("reject_model3d_asset", { assetId: input.assetId, processingJobId: input.processingJobId, rejectionReason: input.rejectionReason });

export const reprocessModel3dAsset = (input: ReprocessModel3dAssetInput): Promise<CreateModel3dProcessingJobResult> =>
  invoke<CreateModel3dProcessingJobResult>("reprocess_model3d_asset", { 
    assetId: input.assetId, 
    processingJobId: input.processingJobId,
    profile: input.profile,
    quality: input.quality,
  });

export const formatProcessingReport = (report: ProcessingReport | null): string[] => {
  if (!report) return ["No processing report available."];
  
  const lines: string[] = [];
  
  lines.push(`Vertices: ${report.verticesBefore.toLocaleString()} → ${report.verticesAfter.toLocaleString()} (${(report.verticesBefore - report.verticesAfter).toLocaleString()} removed)`);
  lines.push(`Faces: ${report.facesBefore.toLocaleString()} → ${report.facesAfter.toLocaleString()} (${(report.facesBefore - report.facesAfter).toLocaleString()} removed)`);
  lines.push(`Triangles: ${report.trianglesBefore.toLocaleString()} → ${report.trianglesAfter.toLocaleString()} (${(report.trianglesBefore - report.trianglesAfter).toLocaleString()} removed)`);
  lines.push(`Objects: ${report.objectsBefore} → ${report.objectsAfter}`);
  lines.push(`Materials: ${report.materialsBefore} → ${report.materialsAfter}`);
  lines.push(`Duplicate vertices removed: ${report.duplicateVerticesRemoved.toLocaleString()}`);
  lines.push(`Degenerate faces removed: ${report.degenerateFacesRemoved.toLocaleString()}`);
  lines.push(`Loose geometry removed: ${report.looseGeometryRemoved.toLocaleString()}`);
  lines.push(`Normals recalculated: ${report.normalsRecalculated ? "Yes" : "No"}`);
  lines.push(`Transforms normalized: ${report.transformsNormalized ? "Yes" : "No"}`);
  lines.push(`Scale normalized: ${report.scaleNormalized ? "Yes" : "No"}`);
  lines.push(`Orientation normalized: ${report.orientationNormalized ? "Yes" : "No"}`);
  lines.push(`Duplicate geometry removed: ${report.duplicateGeometryRemoved.toLocaleString()}`);
  lines.push(`Tiny components removed: ${report.tinyComponentsRemoved.toLocaleString()}`);
  
  if (report.lod0Triangles !== null) lines.push(`LOD0 triangles: ${report.lod0Triangles.toLocaleString()}`);
  if (report.lod1Triangles !== null) lines.push(`LOD1 triangles: ${report.lod1Triangles.toLocaleString()}`);
  if (report.lod2Triangles !== null) lines.push(`LOD2 triangles: ${report.lod2Triangles.toLocaleString()}`);
  
  lines.push(`Category: ${report.categoryDetected || "Unknown"}`);
  lines.push(`Category confidence: ${(report.categoryConfidence * 100).toFixed(1)}%`);
  lines.push(`Category needs review: ${report.categoryNeedsReview ? "Yes" : "No"}`);
  
  lines.push(`Vehicle detected: ${report.vehicleDetected ? "Yes" : "No"}`);
  lines.push(`Wheel candidates: ${report.wheelCandidates}`);
  lines.push(`Wheel separation possible: ${report.wheelSeparationPossible ? "Yes" : "No"}`);
  lines.push(`Vehicle confidence: ${(report.vehicleConfidence * 100).toFixed(1)}%`);
  lines.push(`Material status: ${report.materialStatus}`);
  
  if (report.warnings.length > 0) {
    lines.push("Warnings:");
    report.warnings.forEach(w => lines.push(`  - ${w}`));
  }
  
  if (report.errors.length > 0) {
    lines.push("Errors:");
    report.errors.forEach(e => lines.push(`  - ${e}`));
  }
  
  return lines;
};

export const formatProcessingStatus = (status: string): { label: string; tone: "good" | "warning" | "muted" | "bad" } => {
  switch (status) {
    case "ready_for_review":
      return { label: "Ready for Review", tone: "good" };
    case "needs_review":
      return { label: "Needs Review", tone: "warning" };
    case "processing":
      return { label: "Processing", tone: "muted" };
    case "approved":
      return { label: "Approved", tone: "good" };
    case "rejected":
      return { label: "Rejected", tone: "bad" };
    default:
      return { label: status, tone: "muted" };
  }
};

export const canApproveAsset = (status: AssetProcessingStatus): boolean =>
  status === "ready_for_review" || status === "needs_review";

export const canRejectAsset = (status: AssetProcessingStatus): boolean =>
  status === "ready_for_review" || status === "needs_review" || status === "processing";

export const canReprocessAsset = (status: AssetProcessingStatus): boolean =>
  status !== "approved"; // Can reprocess anything except approved

export const createUnityDelivery = (input: CreateUnityDeliveryInput): Promise<string> =>
  invoke<string>("create_unity_delivery", { input });

export const updateUnityDeliveryStatus = (input: UpdateUnityDeliveryStatusInput): Promise<void> =>
  invoke<void>("update_unity_delivery_status", { input });

export const getUnityDelivery = (deliveryId: string): Promise<UnityDelivery> =>
  invoke<UnityDelivery>("get_unity_delivery", { deliveryId });

export const listUnityDeliveries = (
  targetId?: string,
  assetId?: string,
): Promise<UnityDelivery[]> =>
  invoke<UnityDelivery[]>("list_unity_deliveries", { targetId, assetId });

export const verifyAssetApprovedForUnityDelivery = (
  assetId: string,
): Promise<UnityDelivery> =>
  invoke<UnityDelivery>("verify_asset_approved_for_unity_delivery", { assetId });

export const discoverUnityProject = (): Promise<string | null> =>
  invoke<string | null>("discover_unity_project");

export const detectEngineTargets = (): Promise<EngineTargetInfo[]> =>
  invoke<EngineTargetInfo[]>("detect_engine_targets");

export const validateUnityProject = (
  projectRoot: string,
): Promise<{ isValid: boolean; unityVersion: string | null; projectRoot: string; errorMessage: string | null }> =>
  invoke<{ isValid: boolean; unityVersion: string | null; projectRoot: string; errorMessage: string | null }>(
    "validate_unity_project",
    { projectRoot },
  );

export const deployModel3dToUnity = (
  assetId: string,
  targetId: string,
  category?: string,
): Promise<UnityDeploymentDto> =>
  invoke<UnityDeploymentDto>("deploy_model3d_to_unity", { assetId, targetId, category });

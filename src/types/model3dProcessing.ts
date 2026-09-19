import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export const MODEL3D_PROCESSING_PROFILE_ID = "foundation.processing.v1" as const;

export type Model3dProcessingProfile = 
  | "generic" 
  | "vehicle" 
  | "character" 
  | "environment" 
  | "prop" 
  | "weapon" 
  | "building" 
  | "vegetation" 
  | "furniture" 
  | "equipment";

export type Model3dProcessingQuality = "master" | "mobile_high" | "mobile_balanced" | "mobile_low";

export interface Model3dProcessingRequest {
  schemaVersion: 1;
  sourceAssetId: string;
  profile: Model3dProcessingProfile;
  quality: Model3dProcessingQuality;
}

export interface CreateModel3dProcessingJobInput {
  schemaVersion: 1;
  sourceAssetId: string;
  profile: Model3dProcessingProfile;
  quality: Model3dProcessingQuality;
  providerId?: string;
}

export interface CreateModel3dProcessingJobResult {
  job: JobInfo;
  compatibility: ProviderFit;
}

export type Model3dProcessingStatus = "completed" | "ready_for_review" | "needs_review" | "failed";

export interface ProcessingReport {
  verticesBefore: number;
  verticesAfter: number;
  facesBefore: number;
  facesAfter: number;
  trianglesBefore: number;
  trianglesAfter: number;
  objectsBefore: number;
  objectsAfter: number;
  materialsBefore: number;
  materialsAfter: number;
  duplicateVerticesRemoved: number;
  degenerateFacesRemoved: number;
  looseGeometryRemoved: number;
  normalsRecalculated: boolean;
  transformsNormalized: boolean;
  scaleNormalized: boolean;
  orientationNormalized: boolean;
  duplicateGeometryRemoved: number;
  tinyComponentsRemoved: number;
  lod0Triangles: number | null;
  lod1Triangles: number | null;
  lod2Triangles: number | null;
  collisionGenerated?: boolean | null;
  collisionTriangles?: number | null;
  qualityScore?: number | null;
  qualityRating?: string | null;
  categoryDetected: string;
  categoryConfidence: number;
  categoryNeedsReview: boolean;
  categoryDetails: Record<string, unknown>;
  vehicleDetected: boolean;
  wheelCandidates: number;
  wheelSeparationPossible: boolean;
  vehicleConfidence: number;
  materialStatus: string;
  warnings: string[];
  errors: string[];
}

export interface Model3dProcessingResult {
  jobId: string;
  status: Model3dProcessingStatus;
  assetIds: string[];
  processingStage: string | null;
  progress: number;
  outputMasterPath: string | null;
  lod0Path: string | null;
  lod1Path: string | null;
  lod2Path: string | null;
  vehicleAnalysisPath: string | null;
  materialStatus: string | null;
  preAnalysisReport: ProcessingReport | null;
  postAnalysisReport: ProcessingReport | null;
  errorCode: string | null;
  errorMessage: string | null;
}

export type UnityDeliveryStatus = 
  | "not_delivered"
  | "preparing"
  | "ready_for_import"
  | "importing"
  | "delivered"
  | "needs_review"
  | "failed";

export type AssetProcessingStatus = "raw" | "processing" | "needs_review" | "ready_for_review" | "approved" | "rejected";

export interface AssetApproval {
  assetId: string;
  status: "pending" | "approved" | "rejected";
  approvedBy: string | null;
  approvedAtMs: number | null;
  rejectionReason: string | null;
  approvedJobId: string | null;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface ApproveModel3dAssetInput {
  assetId: string;
  processingJobId: string;
}

export interface RejectModel3dAssetInput {
  assetId: string;
  processingJobId: string;
  rejectionReason?: string;
}

export interface ReprocessModel3dAssetInput {
  assetId: string;
  processingJobId: string;
  profile?: Model3dProcessingProfile;
  quality?: Model3dProcessingQuality;
}

export interface UnityDelivery {
  deliveryId: string;
  assetId: string;
  approvalId: string;
  processingJobId: string;
  targetId: string;
  approvedArtifactChecksum: string;
  unityProjectRoot: string;
  unityDestinationFolder: string;
  deliveryRevision: number;
  status: string;
  errorMessage: string | null;
  importedAssetPath: string | null;
  prefabPath: string | null;
  validationReport: string | null;
  createdAtMs: number;
  updatedAtMs: number;
  completedAtMs: number | null;
}

export interface CreateUnityDeliveryInput {
  assetId: string;
  processingJobId: string;
  targetId: string;
  revision: number;
}

export interface UpdateUnityDeliveryStatusInput {
  deliveryId: string;
  status: string;
  errorMessage?: string;
  importedAssetPath?: string;
  prefabPath?: string;
  validationReport?: string;
}

export interface UnityDeploymentDto {
  deliveryId: string;
  assetId: string;
  targetId: string;
  unityProjectRoot: string;
  destinationFolder: string;
  deployedPath: string;
  metaPath: string;
  fileSize: number;
  bytesWritten: number;
  checksum: string;
  category: string;
}

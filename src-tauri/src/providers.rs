use crate::hardware::HardwareSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const PROVIDER_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Ord, PartialOrd, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    TextToImage,
    ImageToImage,
    Inpainting,
    BackgroundRemoval,
    Upscaling,
    TextureGeneration,
    TextToVideo,
    ImageToVideo,
    VideoToVideo,
    ThreeDGeneration,
    #[serde(rename = "model3d.text_to_3d")]
    Model3dTextTo3d,
    #[serde(rename = "model3d.image_to_3d")]
    Model3dImageTo3d,
    #[serde(rename = "hunyuan.text_to_3d")]
    HunyuanTextTo3d,
    #[serde(rename = "hunyuan.image_to_3d")]
    HunyuanImageTo3d,
    MeshProcessing,
    MaterialGeneration,
    PromptGeneration,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderType {
    ImageGeneration,
    VideoGeneration,
    ThreeDProcessing,
    MaterialProcessing,
    Utility,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionMode {
    InternalMock,
    LocalHttp,
    ControlledCli,
    PythonWorker,
    ExternalApplication,
    RemoteApi,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderNature {
    Mock,
    Real,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthCheckType {
    Internal,
    LocalHttp,
    Process,
    Manual,
}

#[derive(Clone, Copy, Debug, Deserialize, Ord, PartialOrd, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    JobWorkingDirectoryRead,
    JobWorkingDirectoryWrite,
    InputAssetsRead,
    OutputStagingWrite,
    Network,
    Gpu,
    ExternalApplication,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    Minimal,
    Standard,
    Heavy,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HardwareRequirements {
    pub min_ram_mib: Option<u64>,
    pub recommended_ram_mib: Option<u64>,
    pub min_vram_mib: Option<u64>,
    pub recommended_vram_mib: Option<u64>,
    pub gpu_required: bool,
    pub supported_gpu_vendors: Vec<String>,
    pub cpu_fallback: bool,
    pub min_disk_mib: Option<u64>,
    pub exclusive: bool,
    pub resource_class: ResourceClass,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenseStatus {
    Known,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LicenseMetadata {
    pub status: LicenseStatus,
    pub name: Option<String>,
    pub model_license: Option<String>,
    pub commercial_use_allowed: Option<bool>,
    pub source_reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderManifest {
    pub schema_version: u32,
    pub provider_id: String,
    pub display_name: String,
    pub version: String,
    pub provider_type: ProviderType,
    pub execution_mode: ExecutionMode,
    pub nature: ProviderNature,
    pub classification: Classification,
    pub enabled: bool,
    pub capabilities: Vec<Capability>,
    pub health_check: HealthCheckType,
    pub requirements: HardwareRequirements,
    pub license: LicenseMetadata,
    pub permissions: Vec<Permission>,
}

impl ProviderManifest {
    pub fn from_json(json: &str) -> Result<Self, ProviderError> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| ProviderError::MalformedManifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.schema_version != PROVIDER_MANIFEST_SCHEMA_VERSION {
            return Err(ProviderError::UnknownSchema(self.schema_version));
        }
        if !valid_provider_id(&self.provider_id) {
            return Err(ProviderError::InvalidProviderId(self.provider_id.clone()));
        }
        if self.display_name.trim().is_empty() || !valid_version(&self.version) {
            return Err(ProviderError::InvalidMetadata);
        }
        if self.capabilities.is_empty() || has_duplicates(&self.capabilities) {
            return Err(ProviderError::InvalidCapabilities);
        }
        if has_duplicates(&self.permissions) {
            return Err(ProviderError::InvalidPermissions);
        }
        if self.enabled && self.execution_mode != ExecutionMode::InternalMock {
            return Err(ProviderError::UnsupportedEnabledExecutionMode);
        }
        if self.execution_mode == ExecutionMode::InternalMock
            && (self.nature != ProviderNature::Mock
                || self.classification != Classification::Local
                || self.health_check != HealthCheckType::Internal)
        {
            return Err(ProviderError::InvalidMockConfiguration);
        }
        if self.classification == Classification::Remote
            && self.execution_mode != ExecutionMode::RemoteApi
        {
            return Err(ProviderError::InvalidClassification);
        }
        if self.execution_mode == ExecutionMode::RemoteApi
            && self.classification != Classification::Remote
        {
            return Err(ProviderError::InvalidClassification);
        }
        validate_requirements(&self.requirements)?;
        validate_permissions(self)?;
        validate_license(&self.license)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
    Misconfigured,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider_id: String,
    pub state: HealthState,
    pub checked_at: DateTime<Utc>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityStatus {
    Compatible,
    CompatibleWithWarning,
    Incompatible,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Ord, PartialOrd, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityReasonCode {
    RamBelowMinimum,
    RamBelowRecommended,
    RamUnknown,
    VramBelowMinimum,
    VramBelowRecommended,
    VramUnknown,
    GpuRequired,
    GpuUnknown,
    GpuVendorUnsupported,
    GpuVendorUnknown,
    DiskBelowMinimum,
    DiskUnknown,
    CpuFallback,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub status: CompatibilityStatus,
    pub reason_codes: Vec<CompatibilityReasonCode>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub manifest: ProviderManifest,
    pub health: ProviderHealth,
    pub license: LicenseMetadata,
    pub fit: Compatibility,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolDescriptor {
    pub protocol: &'static str,
    pub transport: &'static str,
    pub protocol_version: u32,
    pub direction: &'static str,
}

/// Describes the future worker contract. Phase 5 does not spawn provider processes.
pub fn protocol_descriptor() -> ProtocolDescriptor {
    ProtocolDescriptor {
        protocol: "JSON-RPC 2.0",
        transport: "stdio",
        protocol_version: 1,
        direction: "future",
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("malformed provider manifest: {0}")]
    MalformedManifest(String),
    #[error("unsupported provider manifest schema: {0}")]
    UnknownSchema(u32),
    #[error("invalid provider id: {0}")]
    InvalidProviderId(String),
    #[error("invalid provider display name or version")]
    InvalidMetadata,
    #[error("capabilities must be non-empty and unique")]
    InvalidCapabilities,
    #[error("permissions must be unique and recognized")]
    InvalidPermissions,
    #[error("only internalMock providers may be enabled in Phase 5")]
    UnsupportedEnabledExecutionMode,
    #[error("invalid internal mock configuration")]
    InvalidMockConfiguration,
    #[error("execution mode and classification do not match")]
    InvalidClassification,
    #[error("invalid hardware requirements: {0}")]
    InvalidRequirements(String),
    #[error("permissions exceed the execution mode's least-privilege boundary")]
    ExcessivePermissions,
    #[error("invalid license metadata")]
    InvalidLicense,
    #[error("provider id is already registered: {0}")]
    DuplicateProvider(String),
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("no suitable local provider for capability")]
    NoSuitableProvider,
}

#[derive(Clone, Debug)]
struct RegisteredProvider {
    manifest: ProviderManifest,
    health: ProviderHealth,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, RegisteredProvider>,
}

impl ProviderRegistry {
    pub fn phase5() -> Result<Self, ProviderError> {
        let mut registry = Self::default();
        registry.register(mock_basic(), HealthState::Healthy)?;
        registry.register(mock_gpu_heavy(), HealthState::Healthy)?;
        registry.register(mock_unavailable(), HealthState::Unavailable)?;
        Ok(registry)
    }

    pub fn phase6(local_enabled: bool) -> Result<Self, ProviderError> {
        let mut registry = Self::phase5()?;
        registry.register_core_local_http(local_a1111(local_enabled), HealthState::Unavailable)?;
        Ok(registry)
    }

    pub fn phase7(image_enabled: bool, video_enabled: bool) -> Result<Self, ProviderError> {
        let mut registry = Self::phase6(image_enabled)?;
        registry
            .register_core_local_http(local_comfyui(video_enabled), HealthState::Unavailable)?;
        Ok(registry)
    }

    pub fn phase8(image_enabled: bool, video_enabled: bool, openrouter_configured: bool) -> Result<Self, ProviderError> {
        let mut registry = Self::phase7(image_enabled, video_enabled)?;
        registry.register_core_local_http(local_hunyuan(true), HealthState::Healthy)?;
        registry.register_core_local_cli(local_blender(), HealthState::Healthy)?;
        let openrouter_health = crate::openrouter::health(&crate::openrouter::OpenRouterConfig {
            enabled: openrouter_configured,
            ..Default::default()
        });
        registry.register_remote_api(
            crate::openrouter::provider_manifest(),
            openrouter_health.state,
        )?;
        Ok(registry)
    }

    pub fn register(
        &mut self,
        manifest: ProviderManifest,
        health: HealthState,
    ) -> Result<(), ProviderError> {
        manifest.validate()?;
        if manifest.execution_mode == ExecutionMode::LocalHttp {
            return Err(ProviderError::UnsupportedEnabledExecutionMode);
        }
        self.insert(manifest, health)
    }

    fn register_remote_api(
        &mut self,
        manifest: ProviderManifest,
        health: HealthState,
    ) -> Result<(), ProviderError> {
        let mut validated = manifest.clone();
        validated.enabled = false;
        validated.validate()?;
        self.insert(manifest, health)
    }

    fn register_core_local_http(
        &mut self,
        manifest: ProviderManifest,
        health: HealthState,
    ) -> Result<(), ProviderError> {
        if !matches!(
            manifest.provider_id.as_str(),
            "local.a1111" | "local.comfyui" | "local.hunyuan"
        ) || manifest.execution_mode != ExecutionMode::LocalHttp
            || manifest.nature != ProviderNature::Real
        {
            return Err(ProviderError::InvalidMetadata);
        }
        let mut validated = manifest.clone();
        validated.enabled = false;
        validated.validate()?;
        self.insert(manifest, health)
    }

    /// Core local CLI providers (Blender) follow the same pattern as core
    /// local HTTP providers: validate a disabled copy, then insert the
    /// enabled manifest. The worker resolves the executable at job time, so
    /// registration is static and the health refresher never probes it.
    fn register_core_local_cli(
        &mut self,
        manifest: ProviderManifest,
        health: HealthState,
    ) -> Result<(), ProviderError> {
        if manifest.provider_id != "local.blender"
            || manifest.execution_mode != ExecutionMode::ControlledCli
            || manifest.nature != ProviderNature::Real
        {
            return Err(ProviderError::InvalidMetadata);
        }
        let mut validated = manifest.clone();
        validated.enabled = false;
        validated.validate()?;
        self.insert(manifest, health)
    }

    fn insert(
        &mut self,
        manifest: ProviderManifest,
        health: HealthState,
    ) -> Result<(), ProviderError> {
        let id = manifest.provider_id.clone();
        if self.providers.contains_key(&id) {
            return Err(ProviderError::DuplicateProvider(id));
        }
        self.providers.insert(
            id.clone(),
            RegisteredProvider {
                manifest,
                health: ProviderHealth {
                    provider_id: id.clone(),
                    state: health,
                    checked_at: Utc::now(),
                    detail: None,
                },
            },
        );
        Ok(())
    }

    pub fn set_local_a1111_state(
        &mut self,
        enabled: bool,
        state: HealthState,
        detail: Option<String>,
    ) -> Result<ProviderHealth, ProviderError> {
        let provider = self
            .providers
            .get_mut("local.a1111")
            .ok_or_else(|| ProviderError::UnknownProvider("local.a1111".into()))?;
        provider.manifest.enabled = enabled;
        provider.health.state = state;
        provider.health.checked_at = Utc::now();
        provider.health.detail = detail;
        Ok(provider.health.clone())
    }

    pub fn set_local_comfyui_state(
        &mut self,
        enabled: bool,
        reachable: bool,
        compatible: bool,
        detail: Option<String>,
    ) -> Result<ProviderHealth, ProviderError> {
        let provider = self
            .providers
            .get_mut("local.comfyui")
            .ok_or_else(|| ProviderError::UnknownProvider("local.comfyui".into()))?;
        provider.manifest.enabled = enabled;
        provider.health.state = if !enabled || !reachable {
            HealthState::Unavailable
        } else if !compatible {
            HealthState::Misconfigured
        } else {
            HealthState::Healthy
        };
        provider.health.checked_at = Utc::now();
        provider.health.detail = detail;
        Ok(provider.health.clone())
    }

    pub fn set_local_hunyuan_state(
        &mut self,
        enabled: bool,
        reachable: bool,
        compatible: bool,
        detail: Option<String>,
    ) -> Result<ProviderHealth, ProviderError> {
        let provider = self
            .providers
            .get_mut("local.hunyuan")
            .ok_or_else(|| ProviderError::UnknownProvider("local.hunyuan".into()))?;
        provider.manifest.enabled = enabled;
        provider.health.state = if !enabled || !reachable {
            HealthState::Unavailable
        } else if !compatible {
            HealthState::Misconfigured
        } else {
            HealthState::Healthy
        };
        provider.health.checked_at = Utc::now();
        provider.health.detail = detail;
        Ok(provider.health.clone())
    }

    pub fn set_openrouter_state(
        &mut self,
        enabled: bool,
        state: HealthState,
        detail: Option<String>,
    ) -> Result<ProviderHealth, ProviderError> {
        let provider = self
            .providers
            .get_mut("remote.openrouter")
            .ok_or_else(|| ProviderError::UnknownProvider("remote.openrouter".into()))?;
        provider.manifest.enabled = enabled;
        provider.health.state = state;
        provider.health.checked_at = Utc::now();
        provider.health.detail = detail;
        Ok(provider.health.clone())
    }

    pub fn validate_video_generation(
        &self,
        provider_id: &str,
        capability: Capability,
        snapshot: &HardwareSnapshot,
    ) -> Result<ProviderView, ProviderError> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| ProviderError::UnknownProvider(provider_id.into()))?;
        let view = provider_view(provider, snapshot);
        if view.manifest.provider_id != "local.comfyui"
            || view.manifest.nature != ProviderNature::Real
            || view.manifest.execution_mode != ExecutionMode::LocalHttp
            || !view.manifest.enabled
            || !view.manifest.capabilities.contains(&capability)
            || view.health.state != HealthState::Healthy
            || !matches!(
                view.fit.status,
                CompatibilityStatus::Compatible | CompatibilityStatus::CompatibleWithWarning
            )
        {
            return Err(ProviderError::NoSuitableProvider);
        }
        Ok(view)
    }

    pub fn validate_image_generation(
        &self,
        provider_id: &str,
        snapshot: &HardwareSnapshot,
    ) -> Result<ProviderView, ProviderError> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| ProviderError::UnknownProvider(provider_id.to_string()))?;
        let view = provider_view(provider, snapshot);
        if view.manifest.provider_id != "local.a1111"
            || view.manifest.nature != ProviderNature::Real
            || view.manifest.execution_mode != ExecutionMode::LocalHttp
            || !view.manifest.enabled
            || !view
                .manifest
                .capabilities
                .contains(&Capability::TextToImage)
            || view.health.state != HealthState::Healthy
            || !matches!(
                view.fit.status,
                CompatibilityStatus::Compatible | CompatibilityStatus::CompatibleWithWarning
            )
        {
            return Err(ProviderError::NoSuitableProvider);
        }
        Ok(view)
    }

    pub fn list(
        &self,
        capability: Option<Capability>,
        snapshot: &HardwareSnapshot,
    ) -> Vec<ProviderView> {
        self.providers
            .values()
            .filter(|provider| {
                capability.is_none_or(|value| provider.manifest.capabilities.contains(&value))
            })
            .map(|provider| provider_view(provider, snapshot))
            .collect()
    }

    pub fn refresh_health(
        &mut self,
        provider_id: Option<&str>,
    ) -> Result<Vec<ProviderHealth>, ProviderError> {
        if let Some(id) = provider_id {
            let provider = self
                .providers
                .get_mut(id)
                .ok_or_else(|| ProviderError::UnknownProvider(id.to_string()))?;
            refresh_mock_health(provider);
            return Ok(vec![provider.health.clone()]);
        }
        for provider in self.providers.values_mut() {
            refresh_mock_health(provider);
        }
        Ok(self
            .providers
            .values()
            .map(|provider| provider.health.clone())
            .collect())
    }

    pub fn select(
        &self,
        capability: Capability,
        snapshot: &HardwareSnapshot,
    ) -> Result<ProviderView, ProviderError> {
        self.providers
            .values()
            .filter(|provider| {
                provider.manifest.enabled
                    && (provider.manifest.classification == Classification::Local
                        || (provider.manifest.classification == Classification::Remote
                            && provider.manifest.execution_mode == ExecutionMode::RemoteApi))
                    && provider.manifest.capabilities.contains(&capability)
                    && matches!(
                        provider.health.state,
                        HealthState::Healthy | HealthState::Degraded
                    )
            })
            .map(|provider| provider_view(provider, snapshot))
            .filter(|view| view.fit.status != CompatibilityStatus::Incompatible)
            .min_by_key(|view| {
                (
                    fit_rank(view.fit.status),
                    health_rank(view.health.state),
                    resource_rank(view.manifest.requirements.resource_class),
                    view.manifest.provider_id.clone(),
                )
            })
            .ok_or(ProviderError::NoSuitableProvider)
    }

    pub fn validate_selection(
        &self,
        provider_id: &str,
        capability: Capability,
        snapshot: &HardwareSnapshot,
    ) -> Result<ProviderView, ProviderError> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| ProviderError::UnknownProvider(provider_id.into()))?;
        let view = provider_view(provider, snapshot);
        if !view.manifest.enabled
            || (view.manifest.classification != Classification::Local
                && !(view.manifest.classification == Classification::Remote
                    && view.manifest.execution_mode == ExecutionMode::RemoteApi))
            || !view.manifest.capabilities.contains(&capability)
            || !matches!(
                view.health.state,
                HealthState::Healthy | HealthState::Degraded
            )
            || view.fit.status == CompatibilityStatus::Incompatible
        {
            return Err(ProviderError::NoSuitableProvider);
        }
        Ok(view)
    }

    pub fn validate_diagnostic_execution(
        &self,
        provider_id: &str,
        capability: Capability,
        snapshot: &HardwareSnapshot,
    ) -> Result<ProviderView, ProviderError> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| ProviderError::UnknownProvider(provider_id.to_string()))?;
        let view = provider_view(provider, snapshot);
        if !view.manifest.enabled
            || view.manifest.execution_mode != ExecutionMode::InternalMock
            || view.manifest.classification != Classification::Local
            || !view.manifest.capabilities.contains(&capability)
            || !matches!(
                view.health.state,
                HealthState::Healthy | HealthState::Degraded
            )
            || view.fit.status == CompatibilityStatus::Incompatible
        {
            return Err(ProviderError::NoSuitableProvider);
        }
        Ok(view)
    }
}

pub fn compatibility(
    requirements: &HardwareRequirements,
    snapshot: &HardwareSnapshot,
) -> Compatibility {
    let mut reasons = BTreeSet::new();
    let mut incompatible = false;
    let mut unknown = false;
    let mut warning = false;

    compare_capacity(
        snapshot.ram_total_mib,
        requirements.min_ram_mib,
        requirements.recommended_ram_mib,
        CompatibilityReasonCode::RamBelowMinimum,
        CompatibilityReasonCode::RamBelowRecommended,
        CompatibilityReasonCode::RamUnknown,
        &mut reasons,
        &mut incompatible,
        &mut unknown,
        &mut warning,
    );
    compare_capacity(
        snapshot.active_project_disk_free_mib,
        requirements.min_disk_mib,
        None,
        CompatibilityReasonCode::DiskBelowMinimum,
        CompatibilityReasonCode::DiskBelowMinimum,
        CompatibilityReasonCode::DiskUnknown,
        &mut reasons,
        &mut incompatible,
        &mut unknown,
        &mut warning,
    );

    let gpu_needed = requirements.gpu_required
        || requirements.min_vram_mib.is_some()
        || requirements.recommended_vram_mib.is_some()
        || !requirements.supported_gpu_vendors.is_empty();
    if gpu_needed && snapshot.gpus.is_empty() {
        if requirements.cpu_fallback {
            reasons.insert(CompatibilityReasonCode::CpuFallback);
            warning = true;
        } else if requirements.gpu_required {
            reasons.insert(CompatibilityReasonCode::GpuRequired);
            incompatible = true;
        } else {
            reasons.insert(CompatibilityReasonCode::GpuUnknown);
            unknown = true;
        }
    } else if gpu_needed {
        let allowed: BTreeSet<String> = requirements
            .supported_gpu_vendors
            .iter()
            .map(|vendor| vendor.to_ascii_lowercase())
            .collect();
        let candidates: Vec<_> = snapshot
            .gpus
            .iter()
            .filter(|gpu| allowed.is_empty() || allowed.contains(&gpu.vendor.to_ascii_lowercase()))
            .collect();
        if candidates.is_empty() {
            if snapshot.gpus.iter().all(|gpu| gpu.vendor == "unknown") {
                reasons.insert(CompatibilityReasonCode::GpuVendorUnknown);
                unknown = true;
            } else if requirements.cpu_fallback {
                reasons.insert(CompatibilityReasonCode::CpuFallback);
                warning = true;
            } else {
                reasons.insert(CompatibilityReasonCode::GpuVendorUnsupported);
                incompatible = true;
            }
        } else {
            let available_vram = candidates.iter().filter_map(|gpu| gpu.vram_total_mib).max();
            compare_capacity(
                available_vram,
                requirements.min_vram_mib,
                requirements.recommended_vram_mib,
                CompatibilityReasonCode::VramBelowMinimum,
                CompatibilityReasonCode::VramBelowRecommended,
                CompatibilityReasonCode::VramUnknown,
                &mut reasons,
                &mut incompatible,
                &mut unknown,
                &mut warning,
            );
        }
    }

    Compatibility {
        status: if incompatible {
            CompatibilityStatus::Incompatible
        } else if unknown {
            CompatibilityStatus::Unknown
        } else if warning {
            CompatibilityStatus::CompatibleWithWarning
        } else {
            CompatibilityStatus::Compatible
        },
        reason_codes: reasons.into_iter().collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_capacity(
    actual: Option<u64>,
    minimum: Option<u64>,
    recommended: Option<u64>,
    below_minimum: CompatibilityReasonCode,
    below_recommended: CompatibilityReasonCode,
    unknown_code: CompatibilityReasonCode,
    reasons: &mut BTreeSet<CompatibilityReasonCode>,
    incompatible: &mut bool,
    unknown: &mut bool,
    warning: &mut bool,
) {
    if minimum.is_none() && recommended.is_none() {
        return;
    }
    let Some(actual) = actual else {
        reasons.insert(unknown_code);
        *unknown = true;
        return;
    };
    if minimum.is_some_and(|value| actual < value) {
        reasons.insert(below_minimum);
        *incompatible = true;
    } else if recommended.is_some_and(|value| actual < value) {
        reasons.insert(below_recommended);
        *warning = true;
    }
}

fn provider_view(provider: &RegisteredProvider, snapshot: &HardwareSnapshot) -> ProviderView {
    ProviderView {
        manifest: provider.manifest.clone(),
        health: provider.health.clone(),
        license: provider.manifest.license.clone(),
        fit: compatibility(&provider.manifest.requirements, snapshot),
    }
}

fn refresh_mock_health(provider: &mut RegisteredProvider) {
    if provider.manifest.nature != ProviderNature::Mock {
        return;
    }
    provider.health.checked_at = Utc::now();
    provider.health.state = if provider.manifest.provider_id == "mock.unavailable" {
        HealthState::Unavailable
    } else {
        HealthState::Healthy
    };
    provider.health.detail = None;
}

fn validate_requirements(requirements: &HardwareRequirements) -> Result<(), ProviderError> {
    for (minimum, recommended, name) in [
        (
            requirements.min_ram_mib,
            requirements.recommended_ram_mib,
            "RAM",
        ),
        (
            requirements.min_vram_mib,
            requirements.recommended_vram_mib,
            "VRAM",
        ),
    ] {
        if minimum == Some(0) || recommended == Some(0) {
            return Err(ProviderError::InvalidRequirements(format!(
                "{name} values must be positive"
            )));
        }
        if let (Some(minimum), Some(recommended)) = (minimum, recommended)
            && recommended < minimum
        {
            return Err(ProviderError::InvalidRequirements(format!(
                "recommended {name} is below minimum"
            )));
        }
    }
    if requirements.min_disk_mib == Some(0) {
        return Err(ProviderError::InvalidRequirements(
            "disk value must be positive".to_string(),
        ));
    }
    let mut vendors = BTreeSet::new();
    for vendor in &requirements.supported_gpu_vendors {
        if !matches!(vendor.as_str(), "nvidia" | "amd" | "intel") || !vendors.insert(vendor) {
            return Err(ProviderError::InvalidRequirements(
                "GPU vendors must be known, lowercase, and unique".to_string(),
            ));
        }
    }
    if requirements.gpu_required && requirements.cpu_fallback {
        return Err(ProviderError::InvalidRequirements(
            "gpuRequired conflicts with cpuFallback".to_string(),
        ));
    }
    Ok(())
}

fn validate_permissions(manifest: &ProviderManifest) -> Result<(), ProviderError> {
    if manifest.execution_mode == ExecutionMode::InternalMock && !manifest.permissions.is_empty() {
        return Err(ProviderError::ExcessivePermissions);
    }
    if manifest.classification == Classification::Remote
        && !manifest.permissions.contains(&Permission::Network)
    {
        return Err(ProviderError::InvalidPermissions);
    }
    if manifest.classification == Classification::Local
        && manifest.execution_mode != ExecutionMode::LocalHttp
        && manifest.permissions.contains(&Permission::Network)
    {
        return Err(ProviderError::ExcessivePermissions);
    }
    if manifest.execution_mode != ExecutionMode::ExternalApplication
        && manifest
            .permissions
            .contains(&Permission::ExternalApplication)
    {
        return Err(ProviderError::ExcessivePermissions);
    }
    Ok(())
}

fn validate_license(license: &LicenseMetadata) -> Result<(), ProviderError> {
    match license.status {
        LicenseStatus::Unknown
            if license.name.is_some()
                || license.model_license.is_some()
                || license.commercial_use_allowed.is_some()
                || license.source_reference.is_some() =>
        {
            Err(ProviderError::InvalidLicense)
        }
        LicenseStatus::Known
            if license
                .name
                .as_ref()
                .is_none_or(|name| name.trim().is_empty()) =>
        {
            Err(ProviderError::InvalidLicense)
        }
        LicenseStatus::Known
            if license
                .source_reference
                .as_ref()
                .is_none_or(|reference| reference.trim().is_empty()) =>
        {
            Err(ProviderError::InvalidLicense)
        }
        _ => Ok(()),
    }
}

fn valid_provider_id(value: &str) -> bool {
    value.len() <= 128
        && value.split('.').count() >= 2
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !part.starts_with('-')
                && !part.ends_with('-')
        })
}

fn valid_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn has_duplicates<T: Ord + Copy>(values: &[T]) -> bool {
    let set: BTreeSet<T> = values.iter().copied().collect();
    set.len() != values.len()
}

fn fit_rank(status: CompatibilityStatus) -> u8 {
    match status {
        CompatibilityStatus::Compatible => 0,
        CompatibilityStatus::CompatibleWithWarning => 1,
        CompatibilityStatus::Unknown => 2,
        CompatibilityStatus::Incompatible => 3,
    }
}

fn health_rank(state: HealthState) -> u8 {
    match state {
        HealthState::Healthy => 0,
        HealthState::Degraded => 1,
        HealthState::Unknown => 2,
        HealthState::Misconfigured => 3,
        HealthState::Unavailable => 4,
    }
}

fn resource_rank(class: ResourceClass) -> u8 {
    match class {
        ResourceClass::Minimal => 0,
        ResourceClass::Standard => 1,
        ResourceClass::Heavy => 2,
    }
}

fn unknown_license() -> LicenseMetadata {
    LicenseMetadata {
        status: LicenseStatus::Unknown,
        name: None,
        model_license: None,
        commercial_use_allowed: None,
        source_reference: None,
    }
}

fn requirements(resource_class: ResourceClass) -> HardwareRequirements {
    HardwareRequirements {
        min_ram_mib: None,
        recommended_ram_mib: None,
        min_vram_mib: None,
        recommended_vram_mib: None,
        gpu_required: false,
        supported_gpu_vendors: Vec::new(),
        cpu_fallback: false,
        min_disk_mib: None,
        exclusive: false,
        resource_class,
    }
}

fn mock_manifest(id: &str, display_name: &str, capabilities: Vec<Capability>) -> ProviderManifest {
    ProviderManifest {
        schema_version: PROVIDER_MANIFEST_SCHEMA_VERSION,
        provider_id: id.to_string(),
        display_name: display_name.to_string(),
        version: "1.0.0".to_string(),
        provider_type: ProviderType::ImageGeneration,
        execution_mode: ExecutionMode::InternalMock,
        nature: ProviderNature::Mock,
        classification: Classification::Local,
        enabled: true,
        capabilities,
        health_check: HealthCheckType::Internal,
        requirements: requirements(ResourceClass::Minimal),
        license: unknown_license(),
        permissions: Vec::new(),
    }
}

fn local_a1111(enabled: bool) -> ProviderManifest {
    ProviderManifest {
        schema_version: PROVIDER_MANIFEST_SCHEMA_VERSION,
        provider_id: "local.a1111".into(),
        display_name: "Local Automatic1111".into(),
        version: "1.0.0".into(),
        provider_type: ProviderType::ImageGeneration,
        execution_mode: ExecutionMode::LocalHttp,
        nature: ProviderNature::Real,
        classification: Classification::Local,
        enabled,
        capabilities: vec![Capability::TextToImage],
        health_check: HealthCheckType::LocalHttp,
        requirements: requirements(ResourceClass::Standard),
        license: unknown_license(),
        permissions: vec![
            Permission::JobWorkingDirectoryWrite,
            Permission::OutputStagingWrite,
            Permission::Network,
        ],
    }
}

fn local_comfyui(enabled: bool) -> ProviderManifest {
    ProviderManifest {
        schema_version: PROVIDER_MANIFEST_SCHEMA_VERSION,
        provider_id: "local.comfyui".into(),
        display_name: "Local ComfyUI Video".into(),
        version: "1.0.0".into(),
        provider_type: ProviderType::VideoGeneration,
        execution_mode: ExecutionMode::LocalHttp,
        nature: ProviderNature::Real,
        classification: Classification::Local,
        enabled,
        capabilities: vec![Capability::TextToVideo],
        health_check: HealthCheckType::LocalHttp,
        requirements: requirements(ResourceClass::Heavy),
        license: unknown_license(),
        permissions: vec![
            Permission::InputAssetsRead,
            Permission::JobWorkingDirectoryWrite,
            Permission::OutputStagingWrite,
            Permission::Network,
        ],
    }
}

fn local_hunyuan(enabled: bool) -> ProviderManifest {
    ProviderManifest {
        schema_version: PROVIDER_MANIFEST_SCHEMA_VERSION,
        provider_id: "local.hunyuan".into(),
        display_name: "Local Hunyuan3D".into(),
        version: "1.0.0".into(),
        provider_type: ProviderType::ThreeDProcessing,
        execution_mode: ExecutionMode::LocalHttp,
        nature: ProviderNature::Real,
        classification: Classification::Local,
        enabled,
        capabilities: vec![
            Capability::Model3dTextTo3d,
            Capability::Model3dImageTo3d,
            Capability::HunyuanTextTo3d,
            Capability::HunyuanImageTo3d,
        ],
        health_check: HealthCheckType::LocalHttp,
        requirements: requirements(ResourceClass::Heavy),
        license: unknown_license(),
        permissions: vec![
            Permission::InputAssetsRead,
            Permission::JobWorkingDirectoryWrite,
            Permission::OutputStagingWrite,
            Permission::Network,
        ],
    }
}

fn local_blender() -> ProviderManifest {
    ProviderManifest {
        schema_version: PROVIDER_MANIFEST_SCHEMA_VERSION,
        provider_id: "local.blender".into(),
        display_name: "Local Blender".into(),
        version: "1.0.0".into(),
        provider_type: ProviderType::ThreeDProcessing,
        execution_mode: ExecutionMode::ControlledCli,
        nature: ProviderNature::Real,
        classification: Classification::Local,
        enabled: true,
        capabilities: vec![Capability::MeshProcessing],
        health_check: HealthCheckType::Process,
        requirements: requirements(ResourceClass::Standard),
        license: unknown_license(),
        permissions: vec![
            Permission::InputAssetsRead,
            Permission::JobWorkingDirectoryRead,
            Permission::JobWorkingDirectoryWrite,
            Permission::OutputStagingWrite,
        ],
    }
}

fn mock_basic() -> ProviderManifest {
    mock_manifest(
        "mock.image.basic",
        "Basic Image Mock",
        vec![Capability::TextToImage],
    )
}

fn mock_gpu_heavy() -> ProviderManifest {
    let mut manifest = mock_manifest(
        "mock.image.gpu-heavy",
        "GPU-heavy Image Mock",
        vec![Capability::TextToImage, Capability::ImageToImage],
    );
    manifest.requirements = HardwareRequirements {
        min_ram_mib: Some(16_384),
        recommended_ram_mib: Some(32_768),
        min_vram_mib: Some(24_576),
        recommended_vram_mib: Some(49_152),
        gpu_required: true,
        supported_gpu_vendors: vec!["nvidia".to_string(), "amd".to_string()],
        cpu_fallback: false,
        min_disk_mib: Some(20_480),
        exclusive: true,
        resource_class: ResourceClass::Heavy,
    };
    manifest
}

fn mock_unavailable() -> ProviderManifest {
    mock_manifest(
        "mock.unavailable",
        "Unavailable Mock",
        vec![Capability::TextToImage],
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hardware::{Confidence, GpuSnapshot, HARDWARE_SCHEMA_VERSION, HardwareSource};

    pub(crate) fn snapshot(
        ram: Option<u64>,
        disk: Option<u64>,
        gpu: Option<(&str, Option<u64>)>,
    ) -> HardwareSnapshot {
        HardwareSnapshot {
            schema_version: HARDWARE_SCHEMA_VERSION,
            os: "windows".into(),
            arch: "x86_64".into(),
            cpu_model: "fixture".into(),
            cpu_logical_count: 8,
            cpu_physical_count: Some(4),
            ram_total_mib: ram,
            gpus: gpu
                .map(|(vendor, vram)| GpuSnapshot {
                    name: "fixture gpu".into(),
                    vendor: vendor.into(),
                    vram_total_mib: vram,
                    vram_used_mib: None,
                    vram_free_mib: vram,
                    driver: None,
                    source: "fixture".into(),
                    confidence: Confidence::High,
                })
                .into_iter()
                .collect(),
            active_project_disk_free_mib: disk,
            captured_at: Utc::now(),
            sources: vec![HardwareSource {
                name: "fixture".into(),
                confidence: Confidence::High,
            }],
            confidence: Confidence::High,
        }
    }

    fn valid_json() -> String {
        serde_json::to_string(&mock_basic()).unwrap()
    }

    #[test]
    fn valid_manifest_and_provider_id() {
        assert_eq!(
            ProviderManifest::from_json(&valid_json()).unwrap(),
            mock_basic()
        );
        assert!(!valid_provider_id("UPPER.bad"));
        assert!(!valid_provider_id("onepart"));
        assert!(!valid_provider_id("bad..id"));
    }

    #[test]
    fn all_phase5_capabilities_have_stable_identifiers() {
        let capabilities = [
            Capability::TextToImage,
            Capability::ImageToImage,
            Capability::Inpainting,
            Capability::BackgroundRemoval,
            Capability::Upscaling,
            Capability::TextureGeneration,
            Capability::TextToVideo,
            Capability::ImageToVideo,
            Capability::VideoToVideo,
            Capability::ThreeDGeneration,
            Capability::MeshProcessing,
            Capability::MaterialGeneration,
            Capability::PromptGeneration,
        ];
        assert_eq!(
            serde_json::to_value(capabilities).unwrap(),
            serde_json::json!([
                "text_to_image",
                "image_to_image",
                "inpainting",
                "background_removal",
                "upscaling",
                "texture_generation",
                "text_to_video",
                "image_to_video",
                "video_to_video",
                "three_d_generation",
                "mesh_processing",
                "material_generation",
                "prompt_generation"
            ])
        );
    }

    #[test]
    fn malformed_unknown_schema_and_unknown_field_rejected() {
        assert!(matches!(
            ProviderManifest::from_json("{"),
            Err(ProviderError::MalformedManifest(_))
        ));
        let mut value: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
        value["schemaVersion"] = 2.into();
        assert_eq!(
            ProviderManifest::from_json(&value.to_string()).unwrap_err(),
            ProviderError::UnknownSchema(2)
        );
        value["schemaVersion"] = 1.into();
        value["command"] = "evil.exe".into();
        assert!(matches!(
            ProviderManifest::from_json(&value.to_string()),
            Err(ProviderError::MalformedManifest(_))
        ));
    }

    #[test]
    fn invalid_capability_permission_and_requirements_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
        value["capabilities"] = serde_json::json!(["arbitrary_capability"]);
        assert!(ProviderManifest::from_json(&value.to_string()).is_err());
        value = serde_json::from_str(&valid_json()).unwrap();
        value["permissions"] = serde_json::json!(["filesystemAll"]);
        assert!(ProviderManifest::from_json(&value.to_string()).is_err());
        let mut manifest = mock_basic();
        manifest.requirements.min_ram_mib = Some(10);
        manifest.requirements.recommended_ram_mib = Some(5);
        assert!(matches!(
            manifest.validate(),
            Err(ProviderError::InvalidRequirements(_))
        ));
    }

    #[test]
    fn unknown_execution_classification_and_health_check_rejected() {
        for (field, unknown) in [
            ("executionMode", "arbitraryProcess"),
            ("classification", "hybrid"),
            ("healthCheck", "customCommand"),
        ] {
            let mut value: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
            value[field] = unknown.into();
            assert!(matches!(
                ProviderManifest::from_json(&value.to_string()),
                Err(ProviderError::MalformedManifest(_))
            ));
        }
    }

    #[test]
    fn duplicate_capabilities_and_permissions_rejected() {
        let mut manifest = mock_basic();
        manifest.capabilities.push(Capability::TextToImage);
        assert_eq!(manifest.validate(), Err(ProviderError::InvalidCapabilities));
        manifest = mock_basic();
        manifest.permissions = vec![Permission::Gpu, Permission::Gpu];
        assert_eq!(manifest.validate(), Err(ProviderError::InvalidPermissions));
    }

    #[test]
    fn only_internal_mock_can_be_enabled() {
        let mut manifest = mock_basic();
        manifest.execution_mode = ExecutionMode::RemoteApi;
        manifest.classification = Classification::Remote;
        manifest.health_check = HealthCheckType::LocalHttp;
        manifest.permissions = vec![Permission::Network];
        assert_eq!(
            manifest.validate(),
            Err(ProviderError::UnsupportedEnabledExecutionMode)
        );
        manifest.enabled = false;
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn registry_register_list_filter_and_duplicate() {
        let mut registry = ProviderRegistry::phase5().unwrap();
        let hardware = snapshot(Some(64_000), Some(64_000), Some(("nvidia", Some(64_000))));
        assert_eq!(registry.list(None, &hardware).len(), 3);
        assert_eq!(
            registry
                .list(Some(Capability::ImageToImage), &hardware)
                .len(),
            1
        );
        assert!(matches!(
            registry.register(mock_basic(), HealthState::Healthy),
            Err(ProviderError::DuplicateProvider(_))
        ));
    }

    #[test]
    fn health_refresh_preserves_mock_states() {
        let mut registry = ProviderRegistry::phase5().unwrap();
        registry.refresh_health(None).unwrap();
        let hardware = snapshot(None, None, None);
        let views = registry.list(None, &hardware);
        assert_eq!(
            views
                .iter()
                .find(|v| v.manifest.provider_id == "mock.unavailable")
                .unwrap()
                .health
                .state,
            HealthState::Unavailable
        );
        assert_eq!(
            views
                .iter()
                .find(|v| v.manifest.provider_id == "mock.image.basic")
                .unwrap()
                .health
                .state,
            HealthState::Healthy
        );
        assert!(matches!(
            registry.refresh_health(Some("missing")),
            Err(ProviderError::UnknownProvider(_))
        ));
    }

    #[test]
    fn compatibility_covers_ram_vram_gpu_vendor_disk_and_unknown() {
        let requirements = mock_gpu_heavy().requirements;
        let fit = compatibility(&requirements, &snapshot(Some(8_000), Some(10_000), None));
        assert_eq!(fit.status, CompatibilityStatus::Incompatible);
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::RamBelowMinimum)
        );
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::DiskBelowMinimum)
        );
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::GpuRequired)
        );

        let fit = compatibility(
            &requirements,
            &snapshot(Some(64_000), Some(64_000), Some(("intel", Some(64_000)))),
        );
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::GpuVendorUnsupported)
        );
        let fit = compatibility(&requirements, &snapshot(None, None, Some(("nvidia", None))));
        assert_eq!(fit.status, CompatibilityStatus::Unknown);
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::RamUnknown)
        );
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::VramUnknown)
        );
        assert!(
            fit.reason_codes
                .contains(&CompatibilityReasonCode::DiskUnknown)
        );
    }

    #[test]
    fn compatibility_warning_and_compatible_cases() {
        let requirements = mock_gpu_heavy().requirements;
        let warning = compatibility(
            &requirements,
            &snapshot(Some(20_000), Some(30_000), Some(("nvidia", Some(30_000)))),
        );
        assert_eq!(warning.status, CompatibilityStatus::CompatibleWithWarning);
        assert!(
            warning
                .reason_codes
                .contains(&CompatibilityReasonCode::RamBelowRecommended)
        );
        assert!(
            warning
                .reason_codes
                .contains(&CompatibilityReasonCode::VramBelowRecommended)
        );
        let compatible = compatibility(
            &requirements,
            &snapshot(Some(64_000), Some(64_000), Some(("nvidia", Some(64_000)))),
        );
        assert_eq!(compatible.status, CompatibilityStatus::Compatible);
    }

    #[test]
    fn security_shape_has_no_command_path_or_secret_and_known_permissions_only() {
        let value = serde_json::to_value(mock_basic()).unwrap();
        let text = value.to_string().to_ascii_lowercase();
        assert!(!text.contains("command"));
        assert!(!text.contains("executable"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("path"));
        assert!(value["permissions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn deterministic_selection_excludes_unhealthy_incompatible_and_remote() {
        let mut registry = ProviderRegistry::phase5().unwrap();
        let mut remote = mock_basic();
        remote.provider_id = "remote.image.api".into();
        remote.execution_mode = ExecutionMode::RemoteApi;
        remote.classification = Classification::Remote;
        remote.health_check = HealthCheckType::LocalHttp;
        remote.enabled = false;
        remote.permissions = vec![Permission::Network];
        registry.register(remote, HealthState::Healthy).unwrap();
        let low = snapshot(Some(8_000), Some(1_000), None);
        assert_eq!(
            registry
                .select(Capability::TextToImage, &low)
                .unwrap()
                .manifest
                .provider_id,
            "mock.image.basic"
        );
        assert!(matches!(
            registry.select(Capability::ImageToImage, &low),
            Err(ProviderError::NoSuitableProvider)
        ));
    }

    #[test]
    fn phase8_registers_selectable_blender_mesh_processing() {
        // Regression: the Blender stage's creation gate resolves its provider
        // through the registry. phase8 (the production constructor) must
        // expose local.blender as a selectable MeshProcessing provider or
        // every Blender optimization fails with "no compatible 3D processing
        // provider is configured".
        let registry = ProviderRegistry::phase8(true, true, false).unwrap();
        let selected = registry
            .select(Capability::MeshProcessing, &snapshot(Some(16_000), Some(50_000), None))
            .expect("local.blender must be selectable for MeshProcessing");
        assert_eq!(selected.manifest.provider_id, "local.blender");
        assert_eq!(selected.fit.status, CompatibilityStatus::Compatible);
    }

    #[test]
    fn protocol_is_future_descriptor_only() {
        let descriptor = protocol_descriptor();
        assert_eq!(descriptor.protocol, "JSON-RPC 2.0");
        assert_eq!(descriptor.transport, "stdio");
        assert_eq!(descriptor.protocol_version, 1);
    }
}

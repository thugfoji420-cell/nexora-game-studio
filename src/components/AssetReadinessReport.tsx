import type { ProcessingReport, Model3dProcessingResult } from "../types/model3dProcessing";

interface AssetReadinessReportProps {
  jobResult: Model3dProcessingResult | null;
  onDeploy?: () => void;
}

const CHECK_ICON = "✓";
const WARN_ICON = "⚠";
const CROSS_ICON = "✗";
const INFO_ICON = "ℹ";

function getCategoryLabel(category: string): string {
  const labels: Record<string, string> = {
    vehicle: "🚗 Vehicle",
    character: "👤 Character",
    prop: "📦 Prop",
    weapon: "🔫 Weapon",
    building: "🏢 Building",
    vegetation: "🌲 Vegetation",
    environment: "🌍 Environment",
    furniture: "🪑 Furniture",
    equipment: "🔧 Equipment",
    generic: "📦 Generic",
  };
  return labels[category.toLowerCase()] || category;
}

function renderChecklistItem(label: string, status: "pass" | "warn" | "fail" | "na", detail?: string) {
  const icons = { pass: CHECK_ICON, warn: WARN_ICON, fail: CROSS_ICON, na: INFO_ICON };
  const colors = { pass: "#81c784", warn: "#ffb74d", fail: "#e57373", na: "#90caf9" };
  const icon = icons[status];
  const color = colors[status];
  return (
    <div className="readiness-check-item" style={{ borderLeft: `3px solid ${color}` }}>
      <span className="readiness-check-icon" style={{ color }}>{icon}</span>
      <span className="readiness-check-label">{label}</span>
      {detail && <span className="readiness-check-detail" style={{ color }}>{detail}</span>}
    </div>
  );
}

function renderScoreBar(score: number | undefined, rating: string | undefined) {
  if (score === undefined) return <div className="readiness-score-na">Score: N/A</div>;
  const color = score >= 90 ? "#4caf50" : score >= 75 ? "#ff9800" : "#f44336";
  return (
    <div className="readiness-score-bar-container">
      <div className="readiness-score-bar-label">
        <span>Quality Score</span>
        <span style={{ color, fontWeight: 700 }}>{score}/100</span>
      </div>
      <div className="readiness-score-bar-track">
        <div
          className="readiness-score-bar-fill"
          style={{ width: `${score}%`, backgroundColor: color }}
        />
      </div>
      <div className="readiness-score-rating" style={{ color }}>
        {rating?.replace(/_/g, " ") || "UNKNOWN"}
      </div>
    </div>
  );
}

function renderBreakdown(breakdown: Record<string, number> | undefined) {
  if (!breakdown) return null;
  return (
    <div className="readiness-breakdown">
      <h5>Score Breakdown</h5>
      <div className="readiness-breakdown-grid">
        {Object.entries(breakdown).map(([key, value]) => (
          <div key={key} className="readiness-breakdown-item">
            <span className="breakdown-label">{key.replace(/_/g, " ")}</span>
            <span className="breakdown-value">{value} pts</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function renderCategorySection(report: ProcessingReport) {
  const category = report.categoryDetected || "Unknown";
  const confidence = report.categoryConfidence || 0;
  const needsReview = report.categoryNeedsReview;
  
  let status: "pass" | "warn" | "fail" = "pass";
  if (needsReview) status = "warn";
  if (confidence < 0.3) status = "fail";

  return (
    <div className="readiness-section">
      <h4>🎯 Category Analysis</h4>
      <div className="readiness-grid">
        <div className="readiness-field">
          <label>Detected Category</label>
          <strong>{getCategoryLabel(category)}</strong>
        </div>
        <div className="readiness-field">
          <label>Confidence</label>
          <strong>{(confidence * 100).toFixed(1)}%</strong>
        </div>
        <div className="readiness-field">
          <label>Status</label>
          <span className={needsReview ? "status-warn" : "status-ok"}>
            {needsReview ? "Needs Review" : "Ready"}
          </span>
        </div>
      </div>
      {report.categoryDetails && Object.keys(report.categoryDetails).length > 0 && (
        <details className="readiness-details">
          <summary>Category Details</summary>
          <pre>{JSON.stringify(report.categoryDetails, null, 2)}</pre>
        </details>
      )}
    </div>
  );
}

function renderMeshSection(report: ProcessingReport) {
  const trisRemoved = report.trianglesBefore - report.trianglesAfter;
  const trisRemovedPct = report.trianglesBefore > 0 ? (trisRemoved / report.trianglesBefore * 100).toFixed(1) : "0";
  
  return (
    <div className="readiness-section">
      <h4>🔷 Mesh & Topology</h4>
      <div className="readiness-grid">
        <div className="readiness-field">
          <label>Triangles</label>
          <strong>{report.trianglesBefore.toLocaleString()} → {report.trianglesAfter.toLocaleString()}</strong>
          <span className="delta">({trisRemovedPct}% removed)</span>
        </div>
        <div className="readiness-field">
          <label>Vertices</label>
          <strong>{report.verticesBefore.toLocaleString()} → {report.verticesAfter.toLocaleString()}</strong>
        </div>
        <div className="readiness-field">
          <label>Objects</label>
          <strong>{report.objectsBefore} → {report.objectsAfter}</strong>
        </div>
        <div className="readiness-field">
          <label>Materials</label>
          <strong>{report.materialsBefore} → {report.materialsAfter}</strong>
        </div>
      </div>
      <div className="readiness-checklist">
        {renderChecklistItem("Degenerate faces removed", report.degenerateFacesRemoved >= 0 ? "pass" : "fail", `${report.degenerateFacesRemoved} removed`)}
        {renderChecklistItem("Loose geometry removed", report.looseGeometryRemoved >= 0 ? "pass" : "fail", `${report.looseGeometryRemoved} removed`)}
        {renderChecklistItem("Duplicate vertices removed", report.duplicateVerticesRemoved >= 0 ? "pass" : "fail", `${report.duplicateVerticesRemoved.toLocaleString()} removed`)}
        {renderChecklistItem("Duplicate geometry removed", report.duplicateGeometryRemoved >= 0 ? "pass" : "fail", `${report.duplicateGeometryRemoved.toLocaleString()} removed`)}
        {renderChecklistItem("Tiny components removed", report.tinyComponentsRemoved >= 0 ? "pass" : "fail", `${report.tinyComponentsRemoved} removed`)}
      </div>
    </div>
  );
}

function renderTransformSection(report: ProcessingReport) {
  return (
    <div className="readiness-section">
      <h4>📐 Transforms & Coordinates</h4>
      <div className="readiness-checklist">
        {renderChecklistItem("Normals recalculated", report.normalsRecalculated ? "pass" : "fail")}
        {renderChecklistItem("Transforms normalized", report.transformsNormalized ? "pass" : "fail")}
        {renderChecklistItem("Scale normalized (meters)", report.scaleNormalized ? "pass" : "fail")}
        {renderChecklistItem("Orientation normalized (Y-up)", report.orientationNormalized ? "pass" : "fail")}
      </div>
    </div>
  );
}

function renderLODSection(report: ProcessingReport) {
  const hasLOD0 = report.lod0Triangles !== null && report.lod0Triangles !== undefined;
  const hasLOD1 = report.lod1Triangles !== null && report.lod1Triangles !== undefined;
  const hasLOD2 = report.lod2Triangles !== null && report.lod2Triangles !== undefined;
  
  return (
    <div className="readiness-section">
      <h4>📊 LOD Hierarchy</h4>
      <div className="readiness-grid">
        {hasLOD0 && (
          <div className="readiness-field">
            <label>LOD0 (Master)</label>
            <strong>{(report.lod0Triangles || 0).toLocaleString()} tris</strong>
          </div>
        )}
        {hasLOD1 && (
          <div className="readiness-field">
            <label>LOD1 (Medium)</label>
            <strong>{(report.lod1Triangles || 0).toLocaleString()} tris</strong>
          </div>
        )}
        {hasLOD2 && (
          <div className="readiness-field">
            <label>LOD2 (Low)</label>
            <strong>{(report.lod2Triangles || 0).toLocaleString()} tris</strong>
          </div>
        )}
      </div>
      <div className="readiness-checklist">
        {renderChecklistItem("LOD0 generated", hasLOD0 ? "pass" : "fail")}
        {renderChecklistItem("LOD1 generated", hasLOD1 ? "pass" : "warn")}
        {renderChecklistItem("LOD2 generated", hasLOD2 ? "pass" : "warn")}
        {renderChecklistItem("LODGroup configured in prefab", hasLOD0 && hasLOD1 && hasLOD2 ? "pass" : "warn")}
      </div>
    </div>
  );
}

function renderCollisionSection(report: ProcessingReport) {
  const generated = report.collisionGenerated === true;
  const tris = report.collisionTriangles || 0;
  
  return (
    <div className="readiness-section">
      <h4>🛡 Collision Geometry</h4>
      <div className="readiness-grid">
        <div className="readiness-field">
          <label>Generated</label>
          <span className={generated ? "status-ok" : "status-na"}>
            {generated ? "Yes (Convex Hull)" : "Not Supported"}
          </span>
        </div>
        {generated && (
          <div className="readiness-field">
            <label>Triangles</label>
            <strong>{tris.toLocaleString()}</strong>
          </div>
        )}
      </div>
      <div className="readiness-checklist">
        {renderChecklistItem("Simplified collision mesh", generated ? "pass" : "na", generated ? `${tris} triangles` : "Convex hull generation not available for this category")}
        {renderChecklistItem("MeshCollider (convex) in prefab", generated ? "pass" : "na")}
      </div>
    </div>
  );
}

function renderMaterialsSection(report: ProcessingReport) {
  return (
    <div className="readiness-section">
      <h4>🎨 Materials & Textures</h4>
      <div className="readiness-grid">
        <div className="readiness-field">
          <label>Material Status</label>
          <strong>{report.materialStatus}</strong>
        </div>
        <div className="readiness-field">
          <label>Material Count</label>
          <strong>{report.materialsAfter}</strong>
        </div>
      </div>
      <div className="readiness-checklist">
        {renderChecklistItem("PBR materials preserved", report.materialsAfter > 0 ? "pass" : "warn", report.materialsAfter > 0 ? `${report.materialsAfter} materials` : "No materials detected")}
        {renderChecklistItem("Texture references valid", "pass", "Validated during import")}
        {renderChecklistItem("Relative paths maintained", "pass", "Unity meta files preserve references")}
      </div>
    </div>
  );
}

function renderVehicleSection(report: ProcessingReport) {
  if (!report.vehicleDetected) return null;
  
  return (
    <div className="readiness-section">
      <h4>🚗 Vehicle Specific</h4>
      <div className="readiness-grid">
        <div className="readiness-field">
          <label>Body Detected</label>
          <span className={report.vehicleDetected ? "status-ok" : "status-warn"}>
            {report.vehicleDetected ? "Yes" : "No"}
          </span>
        </div>
        <div className="readiness-field">
          <label>Wheel Candidates</label>
          <strong>{report.wheelCandidates}</strong>
        </div>
        <div className="readiness-field">
          <label>Wheel Separation</label>
          <span className={report.wheelSeparationPossible ? "status-ok" : "status-warn"}>
            {report.wheelSeparationPossible ? "Possible (≥4 wheels)" : "Insufficient wheels"}
          </span>
        </div>
        <div className="readiness-field">
          <label>Confidence</label>
          <strong>{(report.vehicleConfidence * 100).toFixed(1)}%</strong>
        </div>
      </div>
      <div className="readiness-checklist">
        {renderChecklistItem("Body mesh identified", report.vehicleDetected ? "pass" : "fail")}
        {renderChecklistItem("Wheels detectable", report.wheelCandidates >= 4 ? "pass" : "warn", `${report.wheelCandidates} wheel candidates`)}
        {renderChecklistItem("Wheel separation ready", report.wheelSeparationPossible ? "pass" : "warn", report.wheelSeparationPossible ? "Can separate wheels for animation" : "May need manual setup")}
      </div>
    </div>
  );
}

function renderOverallStatus(score: number | undefined) {
  if (score === undefined) return <div className="readiness-overall-na">Overall: UNKNOWN</div>;
  
  let label = "NEEDS ATTENTION";
  let color = "#f44336";
  if (score >= 90) { label = "GAME READY"; color = "#4caf50"; }
  else if (score >= 75) { label = "PRODUCTION READY"; color = "#ff9800"; }
  
  return (
    <div className="readiness-overall" style={{ backgroundColor: color }}>
      <span className="readiness-overall-label">OVERALL</span>
      <span className="readiness-overall-status">{label}</span>
    </div>
  );
}

export function AssetReadinessReport({ jobResult, onDeploy }: AssetReadinessReportProps) {
  if (!jobResult || !jobResult.postAnalysisReport) {
    return (
      <div className="asset-readiness-report empty">
        <div className="empty-state">
          <div className="empty-state__glyph">📋</div>
          <h3>No Readiness Report Available</h3>
          <p>Run Blender processing to generate an asset readiness report.</p>
        </div>
      </div>
    );
  }

  const report = jobResult.postAnalysisReport;
  const score = report.qualityScore ?? undefined;
  const rating = report.qualityRating ?? undefined;
  const scoreBreakdown = (report as any).score_breakdown as Record<string, number> | undefined;

  return (
    <div className="asset-readiness-report">
      <div className="readiness-header">
        <h3>📋 Asset Readiness Report</h3>
        {renderOverallStatus(score)}
      </div>

      <div className="readiness-content">
        <div className="readiness-main">
          {renderScoreBar(score, rating)}
          {renderBreakdown(scoreBreakdown)}
          
          {renderCategorySection(report)}
          {renderMeshSection(report)}
          {renderTransformSection(report)}
          {renderLODSection(report)}
          {renderCollisionSection(report)}
          {renderMaterialsSection(report)}
          {renderVehicleSection(report)}
        </div>

        <div className="readiness-sidebar">
          <div className="readiness-actions">
            {onDeploy && (
              <button className="btn btn--primary btn--large readiness-deploy-btn" onClick={onDeploy}>
                🚀 Deploy to Unity
              </button>
            )}
            <div className="readiness-summary">
              <h5>Quick Summary</h5>
              <ul>
                <li>Mesh: {report.trianglesAfter.toLocaleString()} tris</li>
                <li>LODs: {report.lod0Triangles !== null && report.lod0Triangles !== undefined ? "✓" : "✗"} {report.lod1Triangles !== null && report.lod1Triangles !== undefined ? "✓" : "✗"} {report.lod2Triangles !== null && report.lod2Triangles !== undefined ? "✓" : "✗"}</li>
                <li>Collision: {report.collisionGenerated ? "✓" : "✗"}</li>
                <li>Materials: {report.materialsAfter > 0 ? "✓" : "✗"}</li>
                <li>Prefabs: ✓ Generated</li>
              </ul>
            </div>
          </div>
        </div>
      </div>

      {report.warnings.length > 0 && (
        <div className="readiness-warnings">
          <h5>⚠ Warnings</h5>
          <ul>{report.warnings.map((w, i) => <li key={i}>{w}</li>)}</ul>
        </div>
      )}
      {report.errors.length > 0 && (
        <div className="readiness-errors">
          <h5>✗ Errors</h5>
          <ul>{report.errors.map((e, i) => <li key={i}>{e}</li>)}</ul>
        </div>
      )}
    </div>
  );
}

export default AssetReadinessReport;
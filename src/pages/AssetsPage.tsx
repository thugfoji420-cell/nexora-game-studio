import { useEffect, useState, useCallback, useRef } from "react";
import { importAsset, listAssets, searchAssets, pickAssetFile, isDuplicateError, formatFileSize, formatTimestamp } from "../services/assets";
import { classifyAssetPreview, getImagePreviewSource } from "../services/assetPreview";
import { formatModel3dAssetFormat, formatModel3dBounds, formatModel3dCount, getSupportedModel3dMetadata } from "../services/assetModel3d";
import { AssetVideoPreview } from "../components/AssetVideoPreview";
import type { AssetInfo } from "../types/core";

export function AssetsPage() {
  const [assets, setAssets] = useState<AssetInfo[]>([]);
  const [filteredAssets, setFilteredAssets] = useState<AssetInfo[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedAssetId, setSelectedAssetId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [importing, setImporting] = useState(false);
  const [previewData, setPreviewData] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const projectOpen = useRef(true);
  const previewRequestVersion = useRef(0);
  const searchRequestVersion = useRef(0);

  const loadAssets = useCallback(async () => {
    try {
      setLoading(true);
      const data = await listAssets();
      setAssets(data);
      setFilteredAssets(data);
      setError(null);
      projectOpen.current = true;
    } catch (err) {
      if (err instanceof Error && err.message.includes("no project open")) {
        projectOpen.current = false;
        setError("No project open. Please create or open a project first.");
        setAssets([]);
        setFilteredAssets([]);
      } else {
        setError(err instanceof Error ? err.message : "Failed to load assets");
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadAssets();
  }, [loadAssets]);

  useEffect(() => {
    const requestVersion = ++searchRequestVersion.current;
    const query = searchQuery.trim();
    let active = true;
    if (!query) {
      setFilteredAssets(assets);
      return () => { active = false; };
    }
    searchAssets(query).then((results) => {
      if (active && searchRequestVersion.current === requestVersion) setFilteredAssets(results);
    }).catch(() => {
      const filtered = assets.filter((a) =>
        a.originalFilename.toLowerCase().includes(query.toLowerCase())
      );
      if (active && searchRequestVersion.current === requestVersion) setFilteredAssets(filtered);
    });
    return () => { active = false; };
  }, [searchQuery, assets]);

  const handleImport = async () => {
    setError(null);
    try {
      const path = await pickAssetFile();
      if (!path) return;
      
      setImporting(true);
      const asset = await importAsset(path);
      const nextAssets = [asset, ...assets.filter((item) => item.assetId !== asset.assetId)];
      const query = searchQuery.trim();
      ++searchRequestVersion.current;
      setAssets(nextAssets);
      if (!query) {
        setFilteredAssets(nextAssets);
      } else {
        setFilteredAssets(nextAssets.filter((item) => item.originalFilename.toLowerCase().includes(query.toLowerCase())));
      }
    } catch (err) {
      if (isDuplicateError(err)) {
        setError("Duplicate asset detected. This content has already been imported.");
      } else {
        setError(err instanceof Error ? err.message : "Import failed");
      }
    } finally {
      setImporting(false);
    }
  };

  const handleAssetSelect = (asset: AssetInfo) => {
    setSelectedAssetId(asset.assetId);
  };

  const selectedAsset = assets.find((a) => a.assetId === selectedAssetId);
  const selectedPreviewKind = selectedAsset ? classifyAssetPreview(selectedAsset) : "unsupported";
  const selectedModelMetadata = selectedAsset?.mediaKind === "model3d" ? getSupportedModel3dMetadata(selectedAsset) : null;

  useEffect(() => {
    const requestVersion = ++previewRequestVersion.current;
    if (!selectedAsset || selectedPreviewKind !== "image") {
      setPreviewData(null);
      setPreviewLoading(false);
      return;
    }

    setPreviewLoading(true);
    setPreviewData(null);
    getImagePreviewSource(selectedAsset)
      .then((data) => {
        if (previewRequestVersion.current !== requestVersion) return;
        setPreviewData(data);
        setPreviewLoading(false);
      })
      .catch(() => {
        if (previewRequestVersion.current !== requestVersion) return;
        setPreviewData(null);
        setPreviewLoading(false);
      });

    return () => {
      if (previewRequestVersion.current === requestVersion) previewRequestVersion.current++;
    };
  }, [selectedAsset, selectedPreviewKind]);

  if (!projectOpen.current) {
    return (
      <section className="assets-page">
        <header className="assets-header">
          <h2>Asset Library</h2>
          <button className="btn btn--primary" onClick={handleImport} disabled={importing || !projectOpen.current}>
            {importing ? "Importing..." : "Import Asset"}
          </button>
        </header>
        <div className="empty-state">
          <div className="empty-state__glyph">NX</div>
          <h2>No Project Open</h2>
          <p>Create or open a Nexora project to access the Asset Library.</p>
        </div>
      </section>
    );
  }

  return (
    <section className="assets-page">
      <header className="assets-header">
        <h2>Asset Library</h2>
        <div className="assets-header__actions">
          <input
            type="text"
            className="search-input"
            placeholder="Search assets..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            aria-label="Search assets"
          />
          <button className="btn btn--primary" onClick={handleImport} disabled={importing}>
            {importing ? "Importing..." : "Import Asset"}
          </button>
        </div>
      </header>

      {error && <div className="error-banner" role="alert">{error}</div>}

      <div className="assets-layout">
        <aside className="assets-list" aria-label="Asset list">
          {loading ? (
            <div className="loading-placeholder">Loading assets...</div>
          ) : filteredAssets.length === 0 ? (
            <div className="empty-state empty-state--inline">
              <div className="empty-state__glyph">IMG</div>
              <p>{searchQuery ? "No assets match your search." : "No assets imported yet."}</p>
              {searchQuery && <button className="btn btn--secondary" onClick={() => setSearchQuery("")}>Clear search</button>}
            </div>
          ) : (
            <ul className="asset-list" role="listbox">
              {filteredAssets.map((asset) => (
                <li
                  key={asset.assetId}
                  className={`asset-list-item ${selectedAssetId === asset.assetId ? "asset-list-item--selected" : ""}`}
                  onClick={() => handleAssetSelect(asset)}
                  role="option"
                  aria-selected={selectedAssetId === asset.assetId}
                >
                  <div className="asset-list-item__preview">
                    {asset.mediaKind === "model3d" ? (
                      <div className="asset-preview-placeholder"><span className="preview-format preview-format--model3d">MODEL 3D</span></div>
                    ) : asset.mediaKind === "video" ? (
                      <div className="asset-preview-placeholder"><span className="preview-format">VIDEO</span></div>
                    ) : asset.imageWidth && asset.imageHeight ? (
                      <div className="asset-preview-placeholder" style={{ aspectRatio: `${asset.imageWidth} / ${asset.imageHeight}` }}>
                        <span className="preview-format">{asset.imageFormat}</span>
                      </div>
                    ) : (
                      <div className="asset-preview-placeholder"><span className="preview-format">?</span></div>
                    )}
                  </div>
                  <div className="asset-list-item__info">
                    <span className="asset-list-item__name" title={asset.originalFilename}>{asset.originalFilename}</span>
                    <span className="asset-list-item__meta">
                      {asset.mediaKind === "video" && "VIDEO • "}
                      {asset.mediaKind === "model3d" && "MODEL 3D • "}
                      {(asset.mediaWidth ?? asset.imageWidth) && (asset.mediaHeight ?? asset.imageHeight) && `${asset.mediaWidth ?? asset.imageWidth}×${asset.mediaHeight ?? asset.imageHeight} • `}
                      {formatFileSize(asset.fileSize)}
                      {asset.hasAlpha && " • α"}
                    </span>
                  </div>
                  <span className={`status-badge status-badge--${asset.status}`}>{asset.status.toUpperCase()}</span>
                </li>
              ))}
            </ul>
          )}
        </aside>

        <div className="assets-detail">
          {selectedAsset ? (
            <article className="asset-detail">
              <header className="asset-detail__header">
                <h3>{selectedAsset.originalFilename}</h3>
                <span className={`status-badge status-badge--${selectedAsset.status}`}>{selectedAsset.status.toUpperCase()}</span>
              </header>

              <div className="asset-detail__preview">
                {selectedPreviewKind === "model3d" ? (
                  <div className="asset-preview asset-preview--model3d">
                    <strong>MODEL 3D</strong>
                    <span>Interactive 3D preview is not available in Phase 8.</span>
                  </div>
                ) : selectedPreviewKind === "video" ? (
                  <AssetVideoPreview key={selectedAsset.assetId} assetId={selectedAsset.assetId} />
                ) : previewLoading ? (
                  <div className="asset-preview asset-preview--loading">
                    <span>Loading preview...</span>
                  </div>
                ) : previewData ? (
                  <img
                    src={previewData}
                    alt={selectedAsset.originalFilename}
                    className="asset-preview-image"
                    style={selectedAsset.imageWidth && selectedAsset.imageHeight ? { aspectRatio: `${selectedAsset.imageWidth} / ${selectedAsset.imageHeight}` } : undefined}
                  />
                ) : (
                  <div className="asset-preview asset-preview--missing">
                    <span>{selectedAsset.mediaKind === "video" ? "Video preview unavailable" : "Preview unavailable"}</span>
                  </div>
                )}
              </div>

              <dl className="asset-metadata">
                <div className="metadata-row">
                  <dt>Asset ID</dt>
                  <dd className="metadata-value--mono">{selectedAsset.assetId}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Original Filename</dt>
                  <dd>{selectedAsset.originalFilename}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Media Kind</dt>
                  <dd>{selectedAsset.mediaKind === "video" ? "Video" : selectedAsset.mediaKind === "image" ? "Image" : selectedAsset.mediaKind === "model3d" ? "Model 3D" : "Unknown"}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Format</dt>
                  <dd>{selectedAsset.mediaKind === "model3d" ? formatModel3dAssetFormat(selectedAsset) : selectedAsset.mediaKind === "video" ? selectedAsset.mediaContainer || selectedAsset.mediaFormat || "Unknown" : selectedAsset.imageFormat || "Unknown"}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Dimensions</dt>
                  <dd>{(selectedAsset.mediaWidth ?? selectedAsset.imageWidth) && (selectedAsset.mediaHeight ?? selectedAsset.imageHeight) ? `${selectedAsset.mediaWidth ?? selectedAsset.imageWidth} × ${selectedAsset.mediaHeight ?? selectedAsset.imageHeight}` : "Unknown"}</dd>
                </div>
                {selectedAsset.mediaKind === "image" && <div className="metadata-row"><dt>Alpha Channel</dt><dd>{selectedAsset.hasAlpha ? "Yes" : "No"}</dd></div>}
                {selectedAsset.mediaKind === "video" && <>
                  <div className="metadata-row"><dt>Duration</dt><dd>{selectedAsset.durationMs != null ? `${(selectedAsset.durationMs / 1000).toFixed(2)} seconds` : "Unknown"}</dd></div>
                  <div className="metadata-row"><dt>FPS</dt><dd>{selectedAsset.fpsNumerator != null && selectedAsset.fpsDenominator ? (selectedAsset.fpsNumerator / selectedAsset.fpsDenominator).toFixed(2) : "Unknown"}</dd></div>
                  <div className="metadata-row"><dt>Codec</dt><dd>{selectedAsset.codec || "Unknown"}</dd></div>
                  <div className="metadata-row"><dt>Validation</dt><dd>{selectedAsset.validationLevel || "Unknown"}</dd></div>
                </>}
                {selectedAsset.mediaKind === "model3d" && <>
                  <div className="metadata-row"><dt>Metadata Schema</dt><dd>{selectedAsset.modelMetadataSchemaVersion === 1 ? "1" : selectedAsset.modelMetadataSchemaVersion == null ? "Unknown" : `Unsupported (${selectedAsset.modelMetadataSchemaVersion})`}</dd></div>
                  <div className="metadata-row"><dt>glTF Version</dt><dd>{selectedModelMetadata?.gltfVersion || "Unknown"}</dd></div>
                  <div className="metadata-row"><dt>Vertices</dt><dd>{formatModel3dCount(selectedModelMetadata?.vertexCount)}</dd></div>
                  <div className="metadata-row"><dt>Triangles</dt><dd>{formatModel3dCount(selectedModelMetadata?.triangleCount)}</dd></div>
                  <div className="metadata-row"><dt>Meshes</dt><dd>{formatModel3dCount(selectedModelMetadata?.meshCount)}</dd></div>
                  <div className="metadata-row"><dt>Primitives</dt><dd>{formatModel3dCount(selectedModelMetadata?.primitiveCount)}</dd></div>
                  <div className="metadata-row"><dt>Materials</dt><dd>{formatModel3dCount(selectedModelMetadata?.materialCount)}</dd></div>
                  <div className="metadata-row"><dt>Textures</dt><dd>{formatModel3dCount(selectedModelMetadata?.textureCount)}</dd></div>
                  <div className="metadata-row"><dt>Animations</dt><dd>{formatModel3dCount(selectedModelMetadata?.animationCount)}</dd></div>
                  <div className="metadata-row"><dt>Skin</dt><dd>{selectedModelMetadata ? selectedModelMetadata.hasSkin ? "Yes" : "No" : "Unknown"}</dd></div>
                  <div className="metadata-row"><dt>Bounds Min</dt><dd>{formatModel3dBounds(selectedModelMetadata?.boundsMin)}</dd></div>
                  <div className="metadata-row"><dt>Bounds Max</dt><dd>{formatModel3dBounds(selectedModelMetadata?.boundsMax)}</dd></div>
                  <div className="metadata-row"><dt>Validation</dt><dd>{selectedAsset.validationLevel || "Unknown"}</dd></div>
                </>}
                <div className="metadata-row">
                  <dt>File Size</dt>
                  <dd>{formatFileSize(selectedAsset.fileSize)}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Checksum (SHA-256)</dt>
                  <dd className="metadata-value--mono">{selectedAsset.checksum}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Imported</dt>
                  <dd>{formatTimestamp(selectedAsset.importedAtMs)}</dd>
                </div>
                <div className="metadata-row">
                  <dt>Master Path</dt>
                  <dd className="metadata-value--mono metadata-value--path">{selectedAsset.managedMasterPath}</dd>
                </div>
              </dl>
            </article>
          ) : (
            <div className="empty-state empty-state--detail">
              <div className="empty-state__glyph">IMG</div>
              <h3>Select an asset</h3>
              <p>Choose an asset from the list to view its details and preview.</p>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

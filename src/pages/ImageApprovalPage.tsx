import { useEffect, useState } from "react";
import { getAssetPreview, listAssets } from "../services/assets";
import { approveImageAsset } from "../services/model3dProcessing";
import type { AssetInfo } from "../types/core";
import { navigateTo, studioRoutes } from "./navigation";

export function ImageApprovalPage() {
  const [images, setImages] = useState<AssetInfo[]>([]);
  const [selected, setSelected] = useState<AssetInfo | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let mounted = true;
    listAssets()
      .then((assets) => {
        if (!mounted) return;
        const nextImages = assets.filter((asset) => asset.mediaKind === "image" && asset.status === "ready");
        setImages(nextImages);
        setSelected(nextImages[0] ?? null);
      })
      .catch((nextError: unknown) => { if (mounted) setError(nextError instanceof Error ? nextError.message : String(nextError)); })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; };
  }, []);

  useEffect(() => {
    if (!selected) { setPreview(null); return; }
    let mounted = true;
    getAssetPreview(selected.assetId).then((source) => { if (mounted) setPreview(source); }).catch(() => { if (mounted) setPreview(null); });
    return () => { mounted = false; };
  }, [selected]);

  const approve = async () => {
    if (!selected) return;
    setBusy(true);
    setError("");
    try {
      await approveImageAsset(selected.assetId);
      const approved = { ...selected, approvalStatus: "approved" as const };
      setSelected(approved);
      setImages((current) => current.map((asset) => asset.assetId === approved.assetId ? approved : asset));
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setBusy(false);
    }
  };

  if (loading) return <div className="image-generator-loading">Loading image approval…</div>;

  return (
    <section className="image-generator-page image-approval-page">
      <div className="image-generator-intro">
        <div><div className="panel-label">PIPELINE GATE A · IMAGE APPROVAL</div><h2>Review 2D Concept</h2><p>Only an approved image can move into 3D generation.</p></div>
        <button className="btn btn--secondary" type="button" onClick={() => navigateTo(studioRoutes.imageGeneration)}>Back to Image Generation</button>
      </div>
      {error && <div className="error-banner" role="alert">{error}</div>}
      {images.length === 0 ? (
        <div className="image-output-empty"><div className="empty-state__glyph">IMG</div><p>No ready images are available for approval.</p><button className="btn btn--primary" type="button" onClick={() => navigateTo(studioRoutes.imageGeneration)}>Generate a concept image</button></div>
      ) : (
        <div className="image-generator-layout">
          <aside className="image-generator-panel image-approval-list"><div className="image-panel-heading"><div><div className="panel-label">GENERATION HISTORY</div><h3>Concept images</h3></div></div>{images.map((image) => <button type="button" className={`home-recent-asset-card ${selected?.assetId === image.assetId ? "home-recent-asset-card--selected" : ""}`} key={image.assetId} onClick={() => setSelected(image)}><strong>{image.originalFilename}</strong><small>{image.approvalStatus === "approved" ? "APPROVED" : "READY FOR REVIEW"}</small></button>)}</aside>
          <section className="image-generator-panel image-output-panel">
            <div className="image-panel-heading"><div><div className="panel-label">REVIEW</div><h3>{selected?.originalFilename}</h3></div><span className={`status-badge status-badge--provider-${selected?.approvalStatus === "approved" ? "good" : "warning"}`}>{selected?.approvalStatus === "approved" ? "Approved" : "Ready for review"}</span></div>
            {preview && <img src={preview} alt="2D concept under review" className="asset-preview-image" />}
            <div className="checkpoint-actions-row"><button className="btn btn--primary" type="button" disabled={busy || selected?.approvalStatus === "approved"} onClick={() => void approve()}>{busy ? "Approving…" : selected?.approvalStatus === "approved" ? "Image Approved" : "Approve Image for 3D"}</button><button className="btn btn--secondary" type="button" onClick={() => navigateTo(studioRoutes.imageGeneration)}>Reject / Regenerate</button>{selected?.approvalStatus === "approved" && <button className="btn btn--secondary" type="button" onClick={() => { sessionStorage.setItem("nexora_selected_image_asset_id", selected.assetId); sessionStorage.setItem("nexora_initial_prompt", selected.originalFilename.replace(/\.[^.]+$/, "")); navigateTo(studioRoutes.model3dStudio); }}>Continue to 3D Model Studio</button>}</div>
          </section>
        </div>
      )}
    </section>
  );
}

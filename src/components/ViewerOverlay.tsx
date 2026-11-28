import type { buildAssetFilter } from "../assetFilter";
import Filmstrip from "./Filmstrip";
import type { Editor } from "../editors";
import { isImage, thumbUrl } from "./AssetGrid";
import AssetContextMenu from "./AssetContextMenu";
import FaceOverlay, { FaceToggle } from "./FaceOverlay";
import type { FormatTable } from "../types";
import type { AppInfo } from "../hooks/useNavigation";
import type {
  useQuarantineReview,
  useDeletionSuggestions,
  useDeletionActions,
  useQuarantine,
} from "../hooks/useQuarantine";
import type { useListingRefresh } from "../hooks/useNavigation";
import type { useAiSettings, useAi } from "../hooks/useAi";
import type { useViewer, useViewerImage, useViewerAssets, useViewerControls } from "../hooks/useViewer";
import type { useAssetValues } from "../hooks/useAssetValues";
import type { useIptc } from "../hooks/useIptc";
import type { useFileOperations } from "../hooks/useFileOperations";

type ViewerOverlayProps = {
  editors: Editor[];
  openEditor: (editor: Editor, assetIds: number[]) => void;
  appInfo: AppInfo | null;
  uiScale: number;
  total: number;
  refreshTick: number;
  formats: FormatTable;
  assetFilter: ReturnType<typeof buildAssetFilter>;
  review: Pick<ReturnType<typeof useQuarantineReview>, "quarantineBusy">;
  listing: Pick<ReturnType<typeof useListingRefresh>, "epoch">;
  aiSettings: Pick<ReturnType<typeof useAiSettings>, "aiStatus" | "aiReady">;
  suggestions: Pick<ReturnType<typeof useDeletionSuggestions>, "deletionSuggestions">;
  viewerState: Pick<
    ReturnType<typeof useViewer>,
    | "viewer"
    | "setViewer"
    | "slideshow"
    | "setSlideshow"
    | "slideshowSeconds"
    | "setSlideshowSeconds"
    | "viewerFit"
    | "setViewerFit"
    | "viewerZoom"
    | "setViewerZoom"
    | "viewerPan"
    | "setViewerPan"
    | "viewerLock"
    | "setViewerLock"
    | "rawDecode"
    | "setRawDecode"
    | "setViewerPreview"
    | "showFaces"
    | "setShowFaces"
    | "viewerImageRef"
    | "viewerCanvasRef"
    | "viewerOverlayRef"
    | "viewerMenuAnchor"
    | "openViewerMenu"
    | "closeViewerMenu"
  >;
  viewerImage: Pick<
    ReturnType<typeof useViewerImage>,
    | "viewerFileKey"
    | "preview"
    | "viewerFull"
    | "viewerFullLoading"
    | "viewerDimensions"
    | "viewerOriginal"
    | "viewerPixelRatio"
    | "viewerScale"
    | "viewerPercent"
    | "viewerSourceLabel"
    | "faceAiAvailable"
    | "faceAiReason"
    | "viewerFaceId"
    | "viewerFaces"
  >;
  deletion: Pick<ReturnType<typeof useDeletionActions>, "runDeletionAction">;
  assetValues: Pick<ReturnType<typeof useAssetValues>, "setAssetValues">;
  iptc: Pick<ReturnType<typeof useIptc>, "openIptcWrite">;
  viewerAssets: Pick<ReturnType<typeof useViewerAssets>, "showViewerAsset">;
  quarantine: Pick<ReturnType<typeof useQuarantine>, "requestQuarantineMove">;
  files: Pick<ReturnType<typeof useFileOperations>, "beginFileOperation">;
  viewerControls: Pick<ReturnType<typeof useViewerControls>, "moveViewer" | "clampViewerPan" | "wheelViewer">;
  ai: Pick<ReturnType<typeof useAi>, "findSimilar">;
};

export function ViewerOverlay({
  editors,
  openEditor,
  appInfo,
  uiScale,
  total,
  refreshTick,
  formats,
  assetFilter,
  review,
  listing,
  aiSettings,
  suggestions,
  viewerState,
  viewerImage,
  deletion,
  assetValues,
  iptc,
  viewerAssets,
  quarantine,
  files,
  viewerControls,
  ai,
}: ViewerOverlayProps) {
  const { quarantineBusy } = review;
  const { epoch } = listing;
  const { aiStatus, aiReady } = aiSettings;
  const { deletionSuggestions } = suggestions;
  const {
    viewer,
    setViewer,
    slideshow,
    setSlideshow,
    slideshowSeconds,
    setSlideshowSeconds,
    viewerFit,
    setViewerFit,
    viewerZoom,
    setViewerZoom,
    viewerPan,
    setViewerPan,
    viewerLock,
    setViewerLock,
    rawDecode,
    setRawDecode,
    setViewerPreview,
    showFaces,
    setShowFaces,
    viewerImageRef,
    viewerCanvasRef,
    viewerOverlayRef,
    viewerMenuAnchor,
    openViewerMenu,
    closeViewerMenu,
  } = viewerState;
  const {
    viewerFileKey,
    preview,
    viewerFull,
    viewerFullLoading,
    viewerDimensions,
    viewerOriginal,
    viewerPixelRatio,
    viewerScale,
    viewerPercent,
    viewerSourceLabel,
    faceAiAvailable,
    faceAiReason,
    viewerFaceId,
    viewerFaces,
  } = viewerImage;
  const { runDeletionAction } = deletion;
  const { setAssetValues } = assetValues;
  const { openIptcWrite } = iptc;
  const { showViewerAsset } = viewerAssets;
  const { requestQuarantineMove } = quarantine;
  const { beginFileOperation } = files;
  const { moveViewer, clampViewerPan, wheelViewer } = viewerControls;
  const { findSimilar } = ai;
  return (
    viewer && (
      <div className="viewer-overlay" ref={viewerOverlayRef} tabIndex={-1}>
        <div className="viewer-bar">
          <button onClick={() => setViewer(null)}>‹ Zavrieť</button>
          {!viewer.asset.isDirectory && isImage(viewer.asset, formats) && viewer.asset.quarantinedAt == null && (
            <>
              <button
                type="button"
                className="viewer-triage"
                title="U"
                aria-pressed={viewer.asset.editPendingSince != null}
                disabled={!!appInfo?.readonly}
                onClick={() => void setAssetValues([viewer.asset.id], { editPending: "toggle" })}
              >
                Na úpravu
              </button>
              <button
                type="button"
                className="viewer-triage"
                disabled={!!appInfo?.readonly || quarantineBusy || viewer.asset.offline || viewer.asset.missing}
                onClick={() => requestQuarantineMove([viewer.asset.id])}
              >
                Do karantény
              </button>
            </>
          )}
          {deletionSuggestions.has(viewer.asset.id) && (
            <>
              <span className="deletion-suggestion-badge" title="Návrh na vymazanie" aria-label="Návrh na vymazanie">
                ✕
              </span>
              <button
                disabled={!!appInfo?.readonly}
                onClick={() => void runDeletionAction("keep_suggested_assets", [viewer.asset.id])}
              >
                Odmietnuť návrh na vymazanie
              </button>
            </>
          )}
          {editors.map((editor, index) => (
            <button
              key={editor.path}
              title={`${editor.path}${index === 0 ? " (Ctrl+Alt+X)" : ""}`}
              disabled={viewer.asset.offline || viewer.asset.isDirectory}
              onClick={() => openEditor(editor, [viewer.asset.id])}
            >
              Otvoriť v: {editor.name}
            </button>
          ))}
          <button className="primary" onClick={() => setSlideshow((v) => !v)} disabled={total < 2}>
            {slideshow ? "❚❚ Pozastaviť" : "▶ Spustiť prezentáciu"}
          </button>
          <label>
            Interval{" "}
            <select
              aria-label="Interval prezentácie"
              value={slideshowSeconds}
              onChange={(e) => setSlideshowSeconds(Number(e.target.value))}
            >
              {[2, 3, 5, 10, 15, 30].map((seconds) => (
                <option key={seconds} value={seconds}>
                  {seconds} s
                </option>
              ))}
            </select>
          </label>
          <button
            title="Prepnúť medzi prispôsobením oknu a skutočnou veľkosťou"
            onClick={() => {
              setViewerFit(!viewerFit);
              setViewerZoom(1 / viewerPixelRatio);
              setViewerPan({ x: 0, y: 0 });
            }}
          >
            {viewerFit ? "100 %" : "Prispôsobiť"}
          </button>
          <label
            className="viewer-raw-toggle"
            title="Zapnuté: plné rozlíšenie RAW (pomalšie). Vypnuté: vložený náhľad (rýchle)."
          >
            <input
              type="checkbox"
              checked={rawDecode}
              onChange={(event) => setRawDecode(event.target.checked)}
              onKeyDown={(event) => {
                if (event.code === "Space") event.stopPropagation();
              }}
            />{" "}
            Dekódovať RAW
          </label>
          <FaceToggle
            checked={showFaces}
            onChange={setShowFaces}
            enabled={faceAiAvailable && viewerFaceId != null}
            reason={faceAiAvailable ? "Tváre sú dostupné pre fotky v katalógu" : faceAiReason}
          />
          <button title="Zamknúť priblíženie pri prechode na ďalšiu fotku" onClick={() => setViewerLock((v) => !v)}>
            {viewerLock ? "🔒" : "🔓"}
          </button>
          <span className="viewer-title">{viewer.asset.name}</span>
          <span className="viewer-info">
            {viewerDimensions && (
              <>
                {viewerPercent} % · {viewerOriginal?.width} × {viewerOriginal?.height} · {viewerSourceLabel}
              </>
            )}
            {viewerFullLoading && <small className="viewer-full-loading">Načítavam plné rozlíšenie…</small>}
          </span>
          <span className="viewer-count">
            {viewer.index + 1} / {total.toLocaleString("sk-SK")}
          </span>
        </div>
        <div
          ref={viewerCanvasRef}
          className="viewer-canvas"
          onContextMenu={openViewerMenu}
          onPointerDown={(e) => {
            if (e.button === 0 && !viewerFit && !(e.target as HTMLElement).closest(".viewer-arrow"))
              e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onWheel={wheelViewer}
          onPointerMove={(e) => {
            if (e.buttons === 1 && !viewerFit && e.currentTarget.hasPointerCapture(e.pointerId))
              setViewerPan((p) =>
                clampViewerPan({ x: p.x + e.movementX / uiScale, y: p.y + e.movementY / uiScale }, viewerZoom),
              );
          }}
          onDoubleClick={(e) => {
            if (!(e.target as HTMLElement).closest(".viewer-arrow")) setViewer(null);
          }}
        >
          <button className="viewer-arrow" onClick={() => void moveViewer(-1)}>
            ‹
          </button>
          {viewerFileKey && (viewerFull || !preview || preview.dimensions) ? (
            <img
              key={viewerFileKey}
              ref={viewerImageRef}
              className="viewer-image"
              style={
                viewerDimensions
                  ? {
                      width: viewerDimensions.width * viewerScale,
                      height: viewerDimensions.height * viewerScale,
                      maxWidth: "none",
                      maxHeight: "none",
                      objectFit: "contain",
                      flex: "none",
                      transform: viewerFit ? undefined : "translate(" + viewerPan.x + "px," + viewerPan.y + "px)",
                    }
                  : { maxWidth: "100%", maxHeight: "100%", objectFit: "contain" }
              }
              src={viewerFull?.url ?? thumbUrl(viewer.asset, 0, epoch)}
              alt={viewer.asset.name}
              onLoad={(event) => {
                if (!viewerFull)
                  setViewerPreview({
                    fileKey: viewerFileKey,
                    dimensions: {
                      width: event.currentTarget.naturalWidth,
                      height: event.currentTarget.naturalHeight,
                    },
                  });
              }}
              onError={() => {
                if (!viewerFull) setViewerPreview({ fileKey: viewerFileKey, dimensions: null });
              }}
            />
          ) : (
            <div className="viewer-nonimage">
              {viewer.asset.ext.toUpperCase()}
              <small>Náhľad nie je k dispozícii</small>
            </div>
          )}
          {!!viewerFaces?.length && <FaceOverlay imageRef={viewerImageRef} faces={viewerFaces} />}
          <button className="viewer-arrow" onClick={() => void moveViewer(1)}>
            ›
          </button>
        </div>
        {viewerMenuAnchor && (
          <AssetContextMenu
            anchor={viewerMenuAnchor}
            onClose={closeViewerMenu}
            asset={viewer.asset}
            actionIds={() => [viewer.asset.id]}
            offline={!!(viewer.asset.offline || viewer.asset.missing)}
            isPhoto={isImage(viewer.asset, formats)}
            editors={editors}
            readOnly={!!appInfo?.readonly}
            aiReady={aiReady}
            aiReason={aiStatus.reason ?? "AI modul nie je nainštalovaný"}
            onSetValues={setAssetValues}
            onFileOperation={beginFileOperation}
            onQuarantine={requestQuarantineMove}
            onWriteIptc={(ids) => void openIptcWrite(ids)}
            onFindSimilar={(asset) => {
              setViewer(null);
              void findSimilar(asset);
            }}
          />
        )}
        <Filmstrip
          key={refreshTick}
          filter={assetFilter}
          index={viewer.index}
          total={total}
          epoch={epoch}
          onPick={(asset, index) => {
            if (!asset.isDirectory && isImage(asset, formats)) showViewerAsset(asset, index);
          }}
        />
        <div className="viewer-footer">{viewer.asset.path}</div>
      </div>
    )
  );
}

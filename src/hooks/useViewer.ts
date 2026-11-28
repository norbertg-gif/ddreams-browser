import type { buildAssetFilter } from "../assetFilter";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type WheelEvent as ReactWheelEvent,
  type Dispatch,
  type SetStateAction,
  type RefObject,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  formatFor,
  isImage,
  thumbUrl,
  type AssetGridHandle,
  type AssetValuePatch,
  type GridAsset,
} from "../components/AssetGrid";
import { useAssetContextMenu } from "../components/AssetContextMenu";
import { useFullImage } from "../components/useFullImage";
import { useFaceOverlays } from "../components/FaceOverlay";
import type { ComparedPhoto } from "../components/CompareView";
import type { Asset, FormatTable, Page } from "../types";
import { readStored, writeStored } from "../storage";
import type { useAiSettings } from "./useAi";
import type { useGridSelectionState } from "./useAssetValues";

type ViewerOptions = {
  setToast: Dispatch<SetStateAction<string>>;
  grid: RefObject<AssetGridHandle | null>;
};

export function useViewer({ setToast, grid }: ViewerOptions) {
  const [viewer, setViewer] = useState<{ asset: Asset; index: number } | null>(null);
  const [comparison, setComparison] = useState<ComparedPhoto[] | null>(null);
  const closeComparison = useCallback(() => setComparison(null), []);
  const closeIfCompared = useCallback((ids: number[]) => {
    setComparison((current) => (current?.some((photo) => ids.includes(photo.asset.id)) ? null : current));
  }, []);
  const restoreGridFocus = useCallback(() => {
    requestAnimationFrame(() => {
      if (!currentViewer.current && !document.querySelector(".compare-overlay")) grid.current?.focus();
    });
  }, []);
  const removeCompared = useCallback((ids: number[]) => {
    if (!ids.length) return;
    const removed = new Set(ids);
    setComparison((current) => {
      if (!current) return current;
      const remaining = current.filter((photo) => !removed.has(photo.asset.id));
      return remaining.length < 2 ? null : remaining.length === current.length ? current : remaining;
    });
  }, []);
  const openComparison = (photos: GridAsset[]) => {
    if (photos.length < 2 || photos.length > 4) {
      setToast("Porovnať sa dá 2 až 4 fotografie.");
      window.setTimeout(() => setToast(""), 3000);
      return;
    }
    setComparison(photos.map((photo) => ({ ...photo, key: photo.asset.id })));
  };
  const currentViewer = useRef(viewer);
  const viewerValuePatches = useRef(new Map<number, AssetValuePatch>());
  currentViewer.current = viewer;
  // Returning from the full-screen viewer: show and focus the last viewed photo in the grid.
  const lastViewerIndex = useRef<number | null>(null);
  useEffect(() => {
    if (viewer) {
      lastViewerIndex.current = viewer.index;
      return;
    }
    const index = lastViewerIndex.current;
    lastViewerIndex.current = null;
    if (index != null) requestAnimationFrame(() => grid.current?.reveal(index));
  }, [viewer]);
  const [slideshow, setSlideshow] = useState(false);
  const [slideshowSeconds, setSlideshowSeconds] = useState(5);
  const [viewerFit, setViewerFit] = useState(true);
  const [viewerZoom, setViewerZoom] = useState(1);
  const [viewerPan, setViewerPan] = useState({ x: 0, y: 0 });
  const [viewerLock, setViewerLock] = useState(false);
  const [rawDecode, setRawDecode] = useState(() => readStored("ddreams.rawDecode", true));
  useEffect(() => writeStored("ddreams.rawDecode", rawDecode), [rawDecode]);
  const [viewerPreview, setViewerPreview] = useState<{
    fileKey: string;
    dimensions: { width: number; height: number } | null;
  } | null>(null);
  const [viewerFallbackLayout, setViewerFallbackLayout] = useState<{
    fileKey: string;
    width: number;
    height: number;
  } | null>(null);
  const [viewerCanvasSize, setViewerCanvasSize] = useState({ width: 0, height: 0 });
  const [showFaces, setShowFaces] = useState(() => readStored("ddreams.showFaces", false));
  useEffect(() => writeStored("ddreams.showFaces", showFaces), [showFaces]);
  const viewerImageRef = useRef<HTMLImageElement>(null);
  const viewerCanvasRef = useRef<HTMLDivElement>(null);
  const viewerOverlayRef = useRef<HTMLDivElement>(null);
  const viewerOpen = viewer !== null;
  const { anchor: viewerMenuAnchor, open: openViewerMenu, close: closeViewerMenu } = useAssetContextMenu();
  const viewerMenuOpen = viewerMenuAnchor !== null;
  useLayoutEffect(closeViewerMenu, [viewer?.asset.id, closeViewerMenu]);
  useEffect(() => {
    if (!viewerOpen) return;
    const priorFocus = document.activeElement;
    viewerOverlayRef.current?.focus({ preventScroll: true });
    return () => {
      viewerValuePatches.current.clear();
      if (priorFocus instanceof HTMLElement && priorFocus.isConnected) priorFocus.focus({ preventScroll: true });
    };
  }, [viewerOpen]);
  return {
    viewer,
    setViewer,
    comparison,
    setComparison,
    closeComparison,
    closeIfCompared,
    restoreGridFocus,
    removeCompared,
    openComparison,
    currentViewer,
    viewerValuePatches,
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
    viewerPreview,
    setViewerPreview,
    viewerFallbackLayout,
    setViewerFallbackLayout,
    viewerCanvasSize,
    setViewerCanvasSize,
    showFaces,
    setShowFaces,
    viewerImageRef,
    viewerCanvasRef,
    viewerOverlayRef,
    viewerOpen,
    viewerMenuAnchor,
    openViewerMenu,
    closeViewerMenu,
    viewerMenuOpen,
  };
}

type ViewerImageOptions = {
  uiScale: number;
  refreshTick: number;
  valueAliases: RefObject<Map<number, number>>;
  formats: FormatTable;
  aiSettings: Pick<ReturnType<typeof useAiSettings>, "aiStatus" | "aiEnabled" | "aiTick">;
  viewerState: Pick<
    ReturnType<typeof useViewer>,
    | "viewer"
    | "comparison"
    | "viewerFit"
    | "viewerZoom"
    | "rawDecode"
    | "viewerPreview"
    | "setViewerPreview"
    | "viewerFallbackLayout"
    | "setViewerFallbackLayout"
    | "viewerCanvasSize"
    | "showFaces"
  >;
};

export function useViewerImage({
  uiScale,
  refreshTick,
  valueAliases,
  formats,
  aiSettings,
  viewerState,
}: ViewerImageOptions) {
  const { aiStatus, aiEnabled, aiTick } = aiSettings;
  const {
    viewer,
    comparison,
    viewerFit,
    viewerZoom,
    rawDecode,
    viewerPreview,
    setViewerPreview,
    viewerFallbackLayout,
    setViewerFallbackLayout,
    viewerCanvasSize,
    showFaces,
  } = viewerState;
  const viewerReadId = viewer ? (valueAliases.current.get(viewer.asset.id) ?? viewer.asset.id) : null;
  const viewerFileKey =
    viewer?.asset.path && !viewer.asset.isDirectory
      ? JSON.stringify([viewerReadId, viewer.asset.path, viewer.asset.fileModified, viewer.asset.size])
      : null;
  const viewerIsRaw = !!viewer && formatFor(viewer.asset, formats)?.group === "raw";
  const preview = viewerPreview?.fileKey === viewerFileKey ? viewerPreview : null;
  const fullAssetId =
    viewer && isImage(viewer.asset, formats) && !viewer.asset.offline && !viewer.asset.missing ? viewer.asset.id : null;
  const { frame: viewerFull, loading: viewerFullLoading } = useFullImage(
    viewerFileKey,
    fullAssetId,
    !!preview,
    viewerIsRaw && rawDecode,
  );
  useLayoutEffect(() => {
    if (!viewerFileKey) {
      setViewerPreview(null);
      setViewerFallbackLayout(null);
    } else if (viewerFull && !preview?.dimensions) {
      setViewerFallbackLayout((current) =>
        current?.fileKey === viewerFileKey
          ? current
          : { fileKey: viewerFileKey, width: viewerFull.width, height: viewerFull.height },
      );
    }
  }, [viewerFileKey, preview, viewerFull]);
  // Use the first viewable frame as the layout reference, even when the source resolution changes.
  const viewerDimensions =
    preview?.dimensions ??
    (viewerFallbackLayout?.fileKey === viewerFileKey ? viewerFallbackLayout : null) ??
    viewerFull;
  const viewerOriginal = viewerFull
    ? { width: viewerFull.originalWidth, height: viewerFull.originalHeight }
    : viewerDimensions;
  const viewerPixelRatio =
    viewerDimensions && viewerOriginal
      ? uiScale *
        Math.min(viewerDimensions.width / viewerOriginal.width, viewerDimensions.height / viewerOriginal.height)
      : 1;
  const viewerFitScale =
    viewerDimensions && viewerCanvasSize.width > 140 && viewerCanvasSize.height > 36
      ? Math.min(
          1,
          (viewerCanvasSize.width - 140) / viewerDimensions.width,
          (viewerCanvasSize.height - 36) / viewerDimensions.height,
        )
      : 1;
  const viewerScale = viewerFit ? viewerFitScale : viewerZoom;
  const viewerPercent = Math.round(viewerScale * viewerPixelRatio * 100);
  const viewerMaxZoom = Math.max(8, 8 / viewerPixelRatio);
  const viewerMinZoom = Math.min(0.05, viewerFitScale);
  const viewerSourceLabel = viewerFull
    ? {
        original: "originál",
        pillow: "plné rozlíšenie",
        rawFull: "RAW · plné rozlíšenie",
        embedded: "vložený náhľad",
        rawReduced: "RAW · polovičné rozlíšenie",
      }[viewerFull.source]
    : "náhľad";
  const faceAiAvailable = aiStatus.available && aiEnabled;
  const faceAiReason = aiStatus.reason ?? (aiStatus.available ? "AI je vypnutá" : "AI modul nie je nainštalovaný");
  const viewerFaceId = viewer && viewer.asset.id > 0 && isImage(viewer.asset, formats) ? viewer.asset.id : null;
  const faceAssetIds = comparison
    ? comparison.map((photo) => photo.asset.id)
    : viewerFaceId == null
      ? []
      : [viewerFaceId];
  const facesByAsset = useFaceOverlays(
    faceAssetIds,
    showFaces && faceAiAvailable,
    refreshTick,
    aiStatus.state + ":" + aiTick,
  );
  const viewerFaces = viewerFaceId == null ? undefined : facesByAsset?.get(viewerFaceId);
  return {
    viewerReadId,
    viewerFileKey,
    preview,
    viewerFull,
    viewerFullLoading,
    viewerDimensions,
    viewerOriginal,
    viewerPixelRatio,
    viewerScale,
    viewerPercent,
    viewerMaxZoom,
    viewerMinZoom,
    viewerSourceLabel,
    faceAiAvailable,
    faceAiReason,
    viewerFaceId,
    facesByAsset,
    viewerFaces,
  };
}

type ViewerAssetsOptions = {
  formats: FormatTable;
  currentAssetFilter: RefObject<ReturnType<typeof buildAssetFilter>>;
  selection: Pick<ReturnType<typeof useGridSelectionState>, "setSelectedIds" | "setPrimaryId" | "setPrimaryAsset">;
  viewerState: Pick<
    ReturnType<typeof useViewer>,
    | "setViewer"
    | "closeComparison"
    | "removeCompared"
    | "currentViewer"
    | "viewerValuePatches"
    | "setViewerFit"
    | "setViewerZoom"
    | "setViewerPan"
    | "viewerLock"
  >;
};

export function useViewerAssets({ formats, currentAssetFilter, selection, viewerState }: ViewerAssetsOptions) {
  const { setSelectedIds, setPrimaryId, setPrimaryAsset } = selection;
  const {
    setViewer,
    closeComparison,
    removeCompared,
    currentViewer,
    viewerValuePatches,
    setViewerFit,
    setViewerZoom,
    setViewerPan,
    viewerLock,
  } = viewerState;
  const showViewerAsset = useCallback(
    (asset: Asset, index: number) => {
      const patch = viewerValuePatches.current.get(asset.id);
      const shown = patch ? { ...asset, ...patch } : asset;
      const next = { asset: shown, index };
      currentViewer.current = next;
      setViewer(next);
      setPrimaryId(shown.id);
      setPrimaryAsset(shown);
      setSelectedIds(new Set([shown.id]));
      if (!viewerLock) {
        setViewerFit(true);
        setViewerZoom(1);
        setViewerPan({ x: 0, y: 0 });
      }
    },
    [viewerLock],
  );

  const openComparedPhoto = (photo: ComparedPhoto) => {
    closeComparison();
    showViewerAsset(photo.asset, photo.index);
    const shown = currentViewer.current;
    const filter = currentAssetFilter.current;
    void invoke<number | null>("locate_asset", {
      filter: { ...filter, offset: 0, limit: 1 } satisfies import("../bindings/AssetFilter").AssetFilter,
      id: photo.asset.id,
    })
      .then((index) => {
        if (index === null || index === photo.index || currentAssetFilter.current !== filter) return;
        setViewer((current) => (current && current === shown ? { ...current, index } : current));
      })
      .catch(() => {});
  };

  const advanceAfterRemoval = useCallback(
    async (removedIds: number[]) => {
      removeCompared(removedIds);
      const shown = currentViewer.current;
      if (!shown || !removedIds.includes(shown.asset.id)) return;
      const filter = currentAssetFilter.current;
      const stillShowing = () =>
        currentViewer.current?.asset.id === shown.asset.id && currentAssetFilter.current === filter;
      const canShow = (asset: Asset) =>
        !removedIds.includes(asset.id) &&
        !asset.isDirectory &&
        !asset.offline &&
        !asset.missing &&
        !!asset.path &&
        isImage(asset, formats);
      const pageSize = 60;
      try {
        let total = 0;
        for (let offset = shown.index; ; offset += pageSize) {
          const page = await invoke<Page>("get_assets", {
            filter: { ...filter, offset, limit: pageSize } satisfies import("../bindings/AssetFilter").AssetFilter,
          });
          if (!stillShowing()) return;
          total = page.total;
          const index = page.items.findIndex(canShow);
          if (index !== -1) {
            showViewerAsset(page.items[index], offset + index);
            return;
          }
          if (offset + pageSize >= total) break;
        }
        for (let end = Math.min(shown.index, total); end > 0;) {
          const offset = Math.max(0, end - pageSize);
          const page = await invoke<Page>("get_assets", {
            filter: { ...filter, offset, limit: end - offset } satisfies import("../bindings/AssetFilter").AssetFilter,
          });
          if (!stillShowing()) return;
          for (let index = page.items.length - 1; index >= 0; index--) {
            if (canShow(page.items[index])) {
              showViewerAsset(page.items[index], offset + index);
              return;
            }
          }
          end = offset;
        }
      } finally {
        if (stillShowing()) {
          currentViewer.current = null;
          setViewer(null);
        }
      }
    },
    [formats, removeCompared, showViewerAsset],
  );
  return { showViewerAsset, openComparedPhoto, advanceAfterRemoval };
}

type ViewerControlsOptions = {
  uiScale: number;
  epoch: number;
  total: number;
  setToast: Dispatch<SetStateAction<string>>;
  formats: FormatTable;
  assetFilter: ReturnType<typeof buildAssetFilter>;
  showViewerAsset: (asset: Asset, index: number) => void;
  viewerState: Pick<
    ReturnType<typeof useViewer>,
    | "viewer"
    | "setViewer"
    | "slideshow"
    | "setSlideshow"
    | "slideshowSeconds"
    | "viewerFit"
    | "setViewerFit"
    | "viewerZoom"
    | "setViewerZoom"
    | "viewerPan"
    | "setViewerPan"
    | "setViewerCanvasSize"
    | "viewerCanvasRef"
    | "viewerOpen"
    | "closeViewerMenu"
    | "viewerMenuOpen"
  >;
  viewerImage: Pick<
    ReturnType<typeof useViewerImage>,
    "viewerReadId" | "viewerDimensions" | "viewerScale" | "viewerMaxZoom" | "viewerMinZoom"
  >;
};

export function useViewerControls({
  uiScale,
  epoch,
  total,
  setToast,
  formats,
  assetFilter,
  showViewerAsset,
  viewerState,
  viewerImage,
}: ViewerControlsOptions) {
  const {
    viewer,
    setViewer,
    slideshow,
    setSlideshow,
    slideshowSeconds,
    viewerFit,
    setViewerFit,
    viewerZoom,
    setViewerZoom,
    viewerPan,
    setViewerPan,
    setViewerCanvasSize,
    viewerCanvasRef,
    viewerOpen,
    closeViewerMenu,
    viewerMenuOpen,
  } = viewerState;
  const { viewerReadId, viewerDimensions, viewerScale, viewerMaxZoom, viewerMinZoom } = viewerImage;
  const moveViewer = useCallback(
    async (delta: number) => {
      if (!viewer) return;
      let index = viewer.index + delta;
      while (index >= 0 && index < total) {
        const page = await invoke<Page>("get_assets", {
          filter: { ...assetFilter, offset: index, limit: 1 } satisfies import("../bindings/AssetFilter").AssetFilter,
        });
        const asset = page.items[0];
        if (!asset) break;
        if (!asset.isDirectory && isImage(asset, formats)) {
          showViewerAsset(asset, index);
          return;
        }
        index += delta;
      }
      setSlideshow(false);
    },
    [viewer, total, assetFilter, showViewerAsset, formats],
  );
  useEffect(() => {
    if (!viewer) setSlideshow(false);
  }, [viewer]);
  useEffect(() => {
    if (!slideshow || !viewer) return;
    if (viewer.index >= total - 1) {
      setSlideshow(false);
      return;
    }
    const timer = window.setTimeout(
      () =>
        void moveViewer(1).catch(() => {
          setSlideshow(false);
          setToast("Prezentáciu sa nepodarilo pokračovať.");
        }),
      slideshowSeconds * 1000,
    );
    return () => window.clearTimeout(timer);
  }, [slideshow, viewer, total, slideshowSeconds, moveViewer]);
  useEffect(() => {
    if (!viewer) return;
    const key = (event: globalThis.KeyboardEvent) => {
      if (
        viewerMenuOpen ||
        event.defaultPrevented ||
        document.querySelector(".modal-backdrop,[role=dialog],[role=alertdialog]")
      )
        return;
      if (event.key === "Escape" || event.key === "Enter") setViewer(null);
      else if (
        !viewerFit &&
        event.key.startsWith("Arrow") &&
        (event.ctrlKey || event.key === "ArrowUp" || event.key === "ArrowDown")
      ) {
        event.preventDefault();
        const step = event.shiftKey ? 12 : 48;
        setViewerPan((p) =>
          clampViewerPan(
            {
              x: p.x + (event.key === "ArrowRight" ? -step : event.key === "ArrowLeft" ? step : 0),
              y: p.y + (event.key === "ArrowDown" ? -step : event.key === "ArrowUp" ? step : 0),
            },
            viewerZoom,
          ),
        );
      } else if (event.key === "ArrowRight" && !event.altKey) void moveViewer(1);
      else if (event.key === "ArrowLeft" && !event.altKey) void moveViewer(-1);
      else if (event.code === "Space") {
        event.preventDefault();
        void moveViewer(1);
      } else if (event.key === "Backspace") {
        event.preventDefault();
        void moveViewer(-1);
      } else if (event.key === "+" || event.key === "=") {
        setViewerFit(false);
        setViewerZoom(Math.min(viewerMaxZoom, viewerScale * 1.2));
      } else if (event.key === "-") {
        setViewerFit(false);
        setViewerZoom(Math.max(viewerMinZoom, viewerScale / 1.2));
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [
    viewer,
    moveViewer,
    viewerFit,
    viewerZoom,
    viewerDimensions,
    viewerMenuOpen,
    viewerScale,
    viewerMinZoom,
    viewerMaxZoom,
  ]);
  useLayoutEffect(() => {
    if (!viewerOpen || !viewerCanvasRef.current) return;
    const el = viewerCanvasRef.current;
    const update = () => setViewerCanvasSize({ width: el.clientWidth, height: el.clientHeight });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, [viewerOpen]);
  // The zoomed photo is centred in the viewer; panning is limited so it never leaves a gap it could fill.
  const clampViewerPan = (pan: { x: number; y: number }, zoom: number) => {
    const el = viewerCanvasRef.current,
      d = viewerDimensions;
    if (!el || !d) return pan;
    const mx = Math.max(0, (d.width * zoom - el.clientWidth) / 2),
      my = Math.max(0, (d.height * zoom - el.clientHeight) / 2);
    return { x: Math.max(-mx, Math.min(mx, pan.x)), y: Math.max(-my, Math.min(my, pan.y)) };
  };
  const wheelViewer = (e: ReactWheelEvent<HTMLDivElement>) => {
    e.preventDefault();
    closeViewerMenu();
    const r = e.currentTarget.getBoundingClientRect();
    const cx = (e.clientX - r.left - r.width / 2) / uiScale,
      cy = (e.clientY - r.top - r.height / 2) / uiScale;
    const from = viewerScale,
      pan = viewerFit ? { x: 0, y: 0 } : viewerPan;
    const to = Math.max(viewerMinZoom, Math.min(viewerMaxZoom, from * (e.deltaY < 0 ? 1.12 : 0.89))),
      k = to / from;
    setViewerFit(false);
    setViewerZoom(to);
    setViewerPan(clampViewerPan({ x: cx - (cx - pan.x) * k, y: cy - (cy - pan.y) * k }, to));
  };
  useEffect(() => {
    if (!viewer) return;
    for (const offset of [-1, 1]) {
      const index = viewer.index + offset;
      if (index < 0 || index >= total) continue;
      void invoke<Page>("get_assets", {
        filter: { ...assetFilter, offset: index, limit: 1 } satisfies import("../bindings/AssetFilter").AssetFilter,
      })
        .then((page) => {
          const asset = page.items[0];
          if (asset && !asset.isDirectory && asset.path) {
            const image = new Image();
            image.src = thumbUrl(asset, 0, epoch);
          }
        })
        .catch(() => {});
    }
  }, [viewerReadId, viewer?.index, assetFilter, total, epoch]);
  return { moveViewer, clampViewerPan, wheelViewer };
}

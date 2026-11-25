import { useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type Ref } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Editor } from "../editors";
import { readStored, writeStored } from "../storage";
import type { Asset, AssetQuality, FormatTable } from "../types";
import AssetContextMenu, { useAssetContextMenu } from "./AssetContextMenu";
import { formatFor, labelColor, labelText, QUALITY_FLAG_TEXT, thumbUrl, type GridAsset } from "./AssetGrid";
import { createFullImageQueue, useFullImage } from "./useFullImage";
import FaceOverlay, { FaceToggle } from "./FaceOverlay";
import type { FaceInfo } from "./rightPanelData";

export type ComparedPhoto = GridAsset & { key: number };

type Props = {
  photos: ComparedPhoto[];
  initialActive: number | null;
  epoch: number;
  qualityToken: number;
  uiScale: number;
  formats: FormatTable;
  rawDecode: boolean;
  onRawDecode: (decode: boolean) => void;
  showFaces: boolean;
  onShowFaces: (shown: boolean) => void;
  facesByAsset?: ReadonlyMap<number, FaceInfo[]>;
  faceAiAvailable: boolean;
  faceAiReason: string;
  editors: Editor[];
  readOnly: boolean;
  aiReady: boolean;
  aiReason: string;
  onClose: () => void;
  onRestoreFocus: () => void;
  onDrop: (key: number) => void;
  onOpen: (photo: ComparedPhoto) => void;
  onSetValues: (ids: number[], changes: Record<string, unknown>) => void;
  onFileOperation: (ids: number[], mode: "move" | "copy" | "rename" | "delete" | "delete-permanent", asset?: Asset) => void;
  onQuarantine: (ids: number[]) => void;
  onFindSimilar: (asset: Asset) => void;
};

type Camera = { x: number; y: number; zoom: { pixels: number } | null };
type Size = { width: number; height: number };
type PaneHandle = { zoom: (factor: number) => void; focus: () => void };
const fit: Camera = { x: 0.5, y: 0.5, zoom: null };
const labels = ["Red", "Yellow", "Green", "Blue", "Purple", "Gray"];
const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

export default function CompareView({
  photos, initialActive, epoch, qualityToken, uiScale, formats, rawDecode, onRawDecode, editors, readOnly, aiReady, aiReason,
  onClose, onRestoreFocus, onDrop, onOpen, onSetValues, onFileOperation, onQuarantine, onFindSimilar,
  showFaces, onShowFaces, facesByAsset, faceAiAvailable, faceAiReason,
}: Props) {
  const [activeKey, setActiveKey] = useState(initialActive ?? photos[0].key);
  const active = photos.find(photo => photo.key === activeKey) ?? photos[0];
  const [sync, setSync] = useState(() => readStored("ddreams.compareSync", true));
  const [shared, setShared] = useState(fit);
  const [local, setLocal] = useState<Record<number, Camera>>({});
  const [queue] = useState(createFullImageQueue);
  const panes = useRef(new Map<number, PaneHandle>());
  const { anchor, open: openMenu, close: closeMenu } = useAssetContextMenu();
  const [quality, setQuality] = useState(new Map<number, AssetQuality>());
  const assetIds = photos.map(photo => photo.asset.id).join(",");
  useEffect(() => {
    let active = true;
    void invoke<AssetQuality[]>("get_asset_quality", { assetIds: assetIds.split(",").map(Number) }).then(rows => {
      if (active) setQuality(new Map(rows.map(row => [row.assetId, row])));
    }).catch(() => {});
    return () => { active = false; };
  }, [assetIds, qualityToken]);
  const sharpest = photos.map(photo => quality.get(photo.asset.id)).reduce<AssetQuality | undefined>(
    (best, row) => row && Number.isFinite(row.sharp) && (!best || row.sharp > best.sharp) ? row : best, undefined,
  );

  useLayoutEffect(() => {
    // The grid provides its focus target even when a portal button opened the table.
    return onRestoreFocus;
  }, [onRestoreFocus]);
  useLayoutEffect(() => { panes.current.get(active.key)?.focus(); }, [active.key]);

  const updateCamera = (key: number, next: Camera) => {
    if (sync) setShared(next);
    else setLocal(current => ({ ...current, [key]: next }));
  };
  const toggleSync = (locked: boolean) => {
    if (locked) setShared(local[active.key] ?? fit);
    else setLocal(Object.fromEntries(photos.map(photo => [photo.key, shared])));
    setSync(locked);
    writeStored("ddreams.compareSync", locked);
  };

  useEffect(() => {
    const keyDown = (event: globalThis.KeyboardEvent) => {
      // The catalogue panels also listen on window for F2, even without focus.
      if (event.key === "F2") {
        event.preventDefault();
        event.stopImmediatePropagation();
        return;
      }
      if (event.defaultPrevented || anchor || document.querySelector(".modal-backdrop,[role=alertdialog]")) return;
      const { key, ctrlKey, altKey, shiftKey, metaKey } = event;
      const plain = !ctrlKey && !altKey && !metaKey;
      const asset = active.asset;
      const ids = [asset.id];
      const digit = event.code.startsWith("Digit") ? event.code.slice(5) : key;
      const edit = (changes: Record<string, unknown>) => {
        if (!event.repeat) onSetValues(ids, changes);
      };
      const target = event.target;
      const editing = (target instanceof HTMLInputElement && target.type !== "checkbox") || target instanceof HTMLSelectElement
        || target instanceof HTMLTextAreaElement || (target instanceof HTMLElement && target.isContentEditable);
      if (key === "Escape") onClose();
      else if (plain && (key === "Tab" || (!editing && (key === "ArrowLeft" || key === "ArrowRight")))) {
        const index = photos.indexOf(active);
        const step = key === "ArrowLeft" || (key === "Tab" && shiftKey) ? -1 : 1;
        setActiveKey(photos[(index + step + photos.length) % photos.length].key);
      } else if (editing) return;
      else if (plain && ["+", "=", "-", "−"].includes(key)) panes.current.get(active.key)?.zoom(key === "+" || key === "=" ? 1.2 : 1 / 1.2);
      else if (ctrlKey && !altKey && !shiftKey && !metaKey && /^[0-5]$/.test(digit)) edit({ rating: Number(digit) || null });
      else if (altKey && !ctrlKey && !shiftKey && !metaKey && /^[0-6]$/.test(digit)) edit({ label: labels[Number(digit) - 1] ?? null });
      else if (plain && !shiftKey && key.toLowerCase() === "x") edit({ label: labelColor(asset.label ?? "") === labelColor("Gray") ? null : "Gray" });
      else if (plain && !shiftKey && key === "\\") edit({ tagged: "toggle" });
      else if (plain && !shiftKey && key.toLowerCase() === "u") edit({ editPending: "toggle" });
      else if (plain && key === "Delete") {
        if (!event.repeat) onFileOperation(ids, shiftKey ? "delete-permanent" : "delete", asset);
      } else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", keyDown, true);
    return () => window.removeEventListener("keydown", keyDown, true);
  }, [active, anchor, onClose, onFileOperation, onSetValues, photos]);

  return <section className="compare-overlay" role="dialog" aria-modal="true" aria-label="Porovnávací stôl">
    <header className="compare-bar">
      <button onClick={onClose}>‹ Zavrieť</button>
      <strong>Porovnávací stôl</strong>
      <label><input type="checkbox" checked={sync} onChange={event => toggleSync(event.target.checked)} /> Zamknúť zoom a posun</label>
      <label title="Zapnuté: plné rozlíšenie RAW (pomalšie). Vypnuté: vložený náhľad (rýchle).">
        <input type="checkbox" checked={rawDecode} onChange={event => onRawDecode(event.target.checked)} /> Dekódovať RAW
      </label>
      <FaceToggle checked={showFaces} onChange={onShowFaces} enabled={faceAiAvailable && photos.some(photo => photo.asset.id > 0)}
        reason={faceAiAvailable ? "Tváre sú dostupné pre fotky v katalógu" : faceAiReason} />
    </header>
    <div className={"compare-grid compare-grid-" + photos.length}>
      {photos.map(photo => <ComparePane key={photo.key} asset={photo.asset}
        ref={handle => { if (handle) panes.current.set(photo.key, handle); else panes.current.delete(photo.key); }}
        active={photo.key === active.key} epoch={epoch} uiScale={uiScale} queue={queue}
        rawDecode={rawDecode && formatFor(photo.asset, formats)?.group === "raw"}
        camera={sync ? shared : local[photo.key] ?? fit}
        onCamera={camera => updateCamera(photo.key, camera)}
        onActivate={() => setActiveKey(photo.key)}
        onContextMenu={event => { setActiveKey(photo.key); openMenu(event); }}
        onDrop={photos.length > 2 ? () => { closeMenu(); onDrop(photo.key); } : undefined}
        faces={facesByAsset?.get(photo.asset.id)} quality={quality.get(photo.asset.id)} sharpest={sharpest?.assetId === photo.asset.id} />)}
    </div>
    {anchor && <AssetContextMenu anchor={anchor} onClose={closeMenu} asset={active.asset}
      actionIds={() => [active.asset.id]} offline={!!(active.asset.offline || active.asset.missing)} isPhoto
      editors={editors} readOnly={readOnly} aiReady={aiReady} aiReason={aiReason}
      onOpen={() => onOpen(active)} onSetValues={onSetValues} onFileOperation={onFileOperation}
      onQuarantine={onQuarantine} onFindSimilar={onFindSimilar} />}
  </section>;
}

type PaneProps = {
  ref: Ref<PaneHandle>;
  asset: Asset;
  active: boolean;
  epoch: number;
  uiScale: number;
  queue: ReturnType<typeof createFullImageQueue>;
  rawDecode: boolean;
  camera: Camera;
  onCamera: (camera: Camera) => void;
  onActivate: () => void;
  onContextMenu: (event: React.MouseEvent) => void;
  onDrop?: () => void;
  quality?: AssetQuality;
  faces?: FaceInfo[];
  sharpest: boolean;
};

function ComparePane({
  ref, asset, active, epoch, uiScale, queue, rawDecode, camera, onCamera, onActivate, onContextMenu, onDrop, quality, sharpest, faces,
}: PaneProps) {
  const canvas = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const drag = useRef<{ clientX: number; clientY: number; x: number; y: number } | null>(null);
  const [viewport, setViewport] = useState<Size>({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const measure = () => setViewport({ width: element.clientWidth, height: element.clientHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const fileKey = asset.path ? JSON.stringify([asset.path, asset.fileModified, asset.size]) : null;
  const [preview, setPreview] = useState<{ key: string; size: Size | null }>();
  const ready = !!preview && preview.key === fileKey;
  const { frame, loading } = useFullImage(fileKey, asset.offline || asset.missing ? null : asset.id, ready, rawDecode, queue);
  const [fallback, setFallback] = useState<{ key: string; size: Size }>();
  useLayoutEffect(() => {
    if (fileKey && frame && !preview?.size) {
      setFallback(current => current?.key === fileKey ? current : { key: fileKey, size: { width: frame.width, height: frame.height } });
    }
  }, [fileKey, frame, preview]);
  const size = (ready ? preview?.size : null) ?? (fallback?.key === fileKey ? fallback?.size : null) ?? frame;
  let originalWidth = Math.max(0, frame?.originalWidth ?? asset.width ?? 0) || size?.width || 1;
  let originalHeight = Math.max(0, frame?.originalHeight ?? asset.height ?? 0) || size?.height || 1;
  if (!frame && size && (size.width > size.height) !== (originalWidth > originalHeight)) {
    [originalWidth, originalHeight] = [originalHeight, originalWidth];
  }
  const pixelRatio = size ? uiScale * Math.min(size.width / originalWidth, size.height / originalHeight) : uiScale;
  // A new raster keeps the displayed footprint until the next zoom gesture establishes a real-pixel scale.
  const [basis, setBasis] = useState({ zoom: camera.zoom, fileKey, hasSize: !!size, pixelRatio });
  const newBasis = basis.zoom !== camera.zoom || basis.fileKey !== fileKey || basis.hasSize !== !!size;
  if (newBasis) setBasis({ zoom: camera.zoom, fileKey, hasSize: !!size, pixelRatio });
  const ratio = newBasis ? pixelRatio : basis.pixelRatio;
  const fitted = size && viewport.width && viewport.height ? Math.min(1, viewport.width / size.width, viewport.height / size.height) : 1;
  const scale = camera.zoom ? camera.zoom.pixels / ratio : fitted;
  const width = (size?.width ?? 0) * scale;
  const height = (size?.height ?? 0) * scale;
  const maxX = Math.max(0, (width - viewport.width) / 2);
  const maxY = Math.max(0, (height - viewport.height) / 2);
  const x = clamp((0.5 - camera.x) * width, -maxX, maxX);
  const y = clamp((0.5 - camera.y) * height, -maxY, maxY);

  const zoom = (factor: number, point?: { x: number; y: number }) => {
    if (!size) return;
    const pixels = clamp(scale * pixelRatio * factor, Math.min(0.05, fitted * pixelRatio), 8);
    const nextScale = pixels / pixelRatio;
    const centerX = maxX ? 0.5 - x / width : camera.x;
    const centerY = maxY ? 0.5 - y / height : camera.y;
    const shift = (offset: number, current: number, next: number) => offset / current - offset / next;
    onCamera({
      zoom: { pixels },
      x: clamp(centerX + shift(point?.x ?? 0, width, size.width * nextScale), 0, 1),
      y: clamp(centerY + shift(point?.y ?? 0, height, size.height * nextScale), 0, 1),
    });
  };
  useImperativeHandle(ref, () => ({ zoom, focus: () => canvas.current?.focus({ preventScroll: true }) }));
  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      onActivate();
      const rect = element.getBoundingClientRect();
      zoom(event.deltaY < 0 ? 1.12 : 1 / 1.12, {
        x: (event.clientX - rect.left - rect.width / 2) / uiScale,
        y: (event.clientY - rect.top - rect.height / 2) / uiScale,
      });
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  });

  const flags = quality?.flags.map(flag => QUALITY_FLAG_TEXT[flag] ?? flag).join(", ");
  const rating = clamp(asset.rating ?? 0, 0, 5);
  return <article className={"compare-pane" + (active ? " active" : "")} onClick={onActivate} onContextMenu={onContextMenu}>
    {onDrop && <button className="compare-drop" title="Odobrať z porovnania" aria-label={"Odobrať " + asset.name + " z porovnania"}
      onClick={event => { event.stopPropagation(); onDrop(); }}>×</button>}
    <div className="compare-canvas" ref={canvas} tabIndex={-1} aria-label={asset.name}
      onFocus={onActivate}
      onDoubleClick={event => {
        onActivate();
        if (camera.zoom || !width || !height) { onCamera(camera.zoom ? fit : { ...fit, zoom: { pixels: 1 } }); return; }
        // Zoom to 100 % around the clicked spot (e.g. the eyes), not the image centre.
        const rect = event.currentTarget.getBoundingClientRect();
        const px = (event.clientX - rect.left - rect.width / 2) / uiScale - x;
        const py = (event.clientY - rect.top - rect.height / 2) / uiScale - y;
        onCamera({ zoom: { pixels: 1 }, x: clamp(0.5 + px / width, 0, 1), y: clamp(0.5 + py / height, 0, 1) });
      }}
      onPointerDown={event => {
        if (event.button !== 0) return;
        onActivate();
        event.currentTarget.focus({ preventScroll: true });
        if (!camera.zoom || !size) return;
        drag.current = { clientX: event.clientX, clientY: event.clientY, x, y };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onLostPointerCapture={() => { drag.current = null; }}
      onPointerMove={event => {
        if (!drag.current || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
        const dx = (event.clientX - drag.current.clientX) / uiScale;
        const dy = (event.clientY - drag.current.clientY) / uiScale;
        onCamera({
          zoom: camera.zoom,
          x: maxX ? 0.5 - clamp(drag.current.x + dx, -maxX, maxX) / width : camera.x,
          y: maxY ? 0.5 - clamp(drag.current.y + dy, -maxY, maxY) / height : camera.y,
        });
      }}>
      {fileKey && (frame || !ready || preview?.size) ? <img key={fileKey} ref={imageRef} className="compare-image" alt={asset.name} draggable={false}
        style={size ? { width, height, transform: "translate(-50%, -50%) translate(" + x + "px," + y + "px)" } : undefined}
        src={frame?.url ?? thumbUrl(asset, 0, epoch)}
        onLoad={event => {
          if (!frame) setPreview({ key: fileKey, size: { width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight } });
        }}
        onError={() => { if (!frame) setPreview({ key: fileKey, size: null }); }}
      /> : <span>Náhľad nie je k dispozícii</span>}
      {!!faces?.length && <FaceOverlay imageRef={imageRef} faces={faces} />}
    </div>
    <footer className="compare-caption">
      <strong title={asset.path ?? asset.name}>{asset.name}</strong>
      <span title={"Hodnotenie " + rating + " z 5"}>{"★".repeat(rating) + "☆".repeat(5 - rating)}</span>
      {asset.label && <span className="compare-label"><i style={{ backgroundColor: labelColor(asset.label) }} />{labelText(asset.label)}</span>}
      {!!asset.tagged && <span>Označené</span>}
      {asset.editPendingSince != null && <span>Na úpravu</span>}
      {quality && <span>Ostrosť {quality.sharp.toLocaleString("sk-SK", { maximumFractionDigits: 2 })}{flags && " · " + flags}</span>}
      {sharpest && <span className="compare-sharpest">najostrejšia</span>}
      <span className="compare-zoom">{size && Math.round(scale * pixelRatio * 100) + " %"}</span>
      {loading && <small>Načítavam plné rozlíšenie…</small>}
    </footer>
  </article>;
}

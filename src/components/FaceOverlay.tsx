import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { FaceInfo } from "./rightPanelData";

export const FACE_OVERLAY_REFRESH = "ddreams-faces-changed";

export function useFaceOverlays(assetIds: number[], enabled: boolean, refreshToken: number, aiToken: string) {
  const key = enabled ? [...new Set(assetIds.filter(id => id > 0))].sort((a, b) => a - b).join(",") : "";
  const [result, setResult] = useState<{ key: string; faces: Map<number, FaceInfo[]> }>();
  useEffect(() => {
    if (!key) {
      setResult(undefined);
      return;
    }
    let request = 0;
    const refresh = async () => {
      const version = ++request;
      try {
        const assetIds = key.split(",").map(Number);
        const rows = await invoke<FaceInfo[]>("ai_faces_for_assets", { assetIds });
        if (version !== request) return;
        const faces = new Map<number, FaceInfo[]>();
        for (const row of rows) {
          if (row.x == null || row.y == null || row.w == null || row.h == null) continue;
          const group = faces.get(row.assetId);
          if (group) group.push(row);
          else faces.set(row.assetId, [row]);
        }
        setResult({ key, faces });
      } catch {
        if (version === request) setResult(undefined);
      }
    };
    void refresh();
    window.addEventListener(FACE_OVERLAY_REFRESH, refresh);
    return () => {
      request++;
      window.removeEventListener(FACE_OVERLAY_REFRESH, refresh);
    };
  }, [key, refreshToken, aiToken]);
  return result?.key === key ? result.faces : undefined;
}

export function FaceToggle({ checked, onChange, enabled, reason }: {
  checked: boolean;
  onChange: (shown: boolean) => void;
  enabled: boolean;
  reason: string;
}) {
  return <label className="face-toggle" title={enabled ? "Zobraziť rámčeky tvárí s menami osôb" : reason}>
    <input type="checkbox" checked={checked} disabled={!enabled} onChange={event => onChange(event.target.checked)}
      onKeyDown={event => { if (event.code === "Space") event.stopPropagation(); }} /> Tváre
  </label>;
}

type Geometry = { x: number; y: number; width: number; height: number; viewWidth: number; viewHeight: number; labelHeight: number };
const clamp = (value: number, min = 0, max = 1) => Math.max(min, Math.min(max, value));

export default function FaceOverlay({ imageRef, faces }: {
  imageRef: RefObject<HTMLImageElement | null>;
  faces: FaceInfo[];
}) {
  const layerRef = useRef<HTMLDivElement>(null);
  const measured = useRef<Geometry | undefined>(undefined);
  const [geometry, setGeometry] = useState<Geometry>();
  useLayoutEffect(() => {
    const image = imageRef.current;
    const layer = layerRef.current;
    const update = (next?: Geometry) => {
      const previous = measured.current;
      if (next && previous && (Object.keys(next) as (keyof Geometry)[]).every(key => previous[key] === next[key])) return;
      if (previous === next) return;
      measured.current = next;
      setGeometry(next);
    };
    if (!image || !layer) {
      update();
      return;
    }
    const measure = () => {
      const raster = image.getBoundingClientRect();
      const viewport = layer.getBoundingClientRect();
      if (!raster.width || !raster.height || !viewport.width || !viewport.height || !layer.clientHeight || !image.naturalWidth || !image.naturalHeight) {
        update();
        return;
      }
      // Both views use centered object-fit: contain within a separately sized image element.
      const scale = Math.min(raster.width / image.naturalWidth, raster.height / image.naturalHeight);
      const width = image.naturalWidth * scale;
      const height = image.naturalHeight * scale;
      update({
        x: raster.left + (raster.width - width) / 2 - viewport.left,
        y: raster.top + (raster.height - height) / 2 - viewport.top,
        width, height, viewWidth: viewport.width, viewHeight: viewport.height,
        labelHeight: 19 * viewport.height / layer.clientHeight,
      });
    };
    // Camera changes commit styles before this effect; load and resize also run independently.
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(image);
    observer.observe(layer);
    image.addEventListener("load", measure);
    image.addEventListener("error", measure);
    return () => {
      observer.disconnect();
      image.removeEventListener("load", measure);
      image.removeEventListener("error", measure);
    };
  });

  return <div className="face-overlay" ref={layerRef}>
    {geometry && faces.map(face => {
      const { x, y, w, h } = face;
      if (x == null || y == null || w == null || h == null || ![x, y, w, h].every(Number.isFinite)) return null;
      const left = geometry.x + geometry.width * clamp(x);
      const top = geometry.y + geometry.height * clamp(y);
      const right = geometry.x + geometry.width * clamp(x + w);
      const bottom = geometry.y + geometry.height * clamp(y + h);
      if (right <= left || bottom <= top || right <= 0 || bottom <= 0 || left >= geometry.viewWidth || top >= geometry.viewHeight) return null;
      const percentX = (value: number) => value / geometry.viewWidth * 100 + "%";
      const percentY = (value: number) => value / geometry.viewHeight * 100 + "%";
      const labelTop = clamp(bottom + geometry.labelHeight <= geometry.viewHeight ? bottom : top - geometry.labelHeight,
        0, Math.max(0, geometry.viewHeight - geometry.labelHeight));
      const labelSide = left > geometry.viewWidth * 0.65
        ? { right: percentX(Math.max(0, geometry.viewWidth - right)), maxWidth: percentX(Math.min(right, geometry.viewWidth)) }
        : { left: percentX(Math.max(0, left)), maxWidth: percentX(geometry.viewWidth - Math.max(0, left)) };
      const suggested = face.state === "auto" || face.state === "suggested";
      const name = face.name?.trim();
      const title = !name ? "Neznáma tvár" : suggested ? "Návrh – potvrď v paneli Vlastnosti" : name;
      return <div key={face.faceId}>
        <span className={suggested ? "face-overlay-box suggested" : "face-overlay-box"}
          style={{ left: percentX(left), top: percentY(top), width: percentX(right - left), height: percentY(bottom - top) }} />
        <span className="face-overlay-label" style={{ ...labelSide, top: percentY(labelTop) }} title={title}>
          {name ? name + (suggested ? "?" : "") : "?"}
        </span>
      </div>;
    })}
  </div>;
}

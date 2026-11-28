import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Asset, Page } from "../types";
import { thumbUrl } from "./AssetGrid";

type Data = import("../bindings/AiMapData").AiMapData;
type Props = {
  epoch: number;
  selectedIds: Set<number>;
  onSelect: (asset: Asset, index: number, event: ReactMouseEvent | ReactKeyboardEvent) => void;
  onNavigate: (ids: number[], title: string) => void;
  onGrid: () => void;
  onOpenNeighborhood: (ids: number[]) => void;
};
const MAP_THUMB_LIMIT = 150,
  CELL = 72,
  BUCKET = 0.035;
const palette = [
  "#70a6a0",
  "#c58a76",
  "#8c9dc1",
  "#b3a36d",
  "#a17fa6",
  "#78a07b",
  "#c17d91",
  "#7099b8",
  "#b88f62",
  "#789b9a",
  "#a68e78",
  "#8b91a9",
];
let savedMapView: { scale: number; tx: number; ty: number } = { scale: 1, tx: 0, ty: 0 };
export default function MapView({ epoch, selectedIds, onSelect, onNavigate, onGrid, onOpenNeighborhood }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null),
    host = useRef<HTMLDivElement>(null),
    dataRef = useRef<Data | null>(null),
    buckets = useRef(new Map<string, number[]>()),
    assets = useRef(new Map<number, Asset>()),
    images = useRef(new Map<number, HTMLImageElement>()),
    loading = useRef(0),
    assetRequests = useRef(new Set<number>()),
    view = useRef(savedMapView),
    drag = useRef<{ x: number; y: number; tx: number; ty: number } | null>(null),
    hover = useRef(-1),
    drawn = useRef<{ indices: number[]; visible: Set<number>; thumbs: Set<number> } | null>(null);
  const [data, setData] = useState<Data | null>(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [hoverAsset, setHoverAsset] = useState<Asset | null>(null),
    [size, setSize] = useState({ w: 1, h: 1 });
  const load = useCallback(async () => {
    try {
      setError("");
      const d = await invoke<Data>("ai_map_data");
      dataRef.current = d;
      setData(d);
      const b = new Map<string, number[]>();
      for (let i = 0; i < d.ids.length; i++) {
        const key = `${Math.floor(d.x[i] / BUCKET)},${Math.floor(d.y[i] / BUCKET)}`;
        const a = b.get(key) ?? [];
        a.push(i);
        b.set(key, a);
      }
      buckets.current = b;
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  const build = async () => {
    setBusy(true);
    setError("");
    try {
      await invoke("ai_map_build");
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const transform = (x: number, y: number) => {
    const v = view.current;
    const s = Math.min(size.w, size.h) * 0.88 * v.scale;
    return { x: size.w / 2 + v.tx + (x - 0.5) * s, y: size.h / 2 + v.ty + (y - 0.5) * s };
  };
  const visibleIndices = () => {
    const d = dataRef.current;
    if (!d) return [];
    const v = view.current,
      s = Math.min(size.w, size.h) * 0.88 * v.scale;
    const x1 = Math.max(0, 0.5 + (-size.w / 2 - v.tx) / s),
      x2 = Math.min(1, 0.5 + (size.w / 2 - v.tx) / s),
      y1 = Math.max(0, 0.5 + (-size.h / 2 - v.ty) / s),
      y2 = Math.min(1, 0.5 + (size.h / 2 - v.ty) / s);
    const out: number[] = [];
    for (let by = Math.floor(y1 / BUCKET); by <= Math.floor(y2 / BUCKET); by++)
      for (let bx = Math.floor(x1 / BUCKET); bx <= Math.floor(x2 / BUCKET); bx++)
        for (const i of buckets.current.get(`${bx},${by}`) ?? []) {
          if (d.x[i] >= x1 && d.x[i] <= x2 && d.y[i] >= y1 && d.y[i] <= y2) out.push(i);
        }
    return out;
  };
  function cacheSet<T>(cache: Map<number, T>, id: number, item: T, limit: number) {
    cache.delete(id);
    cache.set(id, item);
    if (cache.size > limit) {
      const oldest = cache.keys().next();
      if (!oldest.done) cache.delete(oldest.value);
    }
  }
  const ensureAsset = async (id: number) => {
    if (assets.current.has(id)) return assets.current.get(id)!;
    try {
      const page = await invoke<Page>("get_assets", {
        filter: {
          kind: "people",
          id: null,
          rootId: null,
          includeChildren: false,
          ids: [id],
          folderPath: null,
          value: null,
          query: null,
          sort: "name",
          descending: false,
          offset: 0,
          limit: 1,
          ratingMode: null,
          ratingValue: null,
          labelFilter: null,
          taggedOnly: false,
          fileGroup: null,
          dateFrom: null,
          dateTo: null,
          dateSource: null,
          dateNoDate: false,
          includeCategories: false,
          includeKeywords: false,
        } satisfies import("../bindings/AssetFilter").AssetFilter,
      });
      const a = page.items[0];
      if (a) cacheSet(assets.current, id, a, 2048);
      return a;
    } catch {
      return undefined;
    }
  };
  const ensureAssets = async (ids: number[]) => {
    const wanted = ids.filter((id) => !assets.current.has(id) && !assetRequests.current.has(id));
    if (!wanted.length) return;
    wanted.forEach((id) => assetRequests.current.add(id));
    try {
      const page = await invoke<Page>("get_assets", {
        filter: {
          kind: "people",
          id: null,
          rootId: null,
          includeChildren: false,
          ids: wanted,
          folderPath: null,
          value: null,
          query: null,
          sort: "name",
          descending: false,
          offset: 0,
          limit: wanted.length,
          ratingMode: null,
          ratingValue: null,
          labelFilter: null,
          taggedOnly: false,
          fileGroup: null,
          dateFrom: null,
          dateTo: null,
          dateSource: null,
          dateNoDate: false,
          includeCategories: false,
          includeKeywords: false,
        } satisfies import("../bindings/AssetFilter").AssetFilter,
      });
      for (const asset of page.items) cacheSet(assets.current, asset.id, asset, 2048);
    } catch {
    } finally {
      wanted.forEach((id) => assetRequests.current.delete(id));
      drawRef.current?.();
    }
  };
  const drawRef = useRef<(() => void) | null>(null);
  const draw = useCallback(() => {
    const el = canvas.current,
      d = dataRef.current;
    if (!el) return;
    const ctx = el.getContext("2d");
    if (!ctx) return;
    const ratio = window.devicePixelRatio || 1;
    el.width = Math.round(size.w * ratio);
    el.height = Math.round(size.h * ratio);
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, size.w, size.h);
    if (!d) return;
    const shown = visibleIndices();
    const doThumb = shown.length <= MAP_THUMB_LIMIT || (view.current.scale > 6 && shown.length > MAP_THUMB_LIMIT);
    let thumbSet = new Set<number>();
    if (doThumb) {
      if (shown.length <= MAP_THUMB_LIMIT) thumbSet = new Set(shown);
      else {
        const cells = new Set<string>();
        for (const i of shown) {
          const q = transform(d.x[i], d.y[i]),
            key = `${Math.floor(q.x / CELL)},${Math.floor(q.y / CELL)}`;
          if (!cells.has(key) && thumbSet.size < MAP_THUMB_LIMIT) {
            cells.add(key);
            thumbSet.add(i);
          }
        }
      }
    }
    drawn.current = { indices: shown, visible: new Set(shown), thumbs: thumbSet };
    for (const c of d.clusters) {
      const q = transform(c.x, c.y),
        r = Math.max(0, c.r * Math.min(size.w, size.h) * 0.88 * view.current.scale);
      ctx.beginPath();
      ctx.arc(q.x, q.y, r, 0, Math.PI * 2);
      ctx.fillStyle = palette[c.id % palette.length] + "12";
      ctx.fill();
    }
    for (const i of shown) {
      const q = transform(d.x[i], d.y[i]),
        id = d.ids[i],
        selected = selectedIds.has(id);
      const im = images.current.get(id);
      if (thumbSet.has(i) && im?.complete && im.naturalWidth) {
        cacheSet(images.current, id, im, 512);
        ctx.drawImage(im, q.x - 32, q.y - 32, 64, 64);
        if (selected) {
          ctx.strokeStyle = "#7db8ff";
          ctx.lineWidth = 3;
          ctx.strokeRect(q.x - 33, q.y - 33, 66, 66);
        }
      } else {
        ctx.beginPath();
        ctx.arc(q.x, q.y, selected ? 5 : hover.current === i ? 4 : 2.2, 0, Math.PI * 2);
        ctx.fillStyle = selected ? "#7db8ff" : palette[d.cluster[i] % palette.length];
        ctx.fill();
      }
    }
    const labels = [...d.clusters].sort((a, b) => b.r - a.r),
      occupied: { x: number; y: number; w: number; h: number }[] = [];
    ctx.font = "12px sans-serif";
    ctx.textAlign = "center";
    for (const c of labels) {
      const q = transform(c.x, c.y),
        r = c.r * Math.min(size.w, size.h) * 0.88 * view.current.scale;
      if (!c.name || r < 40) continue;
      const w = ctx.measureText(c.name).width + 12,
        h = 20;
      if (occupied.some((a) => Math.abs(a.x - q.x) < (a.w + w) / 2 && Math.abs(a.y - q.y) < (a.h + h) / 2)) continue;
      occupied.push({ x: q.x, y: q.y, w, h });
      ctx.lineWidth = 4;
      ctx.strokeStyle = "#15191e";
      ctx.strokeText(c.name, q.x, q.y + 4);
      ctx.fillStyle = "#e9edf2";
      ctx.fillText(c.name, q.x, q.y + 4);
    }
    if (thumbSet.size) void ensureAssets([...thumbSet].map((i) => d.ids[i]));
    for (const i of thumbSet) {
      const id = d.ids[i];
      if (images.current.has(id) || loading.current >= 8) continue;
      const a = assets.current.get(id);
      if (!a?.path) continue;
      loading.current++;
      const im = new Image();
      cacheSet(images.current, id, im, 512);
      im.onload = im.onerror = () => {
        loading.current--;
        draw();
      };
      im.src = thumbUrl(a, 160, epoch);
    }
  }, [size, epoch, selectedIds]);
  drawRef.current = draw;
  useEffect(() => {
    draw();
  }, [draw, data, hoverAsset]);
  const closest = (x: number, y: number) => {
    const d = dataRef.current,
      layout = drawn.current;
    if (!d || !layout) return -1;
    const scale = Math.min(size.w, size.h) * 0.88 * view.current.scale,
      rad = 32 / scale,
      bx = Math.floor((0.5 + (x - size.w / 2 - view.current.tx) / scale) / BUCKET),
      by = Math.floor((0.5 + (y - size.h / 2 - view.current.ty) / scale) / BUCKET),
      span = Math.ceil(rad / BUCKET) + 1,
      candidates: number[] = [];
    for (let cy = by - span; cy <= by + span; cy++)
      for (let cx = bx - span; cx <= bx + span; cx++)
        for (const i of buckets.current.get(`${cx},${cy}`) ?? []) if (layout.visible.has(i)) candidates.push(i);
    candidates.sort((a, b) => b - a);
    for (const i of candidates) {
      const q = transform(d.x[i], d.y[i]);
      if (
        layout.thumbs.has(i) &&
        images.current.get(d.ids[i])?.complete &&
        images.current.get(d.ids[i])?.naturalWidth
      ) {
        if (Math.abs(x - q.x) <= 32 && Math.abs(y - q.y) <= 32) return i;
      } else if (Math.hypot(q.x - x, q.y - y) <= 10) return i;
    }
    return -1;
  };
  const onMove = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect(),
      x = e.clientX - r.left,
      y = e.clientY - r.top;
    if (drag.current && e.buttons) {
      view.current.tx = drag.current.tx + x - drag.current.x;
      view.current.ty = drag.current.ty + y - drag.current.y;
      savedMapView = view.current;
      draw();
      return;
    }
    const i = closest(x, y);
    if (i !== hover.current) {
      hover.current = i;
      draw();
      if (i >= 0) void ensureAsset(dataRef.current!.ids[i]).then((a) => setHoverAsset(a ?? null));
      else setHoverAsset(null);
    }
  };
  const commit = (ids: number[], title: string) => {
    onNavigate(ids, title);
    onGrid();
  };
  const openPoint = (i: number) => {
    const d = dataRef.current;
    if (i < 0 || !d) return;
    const nearest = Array.from({ length: d.ids.length }, (_, j) => ({
      j,
      d: Math.hypot(d.x[j] - d.x[i], d.y[j] - d.y[i]),
    }))
      .sort((a, b) => a.d - b.d)
      .slice(0, 201)
      .map((v) => d.ids[v.j]);
    onGrid();
    onOpenNeighborhood(nearest);
  };
  const openSelected = () => {
    if (selectedIds.size !== 1 || !data) return;
    const i = data.ids.indexOf([...selectedIds][0]);
    if (i >= 0) openPoint(i);
  };
  const onClick = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect(),
      i = closest(e.clientX - r.left, e.clientY - r.top),
      d = dataRef.current;
    if (i < 0 || !d) return;
    void ensureAsset(d.ids[i]).then((a) => {
      if (a)
        onSelect(a, i, {
          ctrlKey: e.ctrlKey || e.metaKey,
          metaKey: false,
          shiftKey: false,
          altKey: false,
          button: 0,
          clientX: e.clientX,
          clientY: e.clientY,
        } as unknown as ReactMouseEvent);
    });
  };

  const openVisible = () => {
    const d = dataRef.current;
    if (!d) return;
    const ids = visibleIndices()
      .map((i) => ({ i, ...transform(d.x[i], d.y[i]) }))
      .sort(
        (a, b) => Math.floor(a.y / CELL) - Math.floor(b.y / CELL) || Math.floor(a.x / CELL) - Math.floor(b.x / CELL),
      )
      .slice(0, 20000)
      .map((v) => d.ids[v.i]);
    commit(ids, "Výrez mapy");
  };
  return (
    <div className="map-view" ref={host}>
      {!data?.ids.length ? (
        <div className="map-empty">
          <b>Mapa ešte nie je vypočítaná</b>
          {error && <small>{error}</small>}
          <button disabled={busy} onClick={() => void build()}>
            {busy ? "Počítam mapu…" : "Vypočítať mapu"}
          </button>
        </div>
      ) : (
        <>
          <div className="map-tools">
            <button
              onClick={() => {
                view.current = { scale: 1, tx: 0, ty: 0 };
                savedMapView = view.current;
                draw();
              }}
            >
              Celá mapa
            </button>
            <button disabled={selectedIds.size !== 1} onClick={openSelected}>
              Otvoriť
            </button>
            <button disabled={busy} onClick={() => void build()}>
              {busy ? "Počítam mapu…" : "Prepočítať"}
            </button>
            <button onClick={openVisible}>Viditeľné v mriežke</button>
            <small>
              {data.ids.length.toLocaleString("sk-SK")} fotiek · {data.clusters.length} ostrovov
            </small>
          </div>
          <canvas
            ref={canvas}
            onPointerDown={(e) => {
              drag.current = { x: e.clientX, y: e.clientY, tx: view.current.tx, ty: view.current.ty };
              e.currentTarget.setPointerCapture(e.pointerId);
            }}
            onPointerMove={onMove}
            onPointerUp={() => {
              drag.current = null;
              savedMapView = view.current;
            }}
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === "Enter" && selectedIds.size === 1) {
                e.preventDefault();
                openSelected();
              }
            }}
            onDoubleClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect(),
                i = closest(e.clientX - r.left, e.clientY - r.top);
              openPoint(i);
            }}
            onClick={onClick}
            onWheel={(e) => {
              e.preventDefault();
              const r = e.currentTarget.getBoundingClientRect(),
                x = e.clientX - r.left - r.width / 2,
                y = e.clientY - r.top - r.height / 2,
                v = view.current,
                k = e.deltaY < 0 ? 1.15 : 1 / 1.15;
              v.scale = Math.max(1, Math.min(400, v.scale * k));
              v.tx = x - (x - v.tx) * k;
              v.ty = y - (y - v.ty) * k;
              savedMapView = v;
              draw();
            }}
          />
          {hoverAsset && (
            <div className="map-hover">
              <img src={thumbUrl(hoverAsset, 160, epoch)} />
              <b>{hoverAsset.name}</b>
            </div>
          )}
        </>
      )}
    </div>
  );
}

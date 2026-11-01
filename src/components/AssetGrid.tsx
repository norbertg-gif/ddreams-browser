import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Asset, Page, View } from "../types";

const PAGE_SIZE = 120;
const IMAGE_TYPES = new Set(["jpg", "jpeg", "png", "webp", "tif", "tiff", "arw", "dng", "cr2", "nef", "raf", "orf", "rw2", "pef", "acr", "raw"]);
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
export function isImage(asset: Asset): boolean { return IMAGE_TYPES.has(asset.ext.toLowerCase()); }

type Props = {
  view: View; query: string; sort: string; descending: boolean; zoom: number; epoch: number;
  selectedIds: Set<number>; primaryId: number | null;
  onSelect: (asset: Asset, index: number, event: MouseEvent | KeyboardEvent) => void;
  onSelectRange: (ids: number[], asset: Asset, index: number) => void;
  onSelectAll: (ids: number[]) => void;
  onOpen: (asset: Asset, index: number) => void;
  onCounts: (total: number, size: number) => void;
  onViewport: () => void;
};

export default function AssetGrid({ view, query, sort, descending, zoom, epoch, selectedIds, primaryId, onSelect, onSelectRange, onSelectAll, onOpen, onCounts, onViewport }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<number, Asset>());
  const pages = useRef(new Set<number>());
  const requestVersion = useRef(0);
  const anchorIndex = useRef(0);
  const scrollPulse = useRef(0);
  const [total, setTotal] = useState(0);
  const [itemsVersion, setItemsVersion] = useState(0);
  const [columns, setColumns] = useState(5);
  const params = useMemo(() => ({ kind: view.kind, id: view.id ?? null, ids: view.ids ?? null, folderPath: view.folderPath ?? null, value: view.value ?? null, query, sort, descending }), [view, query, sort, descending]);
  const paramsKey = JSON.stringify(params);
  const tileWidth = zoom + 26;
  const rowHeight = zoom + 58;

  const fetchPage = useCallback(async (offset: number, version: number) => {
    const pageNo = Math.floor(offset / PAGE_SIZE);
    if (pages.current.has(pageNo)) return;
    pages.current.add(pageNo);
    try {
      const result = await invoke<Page>("get_assets", { filter: { ...params, offset: pageNo * PAGE_SIZE, limit: PAGE_SIZE } });
      if (version !== requestVersion.current) return;
      result.items.forEach((asset, i) => cache.current.set(pageNo * PAGE_SIZE + i, asset));
      setTotal(result.total); onCounts(result.total, result.totalSize); setItemsVersion(v => v + 1);
    } catch (error) { pages.current.delete(pageNo); console.error("Katalóg sa nepodarilo načítať", error); }
  }, [params, onCounts]);

  useEffect(() => {
    requestVersion.current++;
    cache.current.clear(); pages.current.clear(); setTotal(0); setItemsVersion(v => v + 1);
    scrollRef.current?.scrollTo({ top: 0 });
    void fetchPage(0, requestVersion.current);
  }, [paramsKey, fetchPage]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setColumns(Math.max(1, Math.floor((el.clientWidth - 20) / tileWidth))));
    observer.observe(el); return () => observer.disconnect();
  }, [tileWidth]);

  const rowCount = Math.ceil(total / columns);
  const virtualizer = useVirtualizer({ count: rowCount, getScrollElement: () => scrollRef.current, estimateSize: () => rowHeight, overscan: 4 });
  const virtualRows = virtualizer.getVirtualItems();
  useEffect(() => {
    if (!virtualRows.length) return;
    const start = virtualRows[0].index * columns;
    const end = Math.min(total, (virtualRows[virtualRows.length - 1].index + 1) * columns);
    for (let offset = Math.floor(start / PAGE_SIZE) * PAGE_SIZE; offset < end; offset += PAGE_SIZE) void fetchPage(offset, requestVersion.current);
  }, [virtualRows, columns, total, fetchPage, itemsVersion]);

  const getAt = (index: number) => cache.current.get(index);
  const handleTileClick = async (event: MouseEvent<HTMLDivElement>, asset: Asset, index: number) => {
    scrollRef.current?.focus();
    if (event.shiftKey) {
      const start=Math.min(anchorIndex.current,index),end=Math.max(anchorIndex.current,index);const ids:number[]=[];
      for(let offset=Math.floor(start/PAGE_SIZE)*PAGE_SIZE;offset<=end;offset+=PAGE_SIZE){await fetchPage(offset,requestVersion.current);for(let i=Math.max(start,offset);i<=Math.min(end,offset+PAGE_SIZE-1);i++){const item=getAt(i);if(item)ids.push(item.id)}}
      onSelectRange(ids,asset,index);
    } else { anchorIndex.current=index;onSelect(asset,index,event); }
  };
  const onKeyDown = async (event: KeyboardEvent<HTMLDivElement>) => {
    if (document.activeElement !== event.currentTarget) return;
    if (event.ctrlKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      const ids: number[] = [];
      for (let offset = 0; offset < total; offset += 500) {
        const page = await invoke<number[]>("get_asset_ids", { filter: { ...params, offset: 0, limit: 500 }, offset, limit: 500 });
        ids.push(...page);
        if (page.length < 500) break;
      }
      onSelectAll(ids);
      return;
    }
    if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(event.key)) return;
    event.preventDefault();
    const current = [...cache.current.entries()].find(([, a]) => a.id === primaryId)?.[0] ?? 0;
    const delta = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : event.key === "ArrowUp" ? -columns : columns;
    const next = Math.max(0, Math.min(total - 1, current + delta));
    await fetchPage(next, requestVersion.current);
    const asset = getAt(next); if (!asset) return;
    const synthetic = { key: event.key, ctrlKey: event.ctrlKey, shiftKey: event.shiftKey, metaKey: event.metaKey } as KeyboardEvent;
    if (event.shiftKey) {
      const start=Math.min(anchorIndex.current,next),end=Math.max(anchorIndex.current,next);const ids:number[]=[];
      for(let offset=Math.floor(start/PAGE_SIZE)*PAGE_SIZE;offset<=end;offset+=PAGE_SIZE){await fetchPage(offset,requestVersion.current);for(let i=Math.max(start,offset);i<=Math.min(end,offset+PAGE_SIZE-1);i++){const item=getAt(i);if(item)ids.push(item.id)}}
      onSelectRange(ids,asset,next);
    } else { anchorIndex.current=next; onSelect(asset, next, synthetic); }
    virtualizer.scrollToIndex(Math.floor(next / columns), { align: "auto" });
  };

  return <div className="grid-shell">
    <div className="asset-scroll" ref={scrollRef} tabIndex={0} onKeyDown={onKeyDown} onScroll={() => { const now=Date.now();if(now-scrollPulse.current>220){scrollPulse.current=now;onViewport();} }}>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative", width: "100%" }}>
        {virtualRows.map(row => <div key={row.key} className="asset-row" style={{ position: "absolute", top: 0, transform: `translateY(${row.start}px)`, height: rowHeight, gridTemplateColumns: `repeat(${columns}, minmax(0, ${tileWidth}px))` } as CSSProperties}>
          {Array.from({ length: columns }, (_, col) => {
            const index = row.index * columns + col; const asset = getAt(index);
            if (!asset || index >= total) return <div key={col} />;
            return <AssetTile key={`${asset.id}:${epoch}`} asset={asset} size={zoom} epoch={epoch} selected={selectedIds.has(asset.id)} primary={asset.id === primaryId} onViewport={onViewport} onClick={e => void handleTileClick(e,asset,index)} onDoubleClick={() => onOpen(asset, index)} />;
          })}
        </div>)}
      </div>
    </div>
  </div>;
}

function AssetTile({ asset, size, epoch, selected, primary, onViewport, onClick, onDoubleClick }: { asset: Asset; size: number; epoch: number; selected: boolean; primary: boolean; onViewport: () => void; onClick: (event: MouseEvent<HTMLDivElement>) => void; onDoubleClick: () => void }) {
  const [broken, setBroken] = useState(false);
  const [offline, setOffline] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => { setBroken(false); setOffline(false); }, [asset.id, epoch]);
  const ext = asset.ext.toUpperCase();
  const badge = ext === "DNG" ? "DNG" : ["ARW", "CR2", "NEF", "RAF", "ORF", "RW2", "PEF", "ACR", "RAW"].includes(ext) ? "RAW" : ["JPG", "JPEG"].includes(ext) ? "JPG" : ext || "FILE";
  const label = asset.label?.trim();
  return <div className={`asset-tile ${selected ? "selected" : ""} ${primary ? "primary" : ""}`} onClick={onClick} onDoubleClick={onDoubleClick} title={asset.path ?? asset.name}>
    <div className="thumb-wrap" style={{ height: size }}>
      {isImage(asset) && !broken && !offline && asset.path ? <img src={thumbUrl(asset, size, epoch, retry)} loading="lazy" draggable={false} onError={async () => {
        try {
          const state = await invoke<"ready" | "offline" | "stale" | "unavailable">("get_thumbnail_state", { assetId: asset.id, epoch });
          if (state === "stale") { onViewport(); setRetry(v => v + 1); }
          else if (state === "offline") setOffline(true);
          else setBroken(true);
        } catch { setBroken(true); }
      }} /> : <div className="file-placeholder"><span>{asset.isDirectory ? "📁" : offline || asset.offline ? "OFFLINE" : badge}</span>{(offline || asset.offline) && <small>Odpojené</small>}</div>}
      {!asset.isDirectory && <span className="type-badge">{badge}</span>}
      {!!asset.rating && <span className="rating-badge">{"★".repeat(Math.min(5, asset.rating))}</span>}
      {!!asset.tagged && <span className="tagged-badge">✓</span>}
      {label && <span className="label-badge" style={{ backgroundColor: labelColor(label) }}>{label}</span>}
      {!asset.isDirectory && <span className="size-badge">{formatBytes(asset.size)}</span>}
    </div>
    <div className="tile-name">{asset.name}</div>
  </div>;
}
export function thumbUrl(asset: Asset, size: number, epoch: number, retry = 0): string {
  const url = new URL(convertFileSrc(asset.path ?? "", "thumb"));
  url.searchParams.set("id", String(asset.id));
  url.searchParams.set("s", String(size));
  url.searchParams.set("v", String(epoch));
  url.searchParams.set("r", String(retry));
  return url.toString();
}
export function labelColor(label: string): string {
  const colors: Record<string, string> = { red: "#c94c50", "\u010derven\u00e1": "#c94c50", yellow: "#c49b37", "\u017elt\u00e1": "#c49b37", green: "#4b9862", "zelen\u00e1": "#4b9862", blue: "#4e7db7", "modr\u00e1": "#4e7db7", purple: "#8866b4", "fialov\u00e1": "#8866b4" };
  return colors[label.toLowerCase()] ?? "#6b7280";
}

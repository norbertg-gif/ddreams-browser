import { buildAssetFilter } from "../assetFilter";
import { useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent, type PointerEvent as ReactPointerEvent, type Ref } from "react";
import { launchEditor, useEditors, type Editor } from "../editors";
import { useVirtualizer } from "@tanstack/react-virtual";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { createPortal } from "react-dom";
import type { Asset, AssetQuality, CatalogAssetMetadata, FormatTable, Page, View, ViewFilters } from "../types";
import { EXIF_THUMBNAIL_FIELDS, formatKilobytes, formatSlovakDate, type ThumbnailInfoConfig } from "../thumbnailInfo";

const PAGE_SIZE = 120;
const QUALITY_FLAG_TEXT: Record<string,string> = { blur:"Neostrá", over:"Prepálená", under:"Tmavá" };
export type AssetValuePatch = Pick<Asset, "id" | "rating" | "label" | "tagged"> & Partial<Pick<Asset, "editPendingSince">>;
export type AssetGridHandle = {
  applyValues: (patches: ReadonlyMap<number, AssetValuePatch>, reloadListing: boolean) => void;
  reloadListing: () => Promise<void>;
};

type FolderPreview = Pick<Asset, "id" | "path" | "fileModified">;
declare global { interface Window { __ddreamsShift?: boolean } }
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toLocaleString("sk-SK", { maximumFractionDigits: 0 })} KB`;
  const units = [[1024 ** 4, "TB"], [1024 ** 3, "GB"], [1024 ** 2, "MB"]] as const;
  const [divisor, unit] = units.find(([scale]) => bytes >= scale)!;
  return `${(bytes / divisor).toLocaleString("sk-SK", { maximumFractionDigits: 1, minimumFractionDigits: 1 })} ${unit}`;
}
export function formatFor(asset: Asset, formats: FormatTable) { return formats.formats.find(format => format.ext === asset.ext.toLowerCase()); }
export function isImage(asset: Asset, formats: FormatTable): boolean { const group=formatFor(asset,formats)?.group; return group === "image" || group === "raw"; }

type Props = {
  view: View; query: string; sort: string; descending: boolean; zoom: number; epoch: number;
  selectedIds: Set<number>; primaryId: number | null;
  cutIds?: number[];
  deletionSuggestions: ReadonlySet<number>;
  quarantineDeleteDays?: number;
  onQuarantine: (ids: number[]) => void;
  onKeepSuggested: (id: number) => void;
  onSelect: (asset: Asset, index: number, event: MouseEvent | KeyboardEvent) => void;
  onSelectRange: (ids: number[], asset: Asset, index: number, add: boolean) => void;
  onSelectAll: (ids: number[]) => void;
  onOpen: (asset: Asset, index: number) => void;
  onCounts: (total: number, size: number) => void;
  onViewport: () => void;
  filters?: ViewFilters;
  overlayMode?: number;
  hoverMode?: "off"|"hover"|"shift";
  includeChildren: boolean;
  formats: FormatTable;
  thumbnailInfo: ThumbnailInfoConfig;
  onOpenThumbnailInfo: () => void;
  onSetValues: (ids: number[], changes: Record<string, unknown>) => void;
  onFileOperation: (ids:number[], mode:"move"|"copy"|"rename"|"delete"|"delete-permanent", asset?:Asset) => void;
  onFindSimilar: (asset:Asset) => void;
  aiReady: boolean;
  aiReason: string;
  readOnly: boolean;
  uiScale?: number;
  ref?: Ref<AssetGridHandle>;
  refreshToken?: number;
  qualityToken?: number;
};

export default function AssetGrid({ ref, view, query, sort, descending, zoom, epoch, selectedIds, primaryId, cutIds=[], quarantineDeleteDays, onQuarantine, deletionSuggestions, onKeepSuggested, onSelect, onSelectRange, onSelectAll, onOpen, onCounts, onViewport, filters={}, overlayMode=2, hoverMode="shift", includeChildren, refreshToken=0, qualityToken=0, formats, thumbnailInfo, onOpenThumbnailInfo, onSetValues, onFileOperation, onFindSimilar, aiReady, aiReason, readOnly, uiScale=1 }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const groupsById = useMemo(() => {
    const groups = new Map<number, number>();
    if (view.groups) view.ids?.forEach((id, index) => groups.set(id, view.groups![index]));
    return groups;
  }, [view.ids, view.groups]);
  const [shiftDown, setShiftDown] = useState(!!window.__ddreamsShift);
  const dismissTilePopup = useRef<(() => void) | null>(null);
  useEffect(() => {
    const down = (e: globalThis.KeyboardEvent) => { if (e.key === "Shift") setShiftDown(true); };
    const up = (e: globalThis.KeyboardEvent) => { if (e.key === "Shift") setShiftDown(false); };
    const dismiss = () => dismissTilePopup.current?.();
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("ddreams-scroll", dismiss);
    window.addEventListener("resize", dismiss);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("ddreams-scroll", dismiss);
      window.removeEventListener("resize", dismiss);
    };
  }, []);
  const editors = useEditors();
  const cache = useRef(new Map<number, Asset>());
  const valuePatches = useRef(new Map<number, AssetValuePatch>());
  const [, setValuesVersion] = useState(0);
  const storePage = useCallback((items: Asset[], offset: number) => {
    items.forEach((asset, index) => {
      const patch = valuePatches.current.get(asset.id);
      cache.current.set(offset + index, patch ? { ...asset, ...patch } : asset);
    });
  }, []);
  const pages = useRef(new Set<number>());
  const requestVersion = useRef(0);
  const diskReload = useRef<{ version: number; pending: boolean } | null>(null);
  const anchorIndex = useRef(0);
  const [focusIndex, setFocusIndex] = useState<number | null>(null);
  const scrollPulse = useRef(0);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const pendingFetches = useRef(0);
  const loadingTimer = useRef<number | undefined>(undefined);
  const [itemsVersion, setItemsVersion] = useState(0);
  const [columns, setColumns] = useState(5);
  const [tileMetadata, setTileMetadata] = useState(new Map<number, import("../types").TileExifMetadata>());
  const [tileQuality, setTileQuality] = useState(new Map<number, AssetQuality>());
  const qualityCache = useRef(new Map<number, AssetQuality>());
  const qualityRequests = useRef(new Set<number>());
  const folderPreviewCache = useRef(new Map<string, FolderPreview[]>());
  const folderPreviewGeneration = useRef(0);
  const [folderPreviews, setFolderPreviews] = useState(new Map<string, FolderPreview[]>());
  const [rubber,setRubber]=useState<{x1:number;y1:number;x2:number;y2:number;add:boolean}|null>(null);
  const rubberRef=useRef<typeof rubber>(null);
  const wantsCategories = thumbnailInfo.fields.some(field => field.id === "categories");
  const wantsKeywords = thumbnailInfo.fields.some(field => field.id === "keywords");
  const params = useMemo(() => ({
    ...buildAssetFilter(view, query, sort, descending, filters, includeChildren),
    includeCategories: wantsCategories, includeKeywords: wantsKeywords,
  }), [view, query, sort, descending, filters, includeChildren, wantsCategories, wantsKeywords]);
  const paramsKey = JSON.stringify(params);
  const paramsKeyRef = useRef(paramsKey);
  paramsKeyRef.current = paramsKey;
  const tileWidth = zoom + 26;
  const infoLineCount = thumbnailInfo.fields.reduce((sum, field) => sum + field.lines, 0);
  const infoHeight = Math.max(27, 8 + infoLineCount * 14);
  const rowHeight = zoom + 30 + infoHeight;

  const fetchPage = useCallback(async (offset: number, version: number, listingOnly = false, beforeStore?: (page: Page, offset: number) => void) => {
    const reload = diskReload.current?.version === version ? diskReload.current : null;
    if (reload?.pending && !beforeStore) return;
    const pageNo = Math.floor(offset / PAGE_SIZE);
    if (pages.current.has(pageNo)) return;
    pages.current.add(pageNo);
    pendingFetches.current++;
    if (pendingFetches.current === 1) loadingTimer.current = window.setTimeout(() => setLoading(true), 250);
    try {
      const result = await invoke<Page>("get_assets", { filter: { ...params, offset: pageNo * PAGE_SIZE, limit: PAGE_SIZE } });
      if (version !== requestVersion.current) return;
      beforeStore?.(result, pageNo * PAGE_SIZE);
      storePage(result.items, pageNo * PAGE_SIZE);
      setTotal(result.total);
      onCounts(result.total, result.totalSize);
      if (listingOnly) setValuesVersion(v => v + 1);
      else setItemsVersion(v => v + 1);
    } catch (error) {
      if (version === requestVersion.current) pages.current.delete(pageNo);
      console.error("Katalóg sa nepodarilo načítať", error);
    }
    finally {
      if (reload && beforeStore) reload.pending = false;
      pendingFetches.current = Math.max(0, pendingFetches.current - 1);
      if (!pendingFetches.current) { if (loadingTimer.current !== undefined) window.clearTimeout(loadingTimer.current); loadingTimer.current = undefined; setLoading(false); }
    }
  }, [params, onCounts, storePage]);

  useEffect(() => {
    requestVersion.current++;
    cache.current.clear(); pages.current.clear(); valuePatches.current.clear(); setTotal(0); setFocusIndex(null); anchorIndex.current = 0; setItemsVersion(v => v + 1);
    folderPreviewGeneration.current++; folderPreviewCache.current.clear(); setFolderPreviews(new Map());
    setTileMetadata(new Map());
    scrollRef.current?.scrollTo({ top: 0 });
    void fetchPage(0, requestVersion.current);
  }, [paramsKey, fetchPage]);

  const refreshTokenRef = useRef(refreshToken);
  useEffect(() => {
    if (refreshTokenRef.current === refreshToken) return;
    refreshTokenRef.current = refreshToken;
    requestVersion.current++;
    valuePatches.current.clear();
    pages.current.clear();
    folderPreviewGeneration.current++; folderPreviewCache.current.clear(); setFolderPreviews(new Map());
    setItemsVersion(v => v + 1);
    const scroll = scrollRef.current;
    const firstVisible = scroll ? Math.max(0, Math.floor(scroll.scrollTop / rowHeight) * columns) : 0;
    void fetchPage(firstVisible, requestVersion.current);
  }, [refreshToken, fetchPage, rowHeight, columns]);

  useImperativeHandle(ref, () => ({
    reloadListing() {
      const scroll = scrollRef.current;
      const firstVisible = scroll ? Math.max(0, Math.floor(scroll.scrollTop / rowHeight) * columns) : 0;
      const version = ++requestVersion.current;
      diskReload.current = { version, pending: true };
      pages.current.clear();
      return fetchPage(firstVisible, version, false, (page, offset) => {
        // Keep the previous page on-screen until its replacement arrives.
        const previous = cache.current;
        const findIndex = (index: number) => {
          const id = previous.get(index)?.id;
          const next = page.items.findIndex(asset => asset.id === id);
          return next < 0 ? null : offset + next;
        };
        cache.current = new Map();
        setFocusIndex(index => index == null ? null : findIndex(index));
        anchorIndex.current = findIndex(anchorIndex.current) ?? Math.min(anchorIndex.current, Math.max(0, page.total - 1));
        folderPreviewGeneration.current++;
        folderPreviewCache.current.clear();
        setFolderPreviews(new Map());
      });
    },
    applyValues(patches, reloadListing) {
      for (const [id, patch] of patches) {
        const merged = { ...valuePatches.current.get(id), ...patch };
        valuePatches.current.set(id, merged);
        valuePatches.current.set(patch.id, merged);
      }
      for (const [index, asset] of cache.current) {
        const patch = patches.get(asset.id);
        if (patch) cache.current.set(index, { ...asset, ...patch });
      }
      setValuesVersion(version => version + 1);
      if (reloadListing) {
        requestVersion.current++;
        pages.current.clear();
        cache.current.clear();
        const scroll = scrollRef.current;
        const firstVisible = scroll ? Math.max(0, Math.floor(scroll.scrollTop / rowHeight) * columns) : 0;
        void fetchPage(firstVisible, requestVersion.current, true);
      }
    },
  }), [fetchPage, rowHeight, columns]);

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
  useEffect(() => {
    if (!virtualRows.length) return;
    const generation = folderPreviewGeneration.current;
    const start = virtualRows[0].index * columns;
    const end = Math.min(total, (virtualRows[virtualRows.length - 1].index + 1) * columns);
    const requested: string[] = [];
    for (let index = start; index < end; index++) {
      const asset = cache.current.get(index);
      const path = asset?.isDirectory ? asset.path : undefined;
      if (path && !folderPreviewCache.current.has(path) && !requested.includes(path)) requested.push(path);
    }
    if (!requested.length) return;
    const timer = window.setTimeout(() => {
      for (const path of requested) folderPreviewCache.current.set(path, []);
      void invoke<Record<string, FolderPreview[]>>("get_folder_previews", { paths: requested }).then(result => {
        if (generation !== folderPreviewGeneration.current) return;
        for (const path of requested) folderPreviewCache.current.set(path, result[path] ?? []);
        setFolderPreviews(current => {
          if (generation !== folderPreviewGeneration.current) return current;
          const next = new Map(current);
          for (const path of requested) next.set(path, result[path] ?? []);
          return next;
        });
      }).catch(error => console.error("Náhľady priečinkov sa nepodarilo načítať", error));
    }, 70);
    return () => window.clearTimeout(timer);
  }, [virtualRows, columns, total, itemsVersion]);

  const visibleSignature = virtualRows.length ? `${virtualRows[0].index}:${virtualRows[virtualRows.length - 1].index}:${columns}:${itemsVersion}` : "";
  useEffect(()=>{qualityCache.current.clear();qualityRequests.current.clear();setTileQuality(new Map())},[refreshToken]);
  useEffect(()=>{
    if(!virtualRows.length)return;
    const start=virtualRows[0].index*columns,end=Math.min(total,(virtualRows[virtualRows.length-1].index+1)*columns);
    const ids:number[]=[];
    for(let index=start;index<end;index++){const asset=cache.current.get(index);if(asset&&!asset.isDirectory&&asset.id>0&&!qualityCache.current.has(asset.id)&&!qualityRequests.current.has(asset.id))ids.push(asset.id)}
    if(!ids.length)return;
    const timer=window.setTimeout(()=>{
      ids.forEach(id=>qualityRequests.current.add(id));
      void invoke<AssetQuality[]>("get_asset_quality",{assetIds:ids}).then(rows=>{for(const row of rows)qualityCache.current.set(row.assetId,row);setTileQuality(new Map(qualityCache.current))}).catch(()=>{}).finally(()=>ids.forEach(id=>qualityRequests.current.delete(id)));
    },120);
    return()=>window.clearTimeout(timer);
  },[visibleSignature,qualityToken,refreshToken]);
  const exifFields = thumbnailInfo.fields.map(field => field.id).filter(field => EXIF_THUMBNAIL_FIELDS.has(field));
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !visibleSignature || !exifFields.length) return;
    const requestKey = paramsKey;
    const timer = window.setTimeout(() => {
      const first = Math.max(0, Math.floor(el.scrollTop / rowHeight) * columns);
      const last = Math.min(total, Math.ceil((el.scrollTop + el.clientHeight) / rowHeight) * columns);
      const ids: number[] = [];
      for (let index = first; index < last; index++) {
        const asset = getAt(index);
        if (asset && !asset.isDirectory && !asset.offline && asset.path) ids.push(asset.id);
      }
      if (!ids.length) return;
      void invoke<import("../types").TileExifMetadataItem[]>("get_thumbnail_metadata", { assetIds: ids, fields: exifFields }).then(items => {
        if (requestKey !== paramsKeyRef.current) return;
        setTileMetadata(current => { const next = new Map(current); for (const item of items) next.set(item.assetId, item); return next; });
      }).catch(error => console.warn("EXIF údaje pod náhľadmi sa nepodarilo načítať", error));
    }, 180);
    return () => window.clearTimeout(timer);
  }, [visibleSignature, paramsKey, exifFields.join(","), rowHeight, columns, total]);

  const getAt = (index: number) => cache.current.get(index);
  const fetchIds = async (extra: Record<string,unknown> = {}) => { const ids:number[]=[];for(let offset=0;offset<total;offset+=500){const batch=await invoke<number[]>("get_asset_ids",{filter:{...params,...extra,offset:0,limit:500},offset,limit:500});ids.push(...batch);if(batch.length<500)break;}return ids; };
  const fetchAssetsForIds = async (ids:number[]) => {const wanted=new Set(ids),files:number[]=[];for(let offset=0;offset<total&&wanted.size;offset+=500){const p=await invoke<Page>("get_assets",{filter:{...params,offset,limit:500}});for(const a of p.items)if(wanted.has(a.id)){if(!a.isDirectory)files.push(a.id);wanted.delete(a.id)}if(p.items.length<500)break;}return files;};
  const selectRangeTo = async (asset: Asset, index: number, add: boolean) => {
    const version = requestVersion.current;
    const start = Math.min(anchorIndex.current, index), end = Math.max(anchorIndex.current, index);
    const ids: number[] = [];
    for (let offset = Math.floor(start / PAGE_SIZE) * PAGE_SIZE; offset <= end; offset += PAGE_SIZE) {
      const first = Math.max(start, offset), last = Math.min(end, offset + PAGE_SIZE - 1);
      if (Array.from({length: last - first + 1}, (_, i) => getAt(first + i)).some(item => !item)) {
        const page = await invoke<Page>("get_assets", { filter: { ...params, offset, limit: PAGE_SIZE } });
        if (version !== requestVersion.current) return;
        storePage(page.items, offset);
      }
      for (let i = first; i <= last; i++) { const item = getAt(i); if (item) ids.push(item.id); }
    }
    onSelectRange(ids, asset, index, add);
  };
  const handleTileClick = async (event: MouseEvent<HTMLDivElement>, asset: Asset, index: number) => {
    scrollRef.current?.focus({preventScroll:true});
    setFocusIndex(index);
    if (event.shiftKey) await selectRangeTo(asset, index, event.ctrlKey || event.metaKey);
    else { anchorIndex.current = index; onSelect(asset, index, event); }
  };
  const onKeyDown = async (event: KeyboardEvent<HTMLDivElement>) => {
    if (document.activeElement !== event.currentTarget || event.altKey) return;
    if (event.key === "Enter") {
      event.preventDefault();
      // The viewer listens on window and would close again on this same keypress.
      event.stopPropagation();
      const entry = [...cache.current.entries()].find(([, item]) => item.id === primaryId);
      if (entry) onOpen(entry[1], entry[0]);
      return;
    }
    if(event.ctrlKey&&event.shiftKey&&event.key.toLowerCase()==="d"){event.preventDefault();onSelectAll([]);return;}
    if(event.ctrlKey&&event.shiftKey&&event.key.toLowerCase()==="a"){event.preventDefault();const ids=await fetchIds();onSelectAll(await fetchAssetsForIds(ids));return;}
    if(event.ctrlKey&&event.shiftKey&&event.key.toLowerCase()==="i"){event.preventDefault();const ids=await fetchIds();onSelectAll(ids.filter(id=>!selectedIds.has(id)));return;}
    if(event.ctrlKey&&!event.shiftKey&&event.key.toLowerCase()==="i"){event.preventDefault();const ids=await fetchIds({fileGroup:"images"});onSelectAll(await fetchAssetsForIds(ids));return;}
    if (event.ctrlKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      onSelectAll(await fetchIds());
      return;
    }
    const current = focusIndex ?? [...cache.current.entries()].find(([, item]) => item.id === primaryId)?.[0] ?? 0;
    if (event.ctrlKey && event.key === " " && !event.shiftKey) {
      event.preventDefault();
      const asset = getAt(current);
      if (asset) { anchorIndex.current = current; onSelect(asset, current, event); }
      return;
    }
    if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown"].includes(event.key) || !total) return;
    event.preventDefault();
    const pageRows = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? rowHeight) / rowHeight));
    const delta = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : event.key === "ArrowUp" ? -columns : event.key === "ArrowDown" ? columns : event.key === "PageUp" ? -columns * pageRows : columns * pageRows;
    const next = event.key === "Home" ? 0 : event.key === "End" ? total - 1 : Math.max(0, Math.min(total - 1, current + delta));
    const ctrl = event.ctrlKey || event.metaKey, shift = event.shiftKey;
    const version = requestVersion.current;
    if (!getAt(next)) {
      const offset = Math.floor(next / PAGE_SIZE) * PAGE_SIZE;
      const page = await invoke<Page>("get_assets", {filter: {...params, offset, limit: PAGE_SIZE}});
      if (version !== requestVersion.current) return;
      storePage(page.items, offset);
      setItemsVersion(v => v + 1);
    }
    const asset = getAt(next); if (!asset) return;
    setFocusIndex(next);
    if (shift) await selectRangeTo(asset, next, ctrl);
    else if (!ctrl) { anchorIndex.current = next; onSelect(asset, next, event); }
    virtualizer.scrollToIndex(Math.floor(next / columns), { align: "auto" });
  };
  const beginRubber=(e:ReactPointerEvent<HTMLDivElement>)=>{if(e.button!==0||(e.target as HTMLElement).closest(".asset-tile,.tile-context-menu"))return;const r=e.currentTarget.getBoundingClientRect();const v={x1:e.clientX-r.left,y1:e.clientY-r.top,x2:e.clientX-r.left,y2:e.clientY-r.top,add:e.ctrlKey||e.metaKey};rubberRef.current=v;setRubber(v);e.currentTarget.setPointerCapture(e.pointerId);e.preventDefault()};
  const moveRubber=(e:ReactPointerEvent<HTMLDivElement>)=>{if(!rubberRef.current)return;const r=e.currentTarget.getBoundingClientRect();const v={...rubberRef.current,x2:e.clientX-r.left,y2:e.clientY-r.top};rubberRef.current=v;setRubber(v);if(v.y2<30)e.currentTarget.scrollTop-=22;else if(v.y2>r.height-30)e.currentTarget.scrollTop+=22};
  const endRubber=async()=>{const v=rubberRef.current;if(!v)return;rubberRef.current=null;setRubber(null);const el=scrollRef.current;if(!el)return;if(Math.hypot(v.x2-v.x1,v.y2-v.y1)<4){onSelectAll([]);return;}const xlo=Math.min(v.x1,v.x2),xhi=Math.max(v.x1,v.x2),ylo=Math.min(v.y1,v.y2)+el.scrollTop,yhi=Math.max(v.y1,v.y2)+el.scrollTop;const indexes:number[]=[];for(let row=Math.max(0,Math.floor((ylo-10)/rowHeight));row<=Math.min(rowCount-1,Math.floor((yhi-10)/rowHeight));row++){const cy=row*rowHeight+10+Math.min(zoom,rowHeight-24)/2;if(cy<ylo||cy>yhi)continue;for(let col=0;col<columns;col++){const cx=12+col*(tileWidth+9)+tileWidth/2;if(cx>=xlo&&cx<=xhi){const i=row*columns+col;if(i<total)indexes.push(i)}}}const needed=[...new Set(indexes.map(i=>Math.floor(i/PAGE_SIZE)*PAGE_SIZE))];for(const offset of needed){const p=await invoke<Page>("get_assets",{filter:{...params,offset,limit:PAGE_SIZE}});storePage(p.items,offset);}const ids=indexes.map(i=>getAt(i)?.id).filter((id):id is number=>id!=null);onSelectAll(v.add?[...new Set([...selectedIds,...ids])]:ids);};
  useEffect(()=>{if(!rubber)return;const timer=window.setInterval(()=>{const el=scrollRef.current,v=rubberRef.current;if(!el||!v)return;if(v.y2<30)el.scrollTop-=14;else if(v.y2>el.clientHeight-30)el.scrollTop+=14},24);return()=>window.clearInterval(timer)},[!!rubber]);

  return <div className="grid-shell">
    {loading && <div className="grid-loading-bar" aria-label="Načítavam" />}
    <div className="asset-scroll" ref={scrollRef} tabIndex={0} onMouseDown={e=>{if((e.target as HTMLElement).closest(".asset-tile"))e.currentTarget.focus({preventScroll:true})}} onPointerDown={beginRubber} onPointerMove={moveRubber} onPointerUp={()=>void endRubber()} onPointerCancel={()=>void endRubber()} onWheel={e=>{if(e.ctrlKey){e.preventDefault();window.dispatchEvent(new CustomEvent("ddreams-zoom",{detail:e.deltaY>0?-8:8}))}}} onKeyDown={onKeyDown} onScroll={e => { window.dispatchEvent(new Event("ddreams-scroll"));const top=(e.currentTarget as HTMLElement).scrollTop,page=(e.currentTarget as HTMLElement).clientHeight;if(Math.abs(top-scrollPulse.current)>page*3){scrollPulse.current=top;onViewport();} }}>
      {rubber&&<div className="rubber-band" style={{left:Math.min(rubber.x1,rubber.x2),top:Math.min(rubber.y1,rubber.y2),width:Math.abs(rubber.x1-rubber.x2),height:Math.abs(rubber.y1-rubber.y2)}}/>}
      <div style={{ height: virtualizer.getTotalSize(), position: "relative", width: "100%" }}>
        {virtualRows.map(row => <div key={row.key} className="asset-row" style={{ position: "absolute", top: 0, transform: `translateY(${row.start}px)`, height: rowHeight, gridTemplateColumns: `repeat(${columns}, minmax(0, ${tileWidth}px))` } as CSSProperties}>
          {Array.from({ length: columns }, (_, col) => {
            const index = row.index * columns + col; const asset = getAt(index);
            if (!asset || index >= total) return <div key={col} />;
            return <AssetTile key={asset.id} asset={asset} editors={editors} quarantineDeleteDays={quarantineDeleteDays} onQuarantine={onQuarantine} deletionSuggested={deletionSuggestions.has(asset.id)} onKeepSuggested={onKeepSuggested} folderPreviews={asset.path ? folderPreviews.get(asset.path) ?? [] : []} groupNumber={groupsById.get(asset.id)} best={view.kind==="series"&&view.bestIds?.includes(asset.id)} size={zoom} epoch={epoch} selected={selectedIds.has(asset.id)} primary={asset.id === primaryId} focused={focusIndex === null ? asset.id === primaryId : index === focusIndex} cut={cutIds.includes(asset.id)} selectedIds={selectedIds} shiftDown={shiftDown} dismissTilePopup={dismissTilePopup} overlayMode={overlayMode} hoverMode={hoverMode} formats={formats} infoFields={thumbnailInfo.fields} tileMetadata={tileMetadata.get(asset.id)} quality={tileQuality.get(asset.id)} onOpenThumbnailInfo={onOpenThumbnailInfo} onSetValues={onSetValues} onFileOperation={onFileOperation} onFindSimilar={onFindSimilar} aiReady={aiReady} aiReason={aiReason} readOnly={readOnly} uiScale={uiScale} onClick={e => void handleTileClick(e,asset,index)} onContextMenu={() => { if (!selectedIds.has(asset.id)) { anchorIndex.current=index; setFocusIndex(index); onSelect(asset,index,{ctrlKey:false,metaKey:false,shiftKey:false} as MouseEvent); } scrollRef.current?.focus({preventScroll:true}); }} onDoubleClick={() => onOpen(asset, index)} />;
          })}
        </div>)}
      </div>
    </div>
  </div>;
}

function AssetTile({ asset, editors, quarantineDeleteDays, onQuarantine, deletionSuggested, onKeepSuggested, folderPreviews, groupNumber, best, size, epoch, selected, primary, focused, cut, onClick, onContextMenu, onDoubleClick, overlayMode, hoverMode, selectedIds, shiftDown, dismissTilePopup, formats, infoFields, tileMetadata, quality, onOpenThumbnailInfo, onSetValues, onFileOperation, onFindSimilar, aiReady, aiReason, readOnly, uiScale }: { asset: Asset; editors: Editor[]; quarantineDeleteDays?: number; onQuarantine: (ids: number[]) => void; deletionSuggested: boolean; onKeepSuggested: (id: number) => void; folderPreviews: FolderPreview[]; groupNumber?:number; best?:boolean; size: number; epoch: number; selected: boolean; primary: boolean; focused: boolean; cut:boolean; selectedIds:ReadonlySet<number>; shiftDown:boolean; dismissTilePopup:{current:(()=>void)|null}; onClick: (event: MouseEvent<HTMLDivElement>) => void; onContextMenu: () => void; onDoubleClick: () => void; overlayMode:number; hoverMode:"off"|"hover"|"shift"; formats: FormatTable; infoFields: ThumbnailInfoConfig["fields"]; tileMetadata?: import("../types").TileExifMetadata; quality?: AssetQuality; onOpenThumbnailInfo: () => void; onSetValues:(ids:number[],changes:Record<string,unknown>)=>void; onFileOperation:(ids:number[],mode:"move"|"copy"|"rename"|"delete"|"delete-permanent",asset?:Asset)=>void; onFindSimilar:(asset:Asset)=>void; aiReady:boolean; aiReason:string; readOnly:boolean; uiScale:number }) {
  // The epoch a tile was mounted with stays in its thumbnail URL, so later epoch bumps (far scroll, new
  // view) do not reload thumbnails that are already on screen; only a "stale" answer picks up the new one.
  const [tileEpoch,setTileEpoch]=useState(epoch); const latestEpoch=useRef(epoch); latestEpoch.current=epoch;
  const [broken, setBroken] = useState(false);
  const [offline, setOffline] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => { setBroken(false); setOffline(false); }, [asset.id, asset.fileModified, asset.size, epoch]);
  const ext = asset.ext.toUpperCase();
  const format = formatFor(asset, formats);
  const badge = format?.group === "raw" ? ext.toUpperCase() : ext || "FILE";
  const label = asset.label?.trim();
  const qualityReasons=quality?.flags.map(flag=>QUALITY_FLAG_TEXT[flag]??flag)??[];
  const rejected=label?.toLowerCase()==="gray";
  const hasSizeLine = infoFields.some(field => field.id === "sizeKb" || field.id === "sizeAuto");
  const displayValue = (id: string): string => {
    const name = asset.name;
    switch (id) {
      case "name": return name;
      case "baseName": return asset.ext && name.toLowerCase().endsWith(`.${asset.ext.toLowerCase()}`) ? name.slice(0, -(asset.ext.length + 1)) : name;
      case "type": return format?.name ?? (ext || "FILE");
      case "sizeKb": return formatKilobytes(asset.size);
      case "sizeAuto": return formatBytes(asset.size);
      case "folder": { const parts = asset.path?.replace(/[\\/]+$/, "").split(/[\\/]/).slice(0, -1) ?? []; return parts[parts.length - 1] ?? ""; }
      case "modified": return formatSlovakDate(asset.fileModified?.replace(/\.\d+Z$/, "Z"));
      case "resolution": return asset.width && asset.height ? `${asset.width} × ${asset.height}` : "";
      case "dateTaken": return formatSlovakDate(asset.exifDate ?? tileMetadata?.dateTaken);
      case "rating": return asset.rating ? `${asset.rating} / 5` : "";
      case "caption": return asset.caption ?? "";
      case "author": return asset.author ?? "";
      case "categories": return asset.displayCategories ?? "";
      case "keywords": return asset.displayKeywords ?? "";
      case "iso": return tileMetadata?.iso ? `ISO ${tileMetadata.iso}` : "";
      case "aperture": return tileMetadata?.aperture ?? "";
      case "shutter": return tileMetadata?.shutter ?? "";
      case "focalLength": return tileMetadata?.focalLength ?? "";
      case "lens": return tileMetadata?.lens ?? "";
      case "camera": return tileMetadata?.camera ?? "";
      default: return "";
    }
  };
  const [menu,setMenu]=useState<{x:number;y:number}|null>(null);
  const menuRef=useRef<HTMLDivElement>(null);
  const [menuPosition,setMenuPosition]=useState<{left:number;top:number}|null>(null);
  const [hoverMetadata,setHoverMetadata]=useState<CatalogAssetMetadata|null>(null);
  // Only one tile menu may be open: any press outside it, Escape, scrolling or leaving the window closes it.
  useEffect(()=>{if(!menu)return;const close=()=>setMenu(null);const down=(e:PointerEvent)=>{if(!menuRef.current?.contains(e.target as Node))close()};const key=(e:globalThis.KeyboardEvent)=>{if(e.key==="Escape"){e.stopPropagation();close()}};const scroll=(e:Event)=>{if(!menuRef.current?.contains(e.target as Node))close()};window.addEventListener("pointerdown",down,true);window.addEventListener("keydown",key,true);window.addEventListener("scroll",scroll,true);window.addEventListener("blur",close);return()=>{window.removeEventListener("pointerdown",down,true);window.removeEventListener("keydown",key,true);window.removeEventListener("scroll",scroll,true);window.removeEventListener("blur",close)}},[menu]);
  useLayoutEffect(()=>{if(!menu)return;const element=menuRef.current;if(!element)return;const pad=8,rect=element.getBoundingClientRect(),vw=window.innerWidth/uiScale,vh=window.innerHeight/uiScale;setMenuPosition({left:Math.max(pad,Math.min(menu.x,Math.max(pad,vw-rect.width-pad))),top:Math.max(pad,Math.min(menu.y,Math.max(pad,vh-rect.height-pad)))})},[menu,uiScale]);
  const [hover,setHover]=useState(false);const [popup,setPopup]=useState(false);const timer=useRef<number|undefined>(undefined);
  const tileRef=useRef<HTMLDivElement>(null);const popupRef=useRef<HTMLDivElement>(null);
  const [popupPosition,setPopupPosition]=useState<{left:number;top:number;maxHeight:number}|null>(null);
  useEffect(()=>{if(!hover||hoverMode==="off"||(hoverMode==="shift"&&!shiftDown)){setPopup(false);return;}timer.current=window.setTimeout(()=>setPopup(true),500);return()=>{if(timer.current!==undefined)window.clearTimeout(timer.current)}},[hover,hoverMode,shiftDown]);
  useLayoutEffect(()=>{if(!popup)return;const tile=tileRef.current?.getBoundingClientRect();const card=popupRef.current;if(!tile||!card)return;const gap=8,pad=8,w=card.offsetWidth,h=card.offsetHeight,vw=window.innerWidth,vh=window.innerHeight;const candidates=[{left:tile.right+gap,top:tile.top},{left:tile.left-w-gap,top:tile.top},{left:tile.left,top:tile.bottom+gap},{left:tile.left,top:tile.top-h-gap}];const fits=candidates.find(p=>p.left>=pad&&p.top>=pad&&p.left+w<=vw-pad&&p.top+h<=vh-pad&&!(p.left<tile.right&&p.left+w>tile.left&&p.top<tile.bottom&&p.top+h>tile.top));const chosen=fits??{left:Math.min(Math.max(pad,tile.right+gap),Math.max(pad,vw-w-pad)),top:Math.min(Math.max(pad,tile.top),Math.max(pad,vh-Math.min(h,vh-pad*2)-pad))};setPopupPosition({left:chosen.left,top:chosen.top,maxHeight:Math.max(120,vh-pad*2)});},[popup,hoverMetadata]);
  useEffect(()=>{if(!popup)return;let current=true;void invoke<CatalogAssetMetadata|null>("get_catalog_asset_metadata",{assetId:asset.id}).then(m=>{if(current)setHoverMetadata(m)}).catch(()=>{});return()=>{current=false}},[popup,asset.id]);
  useEffect(() => {
    if (!popup) return;
    const hide = () => setPopup(false);
    dismissTilePopup.current = hide;
    return () => { if (dismissTilePopup.current === hide) dismissTilePopup.current = null; };
  }, [popup, dismissTilePopup]);
  const daysLeft = asset.quarantinedAt == null || !quarantineDeleteDays ? null
    : Math.max(0, Math.ceil((asset.quarantinedAt + quarantineDeleteDays * 86400 - Date.now() / 1000) / 86400));
  const actionIds = () => selected && selectedIds.size ? [...selectedIds] : [asset.id];
  return <div ref={tileRef} draggable={!asset.isDirectory} onDragStart={e=>{if(asset.isDirectory){e.preventDefault();return;}const ids=actionIds();e.dataTransfer.setData("application/x-ddreams-asset-ids",JSON.stringify(ids));e.dataTransfer.effectAllowed="copyMove";const count=ids.length,noun=count===1?"súbor":count<=4?"súbory":"súborov",dragImage=document.createElement("div");dragImage.textContent=`${count} ${noun}`;Object.assign(dragImage.style,{position:"fixed",left:"-1000px",top:"-1000px",padding:"3px 8px",borderRadius:"10px",background:"#273644",color:"#d9dfe8",fontSize:"11px",lineHeight:"14px",border:"1px solid #465461",whiteSpace:"nowrap",pointerEvents:"none"});document.body.appendChild(dragImage);// Negative offsets place the image down and right of the cursor hotspot.
  e.dataTransfer.setDragImage(dragImage,-14,-14);window.setTimeout(()=>dragImage.remove(),0);}} className={`asset-tile overlay-${overlayMode} ${selected ? "selected" : ""} ${primary ? "primary" : ""} ${focused ? "focused" : ""} ${cut?"cut-pending":""} ${rejected?"rejected":""} ${groupNumber!=null?`duplicate-group-${groupNumber%2}`:""}`} onContextMenu={e=>{e.preventDefault();onContextMenu();setMenu({x:e.clientX,y:e.clientY})}} onMouseEnter={()=>setHover(true)} onMouseLeave={()=>{setHover(false);setPopup(false)}} onClick={onClick} onDoubleClick={onDoubleClick} title={asset.path ?? asset.name}>{groupNumber!=null&&<span className="duplicate-group-badge">#{groupNumber}</span>}
    <div className="thumb-wrap" style={{ height: size }}>
      {asset.quarantinedAt != null && <span className="quarantine-days">
        {quarantineDeleteDays === 0 ? "Plánované mazanie vypnuté" : daysLeft != null ? `Do kontroly zmazania: ${daysLeft} dní` : null}
      </span>}
      {deletionSuggested && (readOnly
        ? <span className="deletion-suggestion-badge" title="Návrh na vymazanie">✕</span>
        : <button type="button" className="deletion-suggestion-badge"
            aria-label="Odmietnuť návrh na vymazanie"
            title="Odmietnuť návrh na vymazanie – túto fotku už nenavrhovať"
            onClick={event=>{event.stopPropagation();onKeepSuggested(asset.id);}}
            onDoubleClick={event=>event.stopPropagation()}>✕</button>)}
      {best&&<span className="best-badge" title="Najostrejšia v sérii">★ najlepšia</span>}
      {!asset.isDirectory && !broken && !offline && asset.path ? <img src={thumbUrl(asset, size, tileEpoch, retry)} loading="lazy" draggable={false} onError={async () => {
        try {
          const current = latestEpoch.current;
          const state = await invoke<"ready" | "offline" | "stale" | "unavailable">("get_thumbnail_state", { assetId: asset.id, epoch: current });
          if (state === "stale" || (state === "ready" && retry < 2)) { setTileEpoch(latestEpoch.current); setRetry(v => v + 1); }
          else if (state === "offline") setOffline(true);
          else setBroken(true);
        } catch { setBroken(true); }
      }} /> : asset.isDirectory ? <div className="folder-tile"><div className="folder-tab"/><div className="folder-body">{folderPreviews.slice(0, 4).map(preview => <img key={preview.id} className="folder-preview" src={thumbUrl(preview, 160, tileEpoch)} loading="lazy" draggable={false} onError={event => { event.currentTarget.style.display = "none"; }} />)}</div></div> : <div className="file-placeholder"><span>{asset.missing ? "CHÝBA" : offline || asset.offline ? "OFFLINE" : badge}</span>{asset.missing ? <small>Súbor neexistuje</small> : (offline || asset.offline) && <small>Odpojené</small>}</div>}
      {overlayMode>0&&!asset.isDirectory && <span className="type-badge" title={`Typ súboru: ${badge}`}>{badge}</span>}
      {overlayMode>0&&(asset.hasCategories||!!asset.rating||asset.editPendingSince!=null)&&<span className="tile-badges-left">{asset.editPendingSince!=null&&<span className="edit-pending-badge" title={`Na úpravu od ${new Intl.DateTimeFormat("sk-SK").format(new Date(asset.editPendingSince*1000)).replace(/\s/g,"")}`}>✎</span>}{asset.hasCategories&&<CategoryBadge assetId={asset.id}/>}{!!asset.rating&&<span className="rating-badge" title={`Hodnotenie: ${Math.min(5, asset.rating)} z 5`}>{"★".repeat(Math.min(5, asset.rating))}</span>}</span>}
      {overlayMode>0&&!!asset.tagged && <span className="tagged-badge" title="Označené">✓</span>}
      
      {overlayMode>0&&label && <span className="label-badge" title={`Farebný štítok: ${labelText(label)}`} style={{ backgroundColor: labelColor(label) }} />}
      {overlayMode>0&&qualityReasons.length>0&&<span className="quality-badge" title={qualityReasons.join(", ")}>!</span>}
      {overlayMode>0&&(asset.missing||offline||asset.offline)&&<span className={asset.missing?"missing-badge":"offline-badge"} title={asset.missing?"Súbor na pripojenom disku neexistuje":"Disk nie je pripojený"}>{asset.missing?"CHÝBA":"OFFLINE"}</span>}
      {!asset.isDirectory && !hasSizeLine && <span className="size-badge">{formatBytes(asset.size)}</span>}
    </div>
    <div className="tile-info-lines">{(asset.isDirectory ? [{id:"name",lines:1 as const}] : infoFields).map((field,index) => { const text=displayValue(field.id); return <div key={`${field.id}:${index}`} className={`tile-info-line ${field.id === "name" ? "tile-name" : ""}`} style={{ WebkitLineClamp: field.lines, maxHeight: field.lines * 14 }} title={text}>{text || "\u00a0"}</div>; })}</div>
    {menu&&createPortal(<div ref={menuRef} className="tile-context-menu" style={{left:menuPosition?.left??menu.x,top:menuPosition?.top??menu.y}} onPointerDown={e=>e.stopPropagation()} onMouseDown={e=>e.stopPropagation()} onClick={e=>e.stopPropagation()} onDoubleClick={e=>e.stopPropagation()} onContextMenu={e=>{e.preventDefault();e.stopPropagation()}} onMouseLeave={()=>setMenu(null)}><button onClick={()=>{setMenu(null);onDoubleClick()}}>Otvoriť</button><button disabled={asset.offline||offline} onClick={()=>{setMenu(null);void invoke("asset_action",{assetId:asset.id,action:"open"})}}>Otvoriť v predvolenom programe</button>{!asset.isDirectory&&editors.map((editor,index)=><button key={editor.path} disabled={asset.offline||offline} title={editor.path} onClick={()=>{setMenu(null);void launchEditor(editor, actionIds()).catch(error=>window.alert(String(error)))}}>Otvoriť v: {editor.name}{index===0?" (predvolený)":""}</button>)}<button disabled={asset.offline||offline} onClick={()=>{setMenu(null);void invoke("asset_action",{assetId:asset.id,action:"reveal"})}}>Ukázať v Prieskumníkovi</button><button onClick={async()=>{setMenu(null);const ids=actionIds();const paths=await invoke<string[]>("get_asset_paths",{ids});await navigator.clipboard.writeText(paths.join("\n"))}}>Kopírovať cestu</button><button onClick={()=>{setMenu(null);onOpenThumbnailInfo()}}>Údaje pod náhľadmi…</button><hr className="menu-separator"/><div className={`write-menu-group ${readOnly?"readonly":""}`}><div className="write-menu-row"><span>Hodnotenie</span>{[1,2,3,4,5].map(value=><button key={value} title={`Ctrl+${value}`} disabled={readOnly} onClick={()=>{onSetValues(actionIds(),{rating:value});setMenu(null)}}>★{value}</button>)}<button title="Ctrl+0" disabled={readOnly} onClick={()=>{onSetValues(actionIds(),{rating:null});setMenu(null)}}>×</button></div><div className="write-menu-row"><span>Farebný štítok</span>{[["Red","Červená"],["Yellow","Žltá"],["Green","Zelená"],["Blue","Modrá"],["Purple","Fialová"],["Gray","Na vyradenie"]].map(([value,label],index)=><button key={value} title={`${label} (Alt+${index+1})`} disabled={readOnly} onClick={()=>{onSetValues(actionIds(),{label:value});setMenu(null)}}><span className="write-menu-dot" style={{backgroundColor:value.toLowerCase()==="gray"?"#8a8f98":value.toLowerCase()}} /></button>)}<button title="Alt+0" disabled={readOnly} onClick={()=>{onSetValues(actionIds(),{label:null});setMenu(null)}}>×</button></div><button title="\\" disabled={readOnly} onClick={()=>{onSetValues(actionIds(),{tagged:"toggle"});setMenu(null)}}>Označiť / Zrušiť označenie (\)</button>{!asset.isDirectory&&isImage(asset,formats)&&asset.quarantinedAt==null&&<button title="U" disabled={readOnly} onClick={()=>{onSetValues(actionIds(),{editPending:"toggle"});setMenu(null)}}>Na úpravu (U)</button>}</div><hr className="menu-separator"/><button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onFileOperation(actionIds(),"move")}}>Presunúť do…</button>{!asset.isDirectory&&isImage(asset,formats)&&asset.quarantinedAt==null&&<button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onQuarantine(actionIds())}}>Do karantény</button>}<button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onFileOperation(actionIds(),"copy")}}>Kopírovať do…</button><button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onFileOperation([asset.id],"rename",asset)}}>Premenovať (F2)</button><hr className="menu-separator"/>{asset.id>0&&!asset.isDirectory&&<button disabled={!aiReady} title={!aiReady?aiReason:""} onClick={()=>{setMenu(null);onFindSimilar(asset)}}>Nájsť podobné</button>}<hr className="menu-separator"/><hr className="menu-separator"/><button disabled={readOnly||(asset.isDirectory&&(!asset.path||asset.offline||offline))} onClick={()=>{setMenu(null);onFileOperation(asset.isDirectory?[asset.id]:actionIds(),"delete",asset)}}>Odstrániť do Koša… (Del)</button><button className="menu-danger" disabled={readOnly||asset.isDirectory} onClick={()=>{setMenu(null);onFileOperation(actionIds(),"delete-permanent",asset)}}>Zmazať natrvalo… (Shift+Del)</button></div>,document.body)}
    {popup && createPortal(
      <div ref={popupRef} className="hover-card" style={{ position: "fixed", left: popupPosition?.left ?? -10000, top: popupPosition?.top ?? -10000, maxHeight: popupPosition?.maxHeight ?? "80vh", overflowY: "auto" }}>
        <div className="hover-image">{!asset.isDirectory && asset.path && <img src={thumbUrl(asset, 320, epoch)} />}</div>
        <b title={asset.name}>{asset.name}</b>
        <small>{formatBytes(asset.size)} {"\u00b7"} {asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}</small>
        <small>{asset.exifDate ?? asset.fileModified ?? "—"}</small>
        <small>{asset.rating ? `★ ${asset.rating}` : "Bez hodnotenia"} {label ? labelText(label) : ""}</small>
        <small>{hoverMetadata?.categories.length ? `Kategórie: ${hoverMetadata.categories.join(", ")}` : "Bez kategórií"}</small>
      </div>, document.body
    )}
  </div>;
}
export function thumbUrl(asset: Pick<Asset, "id" | "path" | "fileModified"> & Partial<Pick<Asset, "size">>, size: number, epoch: number, retry = 0): string {
  const url = new URL(convertFileSrc(asset.path ?? "", "thumb"));
  url.searchParams.set("id", String(asset.id));
  url.searchParams.set("s", String(size));
  url.searchParams.set("v", String(epoch));
  url.searchParams.set("r", String(retry));
  if (asset.fileModified) url.searchParams.set("m", asset.fileModified);
  if (asset.size != null) url.searchParams.set("bytes", String(asset.size));
  return url.toString();
}
export function labelText(label: string): string {
  const names: Record<string,string> = { red:"Červená", yellow:"Žltá", green:"Zelená", blue:"Modrá", purple:"Fialová", gray:"Na vyradenie", "červená":"Červená", "žltá":"Žltá", "zelená":"Zelená", "modrá":"Modrá", "fialová":"Fialová" };
  return names[label.toLocaleLowerCase("sk-SK")] ?? label;
}
export function labelColor(label: string): string {
  const colors: Record<string, string> = { red: "#c94c50", "červená": "#c94c50", yellow: "#c49b37", "žltá": "#c49b37", green: "#4b9862", "zelená": "#4b9862", blue: "#4e7db7", "modrá": "#4e7db7", purple: "#8866b4", "fialová": "#8866b4", gray: "#8a8f98" };
  return colors[label.toLowerCase()] ?? "#6b7280";
}

/** Category badge on a tile; hovering shows the categories the photo belongs to. */
function CategoryBadge({ assetId }: { assetId: number }) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const [paths, setPaths] = useState<string[] | null>(null);
  const show = (event: MouseEvent<HTMLSpanElement>) => {
    setAnchor(event.currentTarget.getBoundingClientRect());
    if (paths !== null || assetId <= 0) return;
    void invoke<{ categoryPaths: { id: number; path: string }[] } | null>("get_catalog_asset_links", { assetId })
      .then(links => setPaths((links?.categoryPaths ?? []).map(item => item.path)))
      .catch(() => setPaths([]));
  };
  return <span className="category-badge" onMouseEnter={show} onMouseLeave={() => setAnchor(null)}>◆
    {anchor && createPortal(<div className="category-badge-popup" role="tooltip"
      style={{ left: Math.min(anchor.left, window.innerWidth - 300), top: anchor.bottom + 6 }}>
      <strong>Zaradené v kategóriách</strong>
      {paths === null ? <small>Načítavam…</small> : paths.length ? paths.map(path => <span key={path}>{path}</span>) : <small>Žiadna kategória</small>}
    </div>, document.body)}
  </span>;
}

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { useEditors } from "../editors";
import { useVirtualizer } from "@tanstack/react-virtual";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { createPortal } from "react-dom";
import type { Asset, CatalogAssetMetadata, FormatTable, Page, View, ViewFilters } from "../types";
import { EXIF_THUMBNAIL_FIELDS, formatKilobytes, formatSlovakDate, type ThumbnailInfoConfig } from "../thumbnailInfo";

const PAGE_SIZE = 120;
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
  onSelect: (asset: Asset, index: number, event: MouseEvent | KeyboardEvent) => void;
  onSelectRange: (ids: number[], asset: Asset, index: number) => void;
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
  refreshToken?: number;
};

export default function AssetGrid({ view, query, sort, descending, zoom, epoch, selectedIds, primaryId, cutIds=[], onSelect, onSelectRange, onSelectAll, onOpen, onCounts, onViewport, filters={}, overlayMode=2, hoverMode="shift", includeChildren, refreshToken=0, formats, thumbnailInfo, onOpenThumbnailInfo, onSetValues, onFileOperation, onFindSimilar, aiReady, aiReason, readOnly, uiScale=1 }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<number, Asset>());
  const pages = useRef(new Set<number>());
  const requestVersion = useRef(0);
  const anchorIndex = useRef(0);
  const scrollPulse = useRef(0);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const pendingFetches = useRef(0);
  const loadingTimer = useRef<number | undefined>(undefined);
  const [itemsVersion, setItemsVersion] = useState(0);
  const [columns, setColumns] = useState(5);
  const [tileMetadata, setTileMetadata] = useState(new Map<number, import("../types").TileExifMetadata>());
  const folderPreviewCache = useRef(new Map<string, FolderPreview[]>());
  const folderPreviewGeneration = useRef(0);
  const [folderPreviews, setFolderPreviews] = useState(new Map<string, FolderPreview[]>());
  const [rubber,setRubber]=useState<{x1:number;y1:number;x2:number;y2:number;add:boolean}|null>(null);
  const rubberRef=useRef<typeof rubber>(null);
  const wantsCategories = thumbnailInfo.fields.some(field => field.id === "categories");
  const wantsKeywords = thumbnailInfo.fields.some(field => field.id === "keywords");
  const params = useMemo(() => ({ kind: view.kind, id: view.id ?? null, rootId: view.rootId ?? null, includeChildren: (view.kind === "category" || view.kind === "keyword") && includeChildren, ids: view.ids ?? null, folderPath: view.folderPath ?? null, value: view.value ?? null, dateFrom:view.dateFrom??null,dateTo:view.dateTo??null,dateSource:view.dateSource??null,dateNoDate:view.dateNoDate??false, query, sort, descending, ratingMode:filters.ratingMode??null,ratingValue:filters.ratingValue??null,labelFilter:filters.labelFilter??null,taggedOnly:filters.taggedOnly??false,fileGroup:filters.fileGroup??null,includeCategories:wantsCategories,includeKeywords:wantsKeywords }), [view, query, sort, descending, filters, includeChildren, wantsCategories, wantsKeywords]);
  const paramsKey = JSON.stringify(params);
  const paramsKeyRef = useRef(paramsKey);
  paramsKeyRef.current = paramsKey;
  const tileWidth = zoom + 26;
  const infoLineCount = thumbnailInfo.fields.reduce((sum, field) => sum + field.lines, 0);
  const infoHeight = Math.max(27, 8 + infoLineCount * 14);
  const rowHeight = zoom + 30 + infoHeight;

  const fetchPage = useCallback(async (offset: number, version: number) => {
    const pageNo = Math.floor(offset / PAGE_SIZE);
    if (pages.current.has(pageNo)) return;
    pages.current.add(pageNo);
    pendingFetches.current++;
    if (pendingFetches.current === 1) loadingTimer.current = window.setTimeout(() => setLoading(true), 250);
    try {
      const result = await invoke<Page>("get_assets", { filter: { ...params, offset: pageNo * PAGE_SIZE, limit: PAGE_SIZE } });
      if (version !== requestVersion.current) return;
      result.items.forEach((asset, i) => cache.current.set(pageNo * PAGE_SIZE + i, asset));
      setTotal(result.total); onCounts(result.total, result.totalSize); setItemsVersion(v => v + 1);
    } catch (error) { pages.current.delete(pageNo); console.error("Katalóg sa nepodarilo načítať", error); }
    finally { pendingFetches.current = Math.max(0, pendingFetches.current - 1); if (!pendingFetches.current) { if (loadingTimer.current !== undefined) window.clearTimeout(loadingTimer.current); loadingTimer.current = undefined; setLoading(false); } }
  }, [params, onCounts]);

  useEffect(() => {
    requestVersion.current++;
    cache.current.clear(); pages.current.clear(); setTotal(0); setItemsVersion(v => v + 1);
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
    pages.current.clear();
    folderPreviewGeneration.current++; folderPreviewCache.current.clear(); setFolderPreviews(new Map());
    setItemsVersion(v => v + 1);
    const scroll = scrollRef.current;
    const firstVisible = scroll ? Math.max(0, Math.floor(scroll.scrollTop / rowHeight) * columns) : 0;
    void fetchPage(firstVisible, requestVersion.current);
  }, [refreshToken, fetchPage, rowHeight, columns]);

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
        const next = new Map(folderPreviews);
        for (const path of requested) {
          const previews = result[path] ?? [];
          folderPreviewCache.current.set(path, previews);
          next.set(path, previews);
        }
        setFolderPreviews(next);
      }).catch(error => console.error("Náhľady priečinkov sa nepodarilo načítať", error));
    }, 70);
    return () => window.clearTimeout(timer);
  }, [virtualRows, columns, total, itemsVersion, folderPreviews]);

  const visibleSignature = virtualRows.length ? `${virtualRows[0].index}:${virtualRows[virtualRows.length - 1].index}:${columns}:${itemsVersion}` : "";
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
  const beginRubber=(e:ReactPointerEvent<HTMLDivElement>)=>{if(e.button!==0||(e.target as HTMLElement).closest(".asset-tile,.tile-context-menu"))return;const r=e.currentTarget.getBoundingClientRect();const v={x1:e.clientX-r.left,y1:e.clientY-r.top,x2:e.clientX-r.left,y2:e.clientY-r.top,add:e.ctrlKey||e.metaKey};rubberRef.current=v;setRubber(v);e.currentTarget.setPointerCapture(e.pointerId);e.preventDefault()};
  const moveRubber=(e:ReactPointerEvent<HTMLDivElement>)=>{if(!rubberRef.current)return;const r=e.currentTarget.getBoundingClientRect();const v={...rubberRef.current,x2:e.clientX-r.left,y2:e.clientY-r.top};rubberRef.current=v;setRubber(v);if(v.y2<30)e.currentTarget.scrollTop-=22;else if(v.y2>r.height-30)e.currentTarget.scrollTop+=22};
  const endRubber=async()=>{const v=rubberRef.current;if(!v)return;rubberRef.current=null;setRubber(null);const el=scrollRef.current;if(!el)return;const xlo=Math.min(v.x1,v.x2),xhi=Math.max(v.x1,v.x2),ylo=Math.min(v.y1,v.y2)+el.scrollTop,yhi=Math.max(v.y1,v.y2)+el.scrollTop;const indexes:number[]=[];for(let row=Math.max(0,Math.floor((ylo-10)/rowHeight));row<=Math.min(rowCount-1,Math.floor((yhi-10)/rowHeight));row++){const cy=row*rowHeight+10+Math.min(zoom,rowHeight-24)/2;if(cy<ylo||cy>yhi)continue;for(let col=0;col<columns;col++){const cx=12+col*(tileWidth+9)+tileWidth/2;if(cx>=xlo&&cx<=xhi){const i=row*columns+col;if(i<total)indexes.push(i)}}}const needed=[...new Set(indexes.map(i=>Math.floor(i/PAGE_SIZE)*PAGE_SIZE))];for(const offset of needed){const p=await invoke<Page>("get_assets",{filter:{...params,offset,limit:PAGE_SIZE}});p.items.forEach((a,j)=>cache.current.set(offset+j,a));}const ids=indexes.map(i=>getAt(i)?.id).filter((id):id is number=>id!=null);onSelectAll(v.add?[...new Set([...selectedIds,...ids])]:ids);};
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
            return <AssetTile key={asset.id} asset={asset} folderPreviews={asset.path ? folderPreviews.get(asset.path) ?? [] : []} groupNumber={view.groups?.[index]} size={zoom} epoch={epoch} selected={selectedIds.has(asset.id)} primary={asset.id === primaryId} cut={cutIds.includes(asset.id)} selectedIds={[...selectedIds]} overlayMode={overlayMode} hoverMode={hoverMode} formats={formats} infoFields={thumbnailInfo.fields} tileMetadata={tileMetadata.get(asset.id)} onOpenThumbnailInfo={onOpenThumbnailInfo} onSetValues={onSetValues} onFileOperation={onFileOperation} onFindSimilar={onFindSimilar} aiReady={aiReady} aiReason={aiReason} readOnly={readOnly} uiScale={uiScale} onClick={e => void handleTileClick(e,asset,index)} onDoubleClick={() => onOpen(asset, index)} />;
          })}
        </div>)}
      </div>
    </div>
  </div>;
}

function AssetTile({ asset, folderPreviews, groupNumber, size, epoch, selected, primary, cut, onClick, onDoubleClick, overlayMode, hoverMode, selectedIds, formats, infoFields, tileMetadata, onOpenThumbnailInfo, onSetValues, onFileOperation, onFindSimilar, aiReady, aiReason, readOnly, uiScale }: { asset: Asset; folderPreviews: FolderPreview[]; groupNumber?:number; size: number; epoch: number; selected: boolean; primary: boolean; cut:boolean; selectedIds:number[]; onClick: (event: MouseEvent<HTMLDivElement>) => void; onDoubleClick: () => void; overlayMode:number; hoverMode:"off"|"hover"|"shift"; formats: FormatTable; infoFields: ThumbnailInfoConfig["fields"]; tileMetadata?: import("../types").TileExifMetadata; onOpenThumbnailInfo: () => void; onSetValues:(ids:number[],changes:Record<string,unknown>)=>void; onFileOperation:(ids:number[],mode:"move"|"copy"|"rename"|"delete"|"delete-permanent",asset?:Asset)=>void; onFindSimilar:(asset:Asset)=>void; aiReady:boolean; aiReason:string; readOnly:boolean; uiScale:number }) {
  // The epoch a tile was mounted with stays in its thumbnail URL, so later epoch bumps (far scroll, new
  // view) do not reload thumbnails that are already on screen; only a "stale" answer picks up the new one.
  const [tileEpoch,setTileEpoch]=useState(epoch); const latestEpoch=useRef(epoch); latestEpoch.current=epoch;
  const [broken, setBroken] = useState(false);
  const [offline, setOffline] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => { setBroken(false); setOffline(false); }, [asset.id, epoch]);
  const ext = asset.ext.toUpperCase();
  const format = formatFor(asset, formats);
  const badge = format?.group === "raw" ? ext.toUpperCase() : ext || "FILE";
  const label = asset.label?.trim();
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
      case "modified": return formatSlovakDate(asset.fileModified);
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
  const editors=useEditors();
  const [menu,setMenu]=useState<{x:number;y:number}|null>(null);
  const menuRef=useRef<HTMLDivElement>(null);
  const [menuPosition,setMenuPosition]=useState<{left:number;top:number}|null>(null);
  const [hoverMetadata,setHoverMetadata]=useState<CatalogAssetMetadata|null>(null);
  useLayoutEffect(()=>{if(!menu)return;const element=menuRef.current;if(!element)return;const pad=8,rect=element.getBoundingClientRect(),vw=window.innerWidth/uiScale,vh=window.innerHeight/uiScale;setMenuPosition({left:Math.max(pad,Math.min(menu.x,Math.max(pad,vw-rect.width-pad))),top:Math.max(pad,Math.min(menu.y,Math.max(pad,vh-rect.height-pad)))})},[menu,uiScale]);
  const [hover,setHover]=useState(false);const [popup,setPopup]=useState(false);const timer=useRef<number|undefined>(undefined);
  const [shiftDown,setShiftDown]=useState(!!window.__ddreamsShift);
  const tileRef=useRef<HTMLDivElement>(null);const popupRef=useRef<HTMLDivElement>(null);
  const [popupPosition,setPopupPosition]=useState<{left:number;top:number;maxHeight:number}|null>(null);
  useEffect(()=>{const down=(e:globalThis.KeyboardEvent)=>{if(e.key==="Shift")setShiftDown(true)};const up=(e:globalThis.KeyboardEvent)=>{if(e.key==="Shift")setShiftDown(false)};window.addEventListener("keydown",down);window.addEventListener("keyup",up);return()=>{window.removeEventListener("keydown",down);window.removeEventListener("keyup",up)}},[]);
  useEffect(()=>{if(!hover||hoverMode==="off"||(hoverMode==="shift"&&!shiftDown)){setPopup(false);return;}timer.current=window.setTimeout(()=>setPopup(true),500);return()=>{if(timer.current!==undefined)window.clearTimeout(timer.current)}},[hover,hoverMode,shiftDown]);
  useLayoutEffect(()=>{if(!popup)return;const tile=tileRef.current?.getBoundingClientRect();const card=popupRef.current;if(!tile||!card)return;const gap=8,pad=8,w=card.offsetWidth,h=card.offsetHeight,vw=window.innerWidth,vh=window.innerHeight;const candidates=[{left:tile.right+gap,top:tile.top},{left:tile.left-w-gap,top:tile.top},{left:tile.left,top:tile.bottom+gap},{left:tile.left,top:tile.top-h-gap}];const fits=candidates.find(p=>p.left>=pad&&p.top>=pad&&p.left+w<=vw-pad&&p.top+h<=vh-pad&&!(p.left<tile.right&&p.left+w>tile.left&&p.top<tile.bottom&&p.top+h>tile.top));const chosen=fits??{left:Math.min(Math.max(pad,tile.right+gap),Math.max(pad,vw-w-pad)),top:Math.min(Math.max(pad,tile.top),Math.max(pad,vh-Math.min(h,vh-pad*2)-pad))};setPopupPosition({left:chosen.left,top:chosen.top,maxHeight:Math.max(120,vh-pad*2)});},[popup,hoverMetadata]);
  useEffect(()=>{if(!popup)return;let current=true;void invoke<CatalogAssetMetadata|null>("get_catalog_asset_metadata",{assetId:asset.id}).then(m=>{if(current)setHoverMetadata(m)}).catch(()=>{});return()=>{current=false}},[popup,asset.id]);
  useEffect(()=>{const hide=()=>setPopup(false);window.addEventListener("ddreams-scroll",hide);window.addEventListener("resize",hide);return()=>{window.removeEventListener("ddreams-scroll",hide);window.removeEventListener("resize",hide)}},[]);
  return <div ref={tileRef} draggable={!asset.isDirectory} onDragStart={e=>{if(asset.isDirectory){e.preventDefault();return;}const ids=selected&&selectedIds.length?selectedIds:[asset.id];e.dataTransfer.setData("application/x-ddreams-asset-ids",JSON.stringify(ids));e.dataTransfer.effectAllowed="copyMove";const count=ids.length,noun=count===1?"súbor":count<=4?"súbory":"súborov",dragImage=document.createElement("div");dragImage.textContent=`${count} ${noun}`;Object.assign(dragImage.style,{position:"fixed",left:"-1000px",top:"-1000px",padding:"3px 8px",borderRadius:"10px",background:"#273644",color:"#d9dfe8",fontSize:"11px",lineHeight:"14px",border:"1px solid #465461",whiteSpace:"nowrap",pointerEvents:"none"});document.body.appendChild(dragImage);// Negative offsets place the image down and right of the cursor hotspot.
  e.dataTransfer.setDragImage(dragImage,-14,-14);window.setTimeout(()=>dragImage.remove(),0);}} className={`asset-tile overlay-${overlayMode} ${selected ? "selected" : ""} ${primary ? "primary" : ""} ${cut?"cut-pending":""} ${groupNumber!=null?`duplicate-group-${groupNumber%2}`:""}`} onContextMenu={e=>{e.preventDefault();setMenu({x:e.clientX,y:e.clientY})}} onMouseEnter={()=>setHover(true)} onMouseLeave={()=>{setHover(false);setPopup(false)}} onClick={onClick} onDoubleClick={onDoubleClick} title={asset.path ?? asset.name}>{groupNumber!=null&&<span className="duplicate-group-badge">#{groupNumber}</span>}
    <div className="thumb-wrap" style={{ height: size }}>
      {!asset.isDirectory && !broken && !offline && asset.path ? <img src={thumbUrl(asset, size, tileEpoch, retry)} loading="lazy" draggable={false} onError={async () => {
        try {
          const current = latestEpoch.current;
          const state = await invoke<"ready" | "offline" | "stale" | "unavailable">("get_thumbnail_state", { assetId: asset.id, epoch: current });
          if (state === "stale" || (state === "ready" && retry < 2)) { setTileEpoch(latestEpoch.current); setRetry(v => v + 1); }
          else if (state === "offline") setOffline(true);
          else setBroken(true);
        } catch { setBroken(true); }
      }} /> : asset.isDirectory ? <div className="folder-tile"><div className="folder-tab"/><div className="folder-body">{folderPreviews.slice(0, 4).map(preview => <img key={preview.id} className="folder-preview" src={thumbUrl(preview, 160, tileEpoch)} loading="lazy" draggable={false} onError={event => { event.currentTarget.style.display = "none"; }} />)}</div></div> : <div className="file-placeholder"><span>{offline || asset.offline ? "OFFLINE" : badge}</span>{(offline || asset.offline) && <small>Odpojené</small>}</div>}
      {overlayMode>0&&!asset.isDirectory && <span className="type-badge" title={`Typ súboru: ${badge}`}>{badge}</span>}
      {overlayMode>0&&(asset.hasCategories||!!asset.rating)&&<span className="tile-badges-left">{asset.hasCategories&&<span className="category-badge" title="Zaradené v kategórii">◆</span>}{!!asset.rating&&<span className="rating-badge" title={`Hodnotenie: ${Math.min(5, asset.rating)} z 5`}>{"★".repeat(Math.min(5, asset.rating))}</span>}</span>}
      {overlayMode>0&&!!asset.tagged && <span className="tagged-badge" title="Označené">✓</span>}
      
      {overlayMode>0&&label && <span className="label-badge" title={`Farebný štítok: ${labelText(label)}`} style={{ backgroundColor: labelColor(label) }} />}
      {overlayMode>0&&(offline||asset.offline)&&<span className="offline-badge" title="Súbor nie je dostupný, disk je odpojený alebo bol súbor presunutý">OFFLINE</span>}
      {!asset.isDirectory && !hasSizeLine && <span className="size-badge">{formatBytes(asset.size)}</span>}
    </div>
    <div className="tile-info-lines">{(asset.isDirectory ? [{id:"name",lines:1 as const}] : infoFields).map((field,index) => { const text=displayValue(field.id); return <div key={`${field.id}:${index}`} className={`tile-info-line ${field.id === "name" ? "tile-name" : ""}`} style={{ WebkitLineClamp: field.lines, maxHeight: field.lines * 14 }} title={text}>{text || "\u00a0"}</div>; })}</div>
    {menu&&createPortal(<div ref={menuRef} className="tile-context-menu" style={{left:menuPosition?.left??menu.x,top:menuPosition?.top??menu.y}} onPointerDown={e=>e.stopPropagation()} onMouseDown={e=>e.stopPropagation()} onClick={e=>e.stopPropagation()} onDoubleClick={e=>e.stopPropagation()} onContextMenu={e=>{e.preventDefault();e.stopPropagation()}} onMouseLeave={()=>setMenu(null)}><button onClick={()=>{setMenu(null);onDoubleClick()}}>Otvoriť</button><button disabled={asset.offline||offline} onClick={()=>{setMenu(null);void invoke("asset_action",{assetId:asset.id,action:"open"})}}>Otvoriť v predvolenom programe</button>{!asset.isDirectory&&editors.map((editor,index)=><button key={editor.path} disabled={asset.offline||offline} title={editor.path} onClick={()=>{setMenu(null);void invoke("open_in_editor",{assetIds:selected&&selectedIds.length?selectedIds:[asset.id],exe:editor.path,multi:editor.multi}).catch(error=>window.alert(String(error)))}}>Otvoriť v: {editor.name}{index===0?" (predvolený)":""}</button>)}<button disabled={asset.offline||offline} onClick={()=>{setMenu(null);void invoke("asset_action",{assetId:asset.id,action:"reveal"})}}>Ukázať v Prieskumníkovi</button><button onClick={async()=>{setMenu(null);const ids=selected && selectedIds.length ? selectedIds : [asset.id];const paths=await invoke<string[]>("get_asset_paths",{ids});await navigator.clipboard.writeText(paths.join("\n"))}}>Kopírovať cestu</button><button onClick={()=>{setMenu(null);onOpenThumbnailInfo()}}>Údaje pod náhľadmi…</button><hr className="menu-separator"/><div className={`write-menu-group ${readOnly?"readonly":""}`}><div className="write-menu-row"><span>Hodnotenie</span>{[1,2,3,4,5].map(value=><button key={value} title={`Ctrl+${value}`} disabled={readOnly} onClick={()=>{onSetValues(selected&&selectedIds.length?selectedIds:[asset.id],{rating:value});setMenu(null)}}>★{value}</button>)}<button title="Ctrl+0" disabled={readOnly} onClick={()=>{onSetValues(selected&&selectedIds.length?selectedIds:[asset.id],{rating:null});setMenu(null)}}>×</button></div><div className="write-menu-row"><span>Farebný štítok</span>{[["Red","Červená"],["Yellow","Žltá"],["Green","Zelená"],["Blue","Modrá"],["Purple","Fialová"]].map(([value,label],index)=><button key={value} title={`${label} (Alt+${index+1})`} disabled={readOnly} onClick={()=>{onSetValues(selected&&selectedIds.length?selectedIds:[asset.id],{label:value});setMenu(null)}}><span className="write-menu-dot" style={{backgroundColor:value.toLowerCase()}} /></button>)}<button title="Alt+0" disabled={readOnly} onClick={()=>{onSetValues(selected&&selectedIds.length?selectedIds:[asset.id],{label:null});setMenu(null)}}>×</button></div><button title="\\" disabled={readOnly} onClick={()=>{onSetValues(selected&&selectedIds.length?selectedIds:[asset.id],{tagged:"toggle"});setMenu(null)}}>Označiť / Zrušiť označenie (\)</button></div><hr className="menu-separator"/><button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onFileOperation(selected&&selectedIds.length?selectedIds:[asset.id],"move")}}>Presunúť do…</button><button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onFileOperation(selected&&selectedIds.length?selectedIds:[asset.id],"copy")}}>Kopírovať do…</button><button disabled={readOnly||asset.offline||offline} onClick={()=>{setMenu(null);onFileOperation([asset.id],"rename",asset)}}>Premenovať (F2)</button><hr className="menu-separator"/>{asset.id>0&&!asset.isDirectory&&<button disabled={!aiReady} title={!aiReady?aiReason:""} onClick={()=>{setMenu(null);onFindSimilar(asset)}}>Nájsť podobné</button>}<hr className="menu-separator"/><hr className="menu-separator"/><button disabled={readOnly||asset.isDirectory} onClick={()=>{setMenu(null);onFileOperation(selected&&selectedIds.length?selectedIds:[asset.id],"delete",asset)}}>Odstrániť do Koša… (Del)</button><button className="menu-danger" disabled={readOnly||asset.isDirectory} onClick={()=>{setMenu(null);onFileOperation(selected&&selectedIds.length?selectedIds:[asset.id],"delete-permanent",asset)}}>Zmazať natrvalo… (Shift+Del)</button></div>,document.body)}
    {popup && createPortal(
      <div ref={popupRef} className="hover-card" style={{ position: "fixed", left: popupPosition?.left ?? -10000, top: popupPosition?.top ?? -10000, maxHeight: popupPosition?.maxHeight ?? "80vh", overflowY: "auto" }}>
        <div className="hover-image">{!asset.isDirectory && asset.path && <img src={thumbUrl(asset, 320, epoch)} />}</div>
        <b title={asset.name}>{asset.name}</b>
        <small>{formatBytes(asset.size)} {"\u00b7"} {asset.width && asset.height ? `${asset.width} \u00d7 ${asset.height}` : "\u2014"}</small>
        <small>{asset.exifDate ?? asset.fileModified ?? "\u2014"}</small>
        <small>{asset.rating ? `\u2605 ${asset.rating}` : "Bez hodnotenia"} {label ? labelText(label) : ""}</small>
        <small>{hoverMetadata?.categories.length ? `Kateg\u00f3rie: ${hoverMetadata.categories.join(", ")}` : "Bez kateg\u00f3ri\u00ed"}</small>
      </div>, document.body
    )}
  </div>;
}
export function thumbUrl(asset: Pick<Asset, "id" | "path" | "fileModified">, size: number, epoch: number, retry = 0): string {
  const url = new URL(convertFileSrc(asset.path ?? "", "thumb"));
  url.searchParams.set("id", String(asset.id));
  url.searchParams.set("s", String(size));
  url.searchParams.set("v", String(epoch));
  url.searchParams.set("r", String(retry));
  if (asset.fileModified) url.searchParams.set("m", asset.fileModified);
  return url.toString();
}
export function labelText(label: string): string {
  const names: Record<string,string> = { red:"\u010cerven\u00e1", yellow:"\u017dlt\u00e1", green:"Zelen\u00e1", blue:"Modr\u00e1", purple:"Fialov\u00e1", "\u010derven\u00e1":"\u010cerven\u00e1", "\u017elt\u00e1":"\u017dlt\u00e1", "zelen\u00e1":"Zelen\u00e1", "modr\u00e1":"Modr\u00e1", "fialov\u00e1":"Fialov\u00e1" };
  return names[label.toLocaleLowerCase("sk-SK")] ?? label;
}
export function labelColor(label: string): string {
  const colors: Record<string, string> = { red: "#c94c50", "\u010derven\u00e1": "#c94c50", yellow: "#c49b37", "\u017elt\u00e1": "#c49b37", green: "#4b9862", "zelen\u00e1": "#4b9862", blue: "#4e7db7", "modr\u00e1": "#4e7db7", purple: "#8866b4", "fialov\u00e1": "#8866b4" };
  return colors[label.toLowerCase()] ?? "#6b7280";
}

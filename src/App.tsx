import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import AssetGrid, { formatBytes, formatFor, labelText, thumbUrl } from "./components/AssetGrid";
import LeftPanel from "./components/LeftPanel";
import RightPanel from "./components/RightPanel";
import ThumbnailInfoDialog from "./components/ThumbnailInfoDialog";
import type { Asset, Folder, FormatTable, Page, View, ViewFilters, ValueCount } from "./types";
import { readStored, writeStored } from "./storage";
import { DEFAULT_THUMBNAIL_INFO, normalizeThumbnailInfo, type ThumbnailInfoConfig } from "./thumbnailInfo";
import "./App.css";

const DEFAULT_PATH = String.raw`F:\My_docs\My Pictures\Fotky`;
type AppInfo = { root: string; catalog: string; backups: string; cache: string; exiftool: string; formats: string; metadataView: string; settings: string; portable: boolean; readonly: boolean };
function dateText(value: string | null | undefined) { return value ? value.replace("T", " ").slice(0, 19) : "—"; }

export default function App() {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  useEffect(() => { void invoke<AppInfo>("get_app_info").then(setAppInfo).catch(() => {}); }, []);
  const catalogTooltip = appInfo ? `Katalóg: ${appInfo.catalog}\nZálohy: ${appInfo.backups}\nCache: ${appInfo.cache}\nExifTool: ${appInfo.exiftool}\nFormáty: ${appInfo.formats}\nMetadata: ${appInfo.metadataView}\nNastavenia: ${appInfo.settings}\nRežim: ${appInfo.portable ? "prenosný" : "vývojový"}` : "Cesty katalógu sa načítavajú";
  const [view, setView] = useState<View>({ kind: "folder", id: -1, title: "Načítavam katalóg…" });
  const [query, setQuery] = useState("");
  const [queryDebounced, setQueryDebounced] = useState("");
  const [sort, setSort] = useState("name");
  const [descending, setDescending] = useState(false);
  const [zoom, setZoom] = useState(168);
  const [overlayMode, setOverlayMode] = useState<0|2>(()=>readStored<number>("ddreams.overlay",2)===0?0:2);
  const [hoverMode, setHoverMode] = useState<"off"|"hover"|"shift">(()=>readStored("ddreams.hover", "shift"));
  const [thumbnailInfo, setThumbnailInfo] = useState<ThumbnailInfoConfig>(()=>normalizeThumbnailInfo(readStored("ddreams.thumbnailInfo",DEFAULT_THUMBNAIL_INFO)));
  const [thumbnailInfoOpen, setThumbnailInfoOpen] = useState(false);
  const [filters, setFilters] = useState<ViewFilters>({});
  const [includeChildren, setIncludeChildren] = useState<boolean>(()=>readStored("ddreams.includeChildren",true));
  const [availableLabels,setAvailableLabels]=useState<ValueCount[]>([]);
  const [history, setHistory] = useState<View[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [epoch, setEpoch] = useState(1);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [primaryId, setPrimaryId] = useState<number | null>(null);
  const [primaryAsset, setPrimaryAsset] = useState<Asset | null>(null);
  const [total, setTotal] = useState(0);
  const [totalSize, setTotalSize] = useState(0);
  const [toast,setToast]=useState("");
  const [refreshTick,setRefreshTick]=useState(0);
  const [viewer, setViewer] = useState<{ asset: Asset; index: number } | null>(null);
  const [slideshow, setSlideshow] = useState(false);
  const [slideshowSeconds, setSlideshowSeconds] = useState(5);
  const [viewerFit,setViewerFit]=useState(true);
  const [viewerZoom,setViewerZoom]=useState(1);
  const [viewerPan,setViewerPan]=useState({x:0,y:0});
  const [viewerLock,setViewerLock]=useState(false);
  const [viewerDimensions,setViewerDimensions]=useState<{width:number;height:number}|null>(null);
  const [viewerPreviewSource,setViewerPreviewSource]=useState<string|null>(null);
  const [viewerBroken,setViewerBroken]=useState(false);
  const [formats,setFormats]=useState<FormatTable>({version:0,formats:[]});
  const [viewerCanvasSize,setViewerCanvasSize]=useState({width:0,height:0});
  const viewerCanvasRef=useRef<HTMLDivElement>(null);
  const [viewerOrigin,setViewerOrigin]=useState("50% 50%");

  useEffect(() => { const timer = window.setTimeout(() => setQueryDebounced(query), 180); return () => window.clearTimeout(timer); }, [query]);
  useEffect(() => { setZoom(readStored("ddreams.zoom",168)); const path=readStored("ddreams.lastFolder",DEFAULT_PATH); void invoke<Folder | null>("find_folder_path", { path }).then(folder => { const v=folder?{kind:"folder" as const,id:folder.id,ids:folder.folderIds,title:folder.path??folder.name,folderPath:folder.path??undefined}:{kind:"folder" as const,id:-1,title:DEFAULT_PATH,folderPath:DEFAULT_PATH};setView(v);setHistory([v]);setHistoryIndex(0); }).catch(() => { const v={kind:"folder" as const,id:-1,title:DEFAULT_PATH,folderPath:DEFAULT_PATH};setView(v);setHistory([v]);setHistoryIndex(0); }); }, []);
  useEffect(()=>{void invoke<FormatTable>("get_formats").then(setFormats).catch(error=>console.error("Format table failed to load",error));},[]);
  useEffect(()=>{void invoke<ValueCount[]>("get_catalog_values",{kind:"label",offset:0,limit:500}).then(setAvailableLabels).catch(()=>setAvailableLabels([]))},[]);
  useEffect(() => writeStored("ddreams.zoom",zoom),[zoom]);
  useEffect(() => writeStored("ddreams.overlay",overlayMode),[overlayMode]);
  useEffect(() => writeStored("ddreams.hover",hoverMode),[hoverMode]);
  useEffect(() => writeStored("ddreams.thumbnailInfo",thumbnailInfo),[thumbnailInfo]);
  useEffect(() => writeStored("ddreams.includeChildren",includeChildren),[includeChildren]);
  const sortKey=view.kind==="folder"?`folder:${(view.folderPath??view.title).replace(/[\\/]+$/g,"").replace(/\//g,"\\").toLowerCase()}`:"catalog";
  useEffect(()=>{const saved=readStored<Record<string,{sort:string;descending:boolean;stamp:number}>>("ddreams.sorts",{});if(saved[sortKey]){setSort(saved[sortKey].sort);setDescending(saved[sortKey].descending)}},[sortKey]);
  useEffect(()=>{const saved=readStored<Record<string,{sort:string;descending:boolean;stamp:number}>>("ddreams.sorts",{});saved[sortKey]={sort,descending,stamp:Date.now()};const entries=Object.entries(saved).sort((a,b)=>b[1].stamp-a[1].stamp).slice(0,500);writeStored("ddreams.sorts",Object.fromEntries(entries));},[sortKey,sort,descending]);
  const counts = useCallback((n: number, size: number) => { setTotal(n); setTotalSize(size); }, []);
  const navigate = useCallback((next: View) => { setView(next);setHistory(h=>{const base=h.slice(0,historyIndex+1);base.push(next);setHistoryIndex(base.length-1);return base;}); setQuery(""); setQueryDebounced(""); setFilters({});setSelectedIds(new Set()); setPrimaryId(null); setPrimaryAsset(null); setViewer(null);if(next.kind==="folder")writeStored("ddreams.lastFolder",next.folderPath??next.title); }, [historyIndex]);
  const travel = useCallback((index:number)=>{const next=history[index];if(!next)return;setHistoryIndex(index);setView(next);setQuery("");setFilters({});setSelectedIds(new Set());setPrimaryId(null);setPrimaryAsset(null);setViewer(null);},[history]);
  const select = useCallback((asset: Asset, _index: number, event: ReactMouseEvent | ReactKeyboardEvent) => {
    setPrimaryId(asset.id); setPrimaryAsset(asset);
    if (event.ctrlKey || event.metaKey) { const next=new Set(selectedIds);if(next.has(asset.id))next.delete(asset.id);else next.add(asset.id);setSelectedIds(next);if(!next.size){setPrimaryId(null);setPrimaryAsset(null)} }
    else if (!event.shiftKey) setSelectedIds(new Set([asset.id]));
  }, [selectedIds]);
  const selectRange = useCallback((ids: number[], asset: Asset, _index: number) => { setSelectedIds(new Set(ids)); setPrimaryId(asset.id); setPrimaryAsset(asset); }, []);
  const selectAll = useCallback(async (ids: number[]) => { setSelectedIds(new Set(ids)); if (!ids.length) {setPrimaryId(null);setPrimaryAsset(null);return;} const wanted=new Set(ids);const page = await invoke<Page>("get_assets", { filter: { kind: view.kind, id: view.id ?? null, rootId:view.rootId??null, includeChildren:(view.kind==="category"||view.kind==="keyword")&&includeChildren, ids: view.ids ?? null, folderPath: view.folderPath ?? null, value: view.value ?? null, query: queryDebounced, sort, descending,ratingMode:filters.ratingMode,ratingValue:filters.ratingValue,labelFilter:filters.labelFilter,taggedOnly:filters.taggedOnly,fileGroup:filters.fileGroup,dateFrom:view.dateFrom,dateTo:view.dateTo,dateSource:view.dateSource,dateNoDate:view.dateNoDate, offset: 0, limit: 500 } }); const asset=page.items.find(a=>wanted.has(a.id));if(asset){setPrimaryId(asset.id);setPrimaryAsset(asset);} }, [view, queryDebounced, sort, descending,filters,includeChildren]);
  const updateViewport = useCallback(() => { setEpoch(v => { const next=v+1;void invoke("set_thumbnail_epoch",{epoch:next});return next; }); }, []);
  const up = useCallback(async()=>{if(view.kind!=="folder")return;const p=view.folderPath??view.title;const parts=p.split(/[\\/]/).filter(Boolean);if(parts.length<=1)return;parts.pop();let parent=parts.join("\\");if(/^[A-Za-z]:$/.test(parent))parent+="\\";const f=await invoke<Folder|null>("find_folder_path",{path:parent}).catch(()=>null);if(f)navigate({kind:"folder",id:f.id,ids:f.folderIds,title:f.path??f.name,folderPath:f.path??undefined});},[view,navigate]);
  const refresh = useCallback(()=>{if(view.kind==="folder"&&view.id!=null)void invoke("refresh_folder_listing",{folderId:view.id});setRefreshTick(v=>v+1);updateViewport();},[view,updateViewport]);
  const copyCurrentResult=useCallback(async()=>{let lines:string[]=[];const params={kind:view.kind,id:view.id??null,rootId:view.rootId??null,includeChildren:(view.kind==="category"||view.kind==="keyword")&&includeChildren,ids:view.ids??null,folderPath:view.folderPath??null,value:view.value??null,query:queryDebounced,sort,descending,ratingMode:filters.ratingMode??null,ratingValue:filters.ratingValue??null,labelFilter:filters.labelFilter??null,taggedOnly:filters.taggedOnly??false,fileGroup:filters.fileGroup??null,dateFrom:view.dateFrom??null,dateTo:view.dateTo??null,dateSource:view.dateSource??null,dateNoDate:view.dateNoDate??false};for(let offset=0;offset<total;offset+=500){const p=await invoke<Page>("get_assets",{filter:{...params,offset,limit:500}});for(const a of p.items)if(!a.isDirectory)lines.push([a.name,a.size??"",a.ext,a.fileModified??"",a.width&&a.height?`${a.width} × ${a.height}`:"",a.caption??"",a.rating??""].join("\t"));if(p.items.length<500)break;}await navigator.clipboard.writeText(lines.join("\n"));setToast(`Skopírované: ${lines.length} položiek`);window.setTimeout(()=>setToast(""),1600);},[view,queryDebounced,sort,descending,filters,total,includeChildren]);
  useEffect(()=>{const key=(e:globalThis.KeyboardEvent)=>{if(e.key==="Shift")window.__ddreamsShift=true;if(viewer)return;const el=e.target as HTMLElement|null;const editable=!!el&&((el.tagName==="INPUT")||(el.tagName==="SELECT")||(el.tagName==="TEXTAREA")||el.isContentEditable);if(editable)return;if(e.ctrlKey&&e.key.toLowerCase()==="g"){e.preventDefault();void copyCurrentResult()}else if(e.ctrlKey&&e.shiftKey&&e.key.toLowerCase()==="o"&&primaryId!=null){e.preventDefault();void invoke("asset_action",{assetId:primaryId,action:"open"})}else if(e.altKey&&e.key==="ArrowLeft"){e.preventDefault();travel(historyIndex-1)}else if(e.altKey&&e.key==="ArrowRight"){e.preventDefault();travel(historyIndex+1)}else if(e.key==="Backspace"){e.preventDefault();void up()}else if(e.key==="F5"){e.preventDefault();refresh()}else if(e.key==="]"){setOverlayMode(v=>v===0?2:0)}};const keyup=(e:globalThis.KeyboardEvent)=>{if(e.key==="Shift")window.__ddreamsShift=false};const zoomer=(e:Event)=>setZoom(v=>Math.max(80,Math.min(400,v+(e as CustomEvent<number>).detail)));const mouse=(e:globalThis.MouseEvent)=>{if(e.button===3){e.preventDefault();travel(historyIndex-1)}else if(e.button===4){e.preventDefault();travel(historyIndex+1)}};window.addEventListener("keydown",key);window.addEventListener("keyup",keyup);window.addEventListener("ddreams-zoom",zoomer);window.addEventListener("mouseup",mouse);return()=>{window.removeEventListener("keydown",key);window.removeEventListener("keyup",keyup);window.removeEventListener("ddreams-zoom",zoomer);window.removeEventListener("mouseup",mouse)}},[travel,historyIndex,up,refresh,copyCurrentResult,primaryId,viewer]);

  const moveViewer = useCallback(async (delta: number) => {
    if (!viewer) return;
    let index = viewer.index + delta;
    while (index >= 0 && index < total) {
      const page = await invoke<Page>("get_assets", { filter: { kind: view.kind, id: view.id ?? null, rootId:view.rootId??null, includeChildren:(view.kind==="category"||view.kind==="keyword")&&includeChildren, ids: view.ids ?? null, folderPath: view.folderPath ?? null, value: view.value ?? null, query: queryDebounced, sort, descending, ...filters, offset: index, limit: 1 } });
      const asset=page.items[0];
      if (!asset) break;
      if (!asset.isDirectory && ["image", "raw"].includes(formatFor(asset, formats)?.group ?? "")) { setViewer({asset,index});setViewerDimensions(null);setPrimaryId(asset.id);setPrimaryAsset(asset);setSelectedIds(new Set([asset.id]));if(!viewerLock){setViewerFit(true);setViewerZoom(1);setViewerPan({x:0,y:0})} return; }
      index += delta;
    }
    setSlideshow(false);
  }, [viewer,total,view,queryDebounced,sort,descending,viewerLock,includeChildren,filters,formats]);
  useEffect(() => { if (!viewer) setSlideshow(false); }, [viewer]);
  useEffect(() => {
    if (!slideshow || !viewer) return;
    if (viewer.index >= total - 1) { setSlideshow(false); return; }
    const timer = window.setTimeout(() => void moveViewer(1).catch(() => { setSlideshow(false); setToast("Prezentáciu sa nepodarilo pokračovať."); }), slideshowSeconds * 1000);
    return () => window.clearTimeout(timer);
  }, [slideshow, viewer, total, slideshowSeconds, moveViewer]);
  useEffect(() => {
    if (!viewer) return;
    const key = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape"||event.key==="Enter") setViewer(null); else if(viewerFit&&event.key==="ArrowRight")void moveViewer(1);else if(viewerFit&&event.key==="ArrowLeft")void moveViewer(-1);else if(viewerFit&&event.code==="Space"){event.preventDefault();void moveViewer(1)}else if(viewerFit&&event.key==="Backspace"){event.preventDefault();void moveViewer(-1)}else if(!viewerFit&&event.key.startsWith("Arrow")){const step=event.ctrlKey?120:event.shiftKey?12:48;setViewerPan(p=>({...p,x:p.x+(event.key==="ArrowRight"?-step:event.key==="ArrowLeft"?step:0),y:p.y+(event.key==="ArrowDown"?-step:event.key==="ArrowUp"?step:0)}))}else if(event.key==="+"||event.key==="="){setViewerFit(false);setViewerZoom(z=>Math.min(8,z*1.2))}else if(event.key==="-"){setViewerFit(false);setViewerZoom(z=>Math.max(.1,z/1.2))} };
    window.addEventListener("keydown",key); return () => window.removeEventListener("keydown",key);
  }, [viewer,moveViewer,viewerFit]);
  useEffect(()=>{if(!viewer||!viewerCanvasRef.current)return;const el=viewerCanvasRef.current;const update=()=>setViewerCanvasSize({width:el.clientWidth,height:el.clientHeight});update();const observer=new ResizeObserver(update);observer.observe(el);return()=>observer.disconnect()},[viewer]);
  const viewerFitPercent=viewerDimensions&&viewerCanvasSize.width&&viewerCanvasSize.height?Math.round(Math.min((viewerCanvasSize.width-120)/viewerDimensions.width,(viewerCanvasSize.height-24)/viewerDimensions.height)*100):0;
  useEffect(()=>{if(!viewer)return;let active=true;setViewerPreviewSource(null);setViewerBroken(false);void invoke<string>("get_preview_source",{assetId:viewer.asset.id,epoch}).then(source=>{if(active)setViewerPreviewSource(source)}).catch(()=>{});for(const d of [-1,1]){const i=viewer.index+d;if(i>=0&&i<total)void invoke<Page>("get_assets",{filter:{kind:view.kind,id:view.id??null,rootId:view.rootId??null,includeChildren:(view.kind==="category"||view.kind==="keyword")&&includeChildren,ids:view.ids??null,folderPath:view.folderPath??null,value:view.value??null,dateFrom:view.dateFrom??null,dateTo:view.dateTo??null,dateSource:view.dateSource??null,dateNoDate:view.dateNoDate??false,query:queryDebounced,sort,descending,...filters,offset:i,limit:1}}).then(p=>{const a=p.items[0];if(a&&!a.isDirectory&&a.path){const im=new Image();im.src=thumbUrl(a,0,epoch)}})}return()=>{active=false}},[viewer?.asset.id,viewer?.index,view,queryDebounced,sort,descending,filters,total,epoch,includeChildren]);

  const crumbs = useMemo(() => view.kind === "folder" ? (view.folderPath ?? view.title).split(/[\\/]/).filter(Boolean) : ["Katalóg", view.title], [view]);
  const openCrumb = async (index: number) => {
    if (view.kind !== "folder") return;
    const parts = (view.folderPath ?? view.title).split(/[\\/]/).filter(Boolean);
    let path = parts.slice(0, index + 1).join("\\");
    if (index === 0 && /^[A-Za-z]:$/.test(path)) path += "\\";
    const folder = await invoke<Folder | null>("find_folder_path", { path }).catch(() => null);
    if (folder) navigate({ kind: "folder", id: folder.id, ids: folder.folderIds, title: folder.path ?? folder.name, folderPath: folder.path ?? undefined });
  };
  return <main className="app-shell">
    <header className="topbar"><div className="brand-mark">D</div><div className="brand-title">DDreams <b>browser</b></div><div className="top-separator"/><span className="mode-label">SPRÁVA FOTOGRAFIÍ</span><div className="top-spacer"/><div className="catalog-label" title={catalogTooltip}><span className="status-light"/>{appInfo?.readonly ? "Katalóg iba na čítanie" : "Katalóg"}</div></header>
    <div className="workspace">
      <LeftPanel onNavigate={navigate} activeFolderId={view.kind === "folder" ? view.id ?? null : null} activeFolderIds={view.kind === "folder" ? view.ids : []} activeView={view} refreshToken={refreshTick}/>
      <section className="center-pane">
        <div className="breadcrumb-bar"><span className="crumb-home">⌂</span>{crumbs.map((crumb,i)=><span key={`${crumb}-${i}`} className="crumb-item">{i>0&&<i>›</i>}<button onClick={()=>void openCrumb(i)}>{crumb}</button></span>)}{Object.values(filters).some(Boolean)&&<span className="active-filters">{filters.ratingMode&&`Hodnotenie ${filters.ratingMode} ${filters.ratingValue??""}`} {filters.labelFilter&&`Farebný štítok: ${labelText(filters.labelFilter)}`} {filters.taggedOnly&&"Označené"} {filters.fileGroup&&`Typ: ${filters.fileGroup}`}<button onClick={()=>setFilters({})}>×</button></span>}</div>
        <div className="toolbar"><div className="toolbar-left"><button disabled={historyIndex<=0} onClick={()=>travel(historyIndex-1)}>←</button><button disabled={historyIndex>=history.length-1} onClick={()=>travel(historyIndex+1)}>→</button><button disabled={view.kind!=="folder"} onClick={()=>void up()}>↑</button><button onClick={refresh}>↻</button><label className="search-box"><span>⌕</span><input value={query} onChange={e=>setQuery(e.target.value)} placeholder="Filtrovať v zobrazení…"/><kbd>Ctrl+F</kbd></label></div><div className="toolbar-right"><span className="sort-label">Zoradiť:</span><select value={sort} onChange={e=>setSort(e.target.value)}>{[["name","Názov"],["dateTaken","Dátum nasnímania"],["modified","Dátum zmeny"],["size","Veľkosť"],["type","Typ súboru"],["dimensions","Rozmery"],["rating","Hodnotenie"],["label","Farebný štítok"],["tagged","Označené"],["caption","Popis"]].map(([v,l])=><option value={v} key={v}>{l}</option>)}</select><button className="direction-button" title={descending?"Zostupne":"Vzostupne"} onClick={()=>setDescending(v=>!v)}>{descending?"↓":"↑"}</button><details className="toolbar-menu"><summary>Filtre</summary><div className="toolbar-pop"><select value={filters.ratingMode??""} onChange={e=>setFilters(f=>({...f,ratingMode:(e.target.value||undefined) as ViewFilters["ratingMode"],ratingValue:e.target.value&&e.target.value!=="unrated"?(f.ratingValue??1):undefined}))}><option value="">Hodnotenie: všetky</option><option value="equal">Rovné</option><option value="atLeast">Aspoň</option><option value="atMost">Najviac</option><option value="unrated">Bez hodnotenia</option></select>{filters.ratingMode&&filters.ratingMode!=="unrated"&&<select value={filters.ratingValue??1} onChange={e=>setFilters(f=>({...f,ratingValue:Number(e.target.value)}))}>{[1,2,3,4,5].map(v=><option value={v} key={v}>{v}</option>)}</select>}<select value={filters.labelFilter??""} onChange={e=>setFilters(f=>({...f,labelFilter:e.target.value||undefined}))}><option value="">Farebný štítok: všetky</option>{availableLabels.filter(v=>v.value).map(v=><option value={v.value} key={v.value}>{labelText(v.value)}</option>)}</select><label><input type="checkbox" checked={!!filters.taggedOnly} onChange={e=>setFilters(f=>({...f,taggedOnly:e.target.checked||undefined}))}/> Len označené</label><select value={filters.fileGroup??""} onChange={e=>setFilters(f=>({...f,fileGroup:(e.target.value||undefined) as ViewFilters["fileGroup"]}))}><option value="">Typ: všetky</option>{[["images","Fotografie"],["raw","RAW"],["video","Video"],["other","Ostatné"]].map(([v,l])=><option value={v} key={v}>{l}</option>)}</select><button onClick={()=>setFilters({})}>Vymazať filtre</button></div></details><select value={hoverMode} onChange={e=>setHoverMode(e.target.value as typeof hoverMode)} title="Náhľad po prejdení myšou"><option value="off">Náhľad vypnutý</option><option value="hover">Náhľad pri prejdení</option><option value="shift">Náhľad so Shift</option></select><button onClick={()=>setOverlayMode(v=>v===0?2:0)} title="Ikonky na náhľadoch, kláves ]">Ikonky {overlayMode===0?"vypnuté":"zapnuté"}</button><button onClick={()=>setThumbnailInfoOpen(true)}>&#218;daje pod n&#225;h&#318;admi&#8230;</button><div className="zoom-control"><span>−</span><input type="range" min="80" max="400" step="8" value={zoom} onChange={e=>setZoom(Number(e.target.value))}/><span>＋</span></div></div></div>
        <div className="grid-heading"><span>{view.title}</span><span>{total.toLocaleString("sk-SK")} položiek {formatBytes(totalSize)}</span></div>
        <AssetGrid view={view} query={queryDebounced} sort={sort} descending={descending} zoom={zoom} epoch={epoch} refreshToken={refreshTick} selectedIds={selectedIds} primaryId={primaryId} filters={filters} overlayMode={overlayMode} hoverMode={hoverMode} includeChildren={includeChildren} formats={formats} thumbnailInfo={thumbnailInfo} onOpenThumbnailInfo={()=>setThumbnailInfoOpen(true)} onSelect={select} onSelectRange={selectRange} onSelectAll={ids=>void selectAll(ids)} onOpen={(asset,index)=>{if(asset.isDirectory){const id=asset.folderId??asset.id;navigate({kind:"folder",id,ids:asset.folderId?[asset.folderId]:undefined,title:asset.path??asset.name,folderPath:asset.path??undefined});return;}setViewer({asset,index});setPrimaryId(asset.id);setPrimaryAsset(asset);setSelectedIds(new Set([asset.id]));}} onCounts={counts} onViewport={updateViewport}/>
        <div className="selection-bar">{primaryAsset ? <><div className="selected-name">{selectedIds.size > 1 ? `${selectedIds.size} vybraných` : primaryAsset.name}</div><span>{formatBytes(primaryAsset.size)}</span><span>{dateText(primaryAsset.exifDate)}</span><span>{primaryAsset.width && primaryAsset.height ? `${primaryAsset.width} × ${primaryAsset.height}` : "—"}</span></> : null}<span className="selection-total">Spolu {total.toLocaleString("sk-SK")} položiek · {formatBytes(totalSize)}</span></div>
      </section>
      <RightPanel primaryId={primaryId} selectedIds={[...selectedIds]} includeChildren={includeChildren} onIncludeChildrenChange={setIncludeChildren} onNavigate={navigate}/>
    </div>
    {viewer && <div className="viewer-overlay"><div className="viewer-top"><button onClick={()=>setViewer(null)}>✕ Zavrieť</button><span>{viewer.asset.name}</span><span>{viewer.index+1} / {total}</span><button onClick={()=>setSlideshow(v=>!v)} disabled={total<2}>{slideshow?"Pozastaviť prezentáciu":"Spustiť prezentáciu"}</button><label>Interval <select aria-label="Interval prezentácie" value={slideshowSeconds} onChange={e=>setSlideshowSeconds(Number(e.target.value))}>{[2,3,5,10,15,30].map(seconds=><option key={seconds} value={seconds}>{seconds} s</option>)}</select></label><button onClick={()=>{setViewerFit(v=>!v);setViewerZoom(1);setViewerPan({x:0,y:0})}}>{viewerFit?"100 %":"Prispôsobiť"}</button><span>{viewerFit?`${viewerFitPercent} %`:`${Math.round(viewerZoom*100)} %`} {viewerDimensions&&<small> · {viewerDimensions.width} × {viewerDimensions.height}{formatFor(viewer.asset,formats)?.group==="raw"?" \u00b7 vlo\u017een\u00fd n\u00e1h\u013ead":""}{viewerPreviewSource?` \u00b7 zdroj: ${viewerPreviewSource}`:""}</small>}</span><button onClick={()=>setViewerLock(v=>!v)}>{viewerLock?"🔒":"🔓"}</button></div><div ref={viewerCanvasRef} className="viewer-canvas" onPointerDown={e=>{if(!viewerFit)e.currentTarget.setPointerCapture(e.pointerId)}} onWheel={e=>{e.preventDefault();const r=e.currentTarget.getBoundingClientRect();setViewerOrigin(`${e.clientX-r.left}px ${e.clientY-r.top}px`);setViewerFit(false);setViewerZoom(z=>Math.max(.1,Math.min(8,z*(e.deltaY<0?1.12:.89))))}} onPointerMove={e=>{if(e.buttons===1&&!viewerFit)setViewerPan(p=>({x:p.x+e.movementX,y:p.y+e.movementY}))}} onDoubleClick={()=>setViewerFit(v=>!v)}><button className="viewer-arrow" onClick={()=>void moveViewer(-1)}>‹</button>{!viewer.asset.isDirectory&&!viewerBroken?<img className="viewer-image" style={viewerFit?{maxWidth:"100%",maxHeight:"100%",objectFit:"contain"}:{maxWidth:"none",maxHeight:"none",transformOrigin:viewerOrigin,transform:`translate(${viewerPan.x}px,${viewerPan.y}px) scale(${viewerZoom})`}} src={thumbUrl(viewer.asset,0,epoch)} alt={viewer.asset.name} onLoad={e=>setViewerDimensions({width:e.currentTarget.naturalWidth,height:e.currentTarget.naturalHeight})} onError={()=>setViewerBroken(true)} onClick={()=>setViewerFit(v=>!v)}/>:<div className="viewer-nonimage">{viewer.asset.ext.toUpperCase()}<small>Náhľad nie je k dispozícii</small></div>}<button className="viewer-arrow" onClick={()=>void moveViewer(1)}>›</button></div><div className="viewer-footer">{viewer.asset.path}</div></div>}
    {thumbnailInfoOpen&&<ThumbnailInfoDialog value={thumbnailInfo} onSave={value=>{setThumbnailInfo(normalizeThumbnailInfo(value));setThumbnailInfoOpen(false);}} onClose={()=>setThumbnailInfoOpen(false)}/> }
    {toast&&<div className="app-toast">{toast}</div>}
  </main>;
}

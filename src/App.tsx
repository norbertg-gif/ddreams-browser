import { useCallback, useEffect, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import AssetGrid, { formatBytes, isImage, thumbUrl } from "./components/AssetGrid";
import LeftPanel from "./components/LeftPanel";
import RightPanel from "./components/RightPanel";
import type { Asset, Folder, Page, View } from "./types";
import "./App.css";

const DEFAULT_PATH = String.raw`F:\My_docs\My Pictures\Fotky`;
function dateText(value: string | null | undefined) { return value ? value.replace("T", " ").slice(0, 19) : "—"; }

export default function App() {
  const [view, setView] = useState<View>({ kind: "folder", id: -1, title: "Načítavam katalóg…" });
  const [query, setQuery] = useState("");
  const [queryDebounced, setQueryDebounced] = useState("");
  const [sort, setSort] = useState("name");
  const [descending, setDescending] = useState(false);
  const [zoom, setZoom] = useState(168);
  const [epoch, setEpoch] = useState(1);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [primaryId, setPrimaryId] = useState<number | null>(null);
  const [primaryAsset, setPrimaryAsset] = useState<Asset | null>(null);
  const [total, setTotal] = useState(0);
  const [totalSize, setTotalSize] = useState(0);
  const [viewer, setViewer] = useState<{ asset: Asset; index: number } | null>(null);
  const [error, setError] = useState("");

  useEffect(() => { const timer = window.setTimeout(() => setQueryDebounced(query), 180); return () => window.clearTimeout(timer); }, [query]);
  useEffect(() => { void invoke<Folder | null>("find_folder_path", { path: DEFAULT_PATH }).then(folder => { if (folder) setView({ kind: "folder", id: folder.id, title: folder.path ?? folder.name, folderPath: folder.path ?? undefined }); else setView({ kind: "folder", id: -1, title: DEFAULT_PATH, folderPath: DEFAULT_PATH }); }).catch(e => setError(String(e))); }, []);
  const counts = useCallback((n: number, size: number) => { setTotal(n); setTotalSize(size); }, []);
  const navigate = useCallback((next: View) => { setView(next); setQuery(""); setQueryDebounced(""); setSelectedIds(new Set()); setPrimaryId(null); setPrimaryAsset(null); setViewer(null); }, []);
  const select = useCallback((asset: Asset, _index: number, event: ReactMouseEvent | ReactKeyboardEvent) => {
    setPrimaryId(asset.id); setPrimaryAsset(asset);
    if (event.ctrlKey || event.metaKey) { const next=new Set(selectedIds);if(next.has(asset.id))next.delete(asset.id);else next.add(asset.id);setSelectedIds(next);if(!next.size){setPrimaryId(null);setPrimaryAsset(null)} }
    else if (!event.shiftKey) setSelectedIds(new Set([asset.id]));
  }, [selectedIds]);
  const selectRange = useCallback((ids: number[], asset: Asset, _index: number) => { setSelectedIds(new Set(ids)); setPrimaryId(asset.id); setPrimaryAsset(asset); }, []);
  const selectAll = useCallback(async (ids: number[]) => { setSelectedIds(new Set(ids)); if (ids.length) { const page = await invoke<Page>("get_assets", { filter: { kind: view.kind, id: view.id ?? null, ids: view.ids ?? null, folderPath: view.folderPath ?? null, value: view.value ?? null, query: queryDebounced, sort, descending, offset: 0, limit: 1 } }); const asset=page.items[0];if(asset){setPrimaryId(asset.id);setPrimaryAsset(asset);} } }, [view, queryDebounced, sort, descending]);
  const updateViewport = useCallback(() => { setEpoch(v => { const next=v+1;void invoke("set_thumbnail_epoch",{epoch:next});return next; }); }, []);

  const moveViewer = useCallback(async (delta: number) => {
    if (!viewer) return;
    const index = Math.max(0, Math.min(total - 1, viewer.index + delta));
    const page = await invoke<Page>("get_assets", { filter: { kind: view.kind, id: view.id ?? null, ids: view.ids ?? null, folderPath: view.folderPath ?? null, value: view.value ?? null, query: queryDebounced, sort, descending, offset: index, limit: 1 } });
    const asset=page.items[0]; if (asset) { setViewer({asset,index});setPrimaryId(asset.id);setPrimaryAsset(asset);setSelectedIds(new Set([asset.id])); }
  }, [viewer,total,view,queryDebounced,sort,descending]);
  useEffect(() => {
    if (!viewer) return;
    const key = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") setViewer(null); else if (event.key === "ArrowRight") void moveViewer(1); else if (event.key === "ArrowLeft") void moveViewer(-1); };
    window.addEventListener("keydown",key); return () => window.removeEventListener("keydown",key);
  }, [viewer,moveViewer]);

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
    <header className="topbar"><div className="brand-mark">D</div><div className="brand-title">DDreams <b>browser</b></div><div className="top-separator"/><span className="mode-label">SPRÁVA FOTOGRAFIÍ</span><div className="top-spacer"/><div className="catalog-label"><span className="status-light"/> Katalóg iba na čítanie</div></header>
    <div className="workspace">
      <LeftPanel onNavigate={navigate} activeFolderId={view.kind === "folder" ? view.id ?? null : null} activeFolderIds={view.kind === "folder" ? view.ids : []}/>
      <section className="center-pane">
        <div className="breadcrumb-bar"><span className="crumb-home">⌂</span>{crumbs.map((crumb,i)=><span key={`${crumb}-${i}`} className="crumb-item">{i>0&&<i>›</i>}<button onClick={()=>void openCrumb(i)}>{crumb}</button></span>)}</div>
        <div className="toolbar"><div className="toolbar-left"><label className="search-box"><span>⌕</span><input value={query} onChange={e=>setQuery(e.target.value)} placeholder="Filtrovať v zobrazení…"/><kbd>Ctrl+F</kbd></label></div><div className="toolbar-right"><span className="sort-label">Zoradiť:</span><select value={sort} onChange={e=>setSort(e.target.value)}><option value="name">Názov</option><option value="date">Dátum nasnímania</option><option value="size">Veľkosť</option><option value="type">Typ súboru</option></select><button className="direction-button" title={descending?"Zostupne":"Vzostupne"} onClick={()=>setDescending(v=>!v)}>{descending?"↓":"↑"}</button><div className="zoom-control"><span>−</span><input type="range" min="100" max="260" step="8" value={zoom} onChange={e=>setZoom(Number(e.target.value))}/><span>＋</span></div></div></div>
        {error && <div className="startup-error">Katalóg sa nepodarilo otvoriť: {error}</div>}
        <div className="grid-heading"><span>{view.title}</span><span>{total.toLocaleString("sk-SK")} položiek</span></div>
        <AssetGrid view={view} query={queryDebounced} sort={sort} descending={descending} zoom={zoom} epoch={epoch} selectedIds={selectedIds} primaryId={primaryId} onSelect={select} onSelectRange={selectRange} onSelectAll={ids=>void selectAll(ids)} onOpen={(asset,index)=>{if(asset.isDirectory){const id=asset.folderId??asset.id;navigate({kind:"folder",id,ids:asset.folderId?[asset.folderId]:undefined,title:asset.path??asset.name,folderPath:asset.path??undefined});return;}setViewer({asset,index});setPrimaryId(asset.id);setPrimaryAsset(asset);setSelectedIds(new Set([asset.id]));}} onCounts={counts} onViewport={updateViewport}/>
        <div className="selection-bar">{primaryAsset ? <><div className="selected-name">{selectedIds.size > 1 ? `${selectedIds.size} vybraných` : primaryAsset.name}</div><span>{formatBytes(primaryAsset.size)}</span><span>{dateText(primaryAsset.exifDate)}</span><span>{primaryAsset.width && primaryAsset.height ? `${primaryAsset.width} × ${primaryAsset.height}` : "—"}</span></> : null}<span className="selection-total">Spolu {total.toLocaleString("sk-SK")} položiek · {formatBytes(totalSize)}</span></div>
      </section>
      <RightPanel primaryId={primaryId} selectedIds={[...selectedIds]} onNavigate={navigate}/>
    </div>
    {viewer && <div className="viewer-overlay"><div className="viewer-top"><button onClick={()=>setViewer(null)}>✕ Zavrieť</button><span>{viewer.asset.name}</span><span>{viewer.index+1} / {total}</span></div><div className="viewer-canvas"><button className="viewer-arrow" onClick={()=>void moveViewer(-1)}>‹</button>{isImage(viewer.asset)?<img src={thumbUrl(viewer.asset,0,epoch)} alt={viewer.asset.name}/>:<div className="viewer-nonimage">{viewer.asset.ext.toUpperCase()}<small>Náhľad nie je k dispozícii</small></div>}<button className="viewer-arrow" onClick={()=>void moveViewer(1)}>›</button></div><div className="viewer-footer">{viewer.asset.path}</div></div>}
  </main>;
}

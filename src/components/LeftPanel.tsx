import { useEffect, useRef, useState, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createPortal } from "react-dom";
import type { Folder, View } from "../types";
import CalendarPane from "./CalendarPane";

const shortcuts = [
  { name: "Fotky", path: String.raw`F:\My_docs\My Pictures\Fotky` },
  { name: "Panasonic", path: String.raw`F:\My_docs\My Pictures\Fotky\Panasonic` },
];
type Props = { onNavigate: (view: View) => void; activeFolderId: number | null; activeFolderIds?: number[]; activeView?: View; refreshToken?:number; refreshPaths?:string[]; onFileDrop?:(ids:number[],path:string,copy:boolean)=>void; onCreateFolder?:(path:string)=>void };

export default function LeftPanel({ onNavigate, activeFolderId, activeFolderIds = [], activeView, refreshToken=0, refreshPaths=[], onFileDrop, onCreateFolder }: Props) {
  const [tab, setTab] = useState<"folders" | "calendar">("folders");
  const [roots, setRoots] = useState<Folder[]>([]);
  const [expanded, setExpanded] = useState(new Set<number>());
  const [children, setChildren] = useState(new Map<number, Folder[]>());
  const childrenRef = useRef(children);
  const [loading, setLoading] = useState(new Set<number>());
  const [error, setError] = useState("");
  const rowRefs = useRef(new Map<number, HTMLDivElement>());
  const lastEnsuredFolder = useRef<number | null>(null);
  useEffect(() => { childrenRef.current = children; }, [children]);

  useEffect(() => { void invoke<Folder[]>("get_folder_children", { parentId: null, parentIds: null, offset: 0, limit: 500 }).then(setRoots).catch(e => setError(String(e))); }, []);
  useEffect(()=>{if(!refreshToken)return;let cancelled=false;void(async()=>{const nextRoots=await invoke<Folder[]>("get_folder_children",{parentId:null,parentIds:null,offset:0,limit:500}).catch(()=>null);if(!nextRoots||cancelled)return;setRoots(nextRoots);const wanted=new Set(refreshPaths.map(path=>path.split("/").join("\\").replace(/\\+$/ ,"").toLowerCase()));const known=new Map<number,Folder>();const collect=(nodes:Folder[])=>{for(const node of nodes){known.set(node.id,node);const nested=childrenRef.current.get(node.id);if(nested)collect(nested);}};collect(nextRoots);for(const nodes of childrenRef.current.values())collect(nodes);for(const id of expanded){const node=known.get(id);if(!node)continue;const nodePath=(node.path??"").split("/").join("\\").replace(/\\+$/ ,"").toLowerCase();if(wanted.size&&!wanted.has(nodePath))continue;const refreshed=await invoke<Folder[]>("get_folder_children",{parentId:node.virtualNode?null:node.id,parentIds:node.folderIds?.length?node.folderIds:null,offset:0,limit:500}).catch(()=>null);if(cancelled) return;if(refreshed){setChildren(previous=>{const next=new Map(previous).set(id,refreshed);childrenRef.current=next;return next;});}}})();return()=>{cancelled=true};},[refreshToken]);
  const navigate = (folder: Folder) => {
    if (folder.virtualNode) return;
    const ids = folder.folderIds?.length ? folder.folderIds : [folder.id];
    onNavigate({ kind: "folder", id: folder.id, ids, title: folder.path ?? folder.name, folderPath: folder.path ?? undefined });
  };
  const loadChildren = async (folder: Folder) => {
    if (childrenRef.current.has(folder.id)) return childrenRef.current.get(folder.id) ?? [];
    setLoading(prev => new Set(prev).add(folder.id));
    try {
      const items = await invoke<Folder[]>("get_folder_children", { parentId: folder.virtualNode ? null : folder.id, parentIds: folder.folderIds?.length ? folder.folderIds : null, offset: 0, limit: 500 });
      setChildren(prev => new Map(prev).set(folder.id, items));
      childrenRef.current = new Map(childrenRef.current).set(folder.id, items);
      return items;
    } catch (e) { setError(String(e)); return []; }
    finally { setLoading(prev => { const next = new Set(prev); next.delete(folder.id); return next; }); }
  };
  const toggle = async (folder: Folder) => {
    if (expanded.has(folder.id)) { setExpanded(prev => { const next = new Set(prev); next.delete(folder.id); return next; }); return; }
    setExpanded(prev => new Set(prev).add(folder.id));
    await loadChildren(folder);
  };
  const clickShortcut = async (path: string) => {
    try { const folder = await invoke<Folder | null>("find_folder_path", { path }); if (folder) navigate(folder); else setError(`Cesta sa v katalógu nenašla: ${path}`); }
    catch (e) { setError(String(e)); }
  };

  useEffect(() => {
    if (activeFolderId == null) { lastEnsuredFolder.current=null; return; }
    if (lastEnsuredFolder.current === activeFolderId) return;
    let cancelled = false;
    setTab("folders");
    void (async () => {
      const ancestors = await invoke<Folder[]>("get_folder_ancestors", { id: activeFolderId }).catch(() => []);
      if (cancelled || !ancestors.length) return;
      let level = roots;
      for (const ancestor of ancestors) {
        if (cancelled) return;
        const node = level.find(item => (item.folderIds ?? [item.id]).includes(ancestor.id));
        if (!node) { setError(`Tree path is missing ancestor: ${ancestor.name}`); return; }
        setExpanded(prev => new Set(prev).add(node.id));
        level = await loadChildren(node);
        if (cancelled) return;
      }
      lastEnsuredFolder.current=activeFolderId;
      window.requestAnimationFrame(() => window.requestAnimationFrame(() => rowRefs.current.get(activeFolderId)?.scrollIntoView({ block: "nearest" })));
    })();
    return () => { cancelled = true; };
  }, [activeFolderId, roots]);

  const activeSet = new Set([activeFolderId, ...activeFolderIds].filter((v): v is number => v != null));
  return <aside className="left-pane">
    <section className="shortcuts-pane"><div className="pane-title">SKRATKY</div>{shortcuts.map(s => <button className="shortcut" key={s.path} onClick={() => void clickShortcut(s.path)}><span>▣</span><span><b>{s.name}</b><small>{s.path}</small></span></button>)}</section>
    <div className="left-tabs"><button className={tab === "folders" ? "active" : ""} onClick={() => setTab("folders")}>Priečinky</button><button className={tab === "calendar" ? "active" : ""} onClick={() => setTab("calendar")}>Kalendár</button></div>
    {tab === "calendar" ? <CalendarPane onNavigate={onNavigate} activeView={activeView} refreshToken={refreshToken}/> : <div className="folder-tree">
      {error && <div className="inline-error">{error}<button onClick={() => setError("")}>×</button></div>}
      {roots.map(folder => <FolderRow key={folder.id} folder={folder} depth={0} activeIds={activeSet} expanded={expanded} childrenMap={children} loading={loading.has(folder.id)} rowRefs={rowRefs} onToggle={toggle} onNavigate={navigate} onFileDrop={onFileDrop} onCreateFolder={onCreateFolder} />)}
    </div>}
  </aside>;
}

function FolderRow({ folder, depth, activeIds, expanded, childrenMap, loading, rowRefs, onToggle, onNavigate, onFileDrop, onCreateFolder }: { folder: Folder; depth: number; activeIds: Set<number>; expanded: Set<number>; childrenMap: Map<number, Folder[]>; loading: boolean; rowRefs: MutableRefObject<Map<number, HTMLDivElement>>; onToggle: (folder: Folder) => void; onNavigate: (folder: Folder) => void; onFileDrop?: (ids:number[],path:string,copy:boolean)=>void; onCreateFolder?:(path:string)=>void }) {
  const open = expanded.has(folder.id); const nodes = childrenMap.get(folder.id) ?? [];
  const [dropTarget,setDropTarget]=useState(false); const [folderMenu,setFolderMenu]=useState<{x:number;y:number}|null>(null); const hoverTimer=useRef<number|null>(null);
  return <div>
    <div ref={el => { if (el) for (const id of folder.folderIds ?? [folder.id]) rowRefs.current.set(id, el); }} className={`tree-row folder-row ${dropTarget?"drop-target":""} ${(folder.folderIds ?? [folder.id]).some(id => activeIds.has(id)) ? "active" : ""}`} style={{ paddingLeft: 8 + depth * 14 }} onDragOver={e=>{if(!e.dataTransfer.types.includes("application/x-ddreams-asset-ids"))return;e.preventDefault();e.dataTransfer.dropEffect=e.ctrlKey?"copy":"move";setDropTarget(true);if(!open&&hoverTimer.current==null)hoverTimer.current=window.setTimeout(()=>{hoverTimer.current=null;void onToggle(folder)},700)}} onDragLeave={()=>{setDropTarget(false);if(hoverTimer.current!=null){clearTimeout(hoverTimer.current);hoverTimer.current=null;}}} onDrop={e=>{const raw=e.dataTransfer.getData("application/x-ddreams-asset-ids");if(!raw)return;e.preventDefault();e.stopPropagation();setDropTarget(false);if(hoverTimer.current!=null)clearTimeout(hoverTimer.current);try{const ids=JSON.parse(raw) as number[];if(ids.length&&folder.path)onFileDrop?.(ids,folder.path,e.ctrlKey)}catch{}}} onContextMenu={e=>{e.preventDefault();if(onCreateFolder&&folder.path)setFolderMenu({x:e.clientX,y:e.clientY})}}>
      <button className="tree-toggle" aria-label={open ? "Zbaliť" : "Rozbaliť"} onClick={() => void onToggle(folder)}>{loading ? "·" : open ? "▾" : "▸"}</button>
      <button className="tree-label" onClick={() => onNavigate(folder)} title={folder.path ?? folder.name}><span className="folder-icon">▰</span><span className="tree-text">{folder.name || "(bez názvu)"}</span><small>{folder.offline ? "OFFLINE" : depth === 0 ? folder.count || "" : ""}</small></button>
    </div>
    {folderMenu&&createPortal(<div className="category-context-menu" style={{left:Math.min(folderMenu.x,window.innerWidth-240),top:Math.min(folderMenu.y,window.innerHeight-80)}} onMouseLeave={()=>setFolderMenu(null)}><button onClick={()=>{setFolderMenu(null);if(folder.path)onCreateFolder?.(folder.path)}}>Nový priečinok…</button></div>,document.body)}
    {open && nodes.map(child => <FolderRow key={child.id} folder={child} depth={depth + 1} activeIds={activeIds} expanded={expanded} childrenMap={childrenMap} loading={false} rowRefs={rowRefs} onToggle={onToggle} onNavigate={onNavigate} onFileDrop={onFileDrop} onCreateFolder={onCreateFolder} />)}
  </div>;
}

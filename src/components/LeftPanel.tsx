import { useContext, useEffect, useRef, useState, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createPortal } from "react-dom";
import type { Folder, View } from "../types";
import CalendarPane from "./CalendarPane";
import { FolderRulesContext, effectiveRule, useFolderRules, type FolderRuleAction } from "../folderRules";
import Splitter from "./Splitter";
import { readStored, writeStored } from "../storage";
import { useSpringOpen } from "../hooks/useSpringOpen";

type Shortcut = { name: string; path: string };
const DEFAULT_SHORTCUTS: Shortcut[] = [
  { name: "Fotky", path: String.raw`F:\My_docs\My Pictures\Fotky` },
  { name: "Panasonic", path: String.raw`F:\My_docs\My Pictures\Fotky\Panasonic` },
];
type Props = { onNavigate: (view: View) => void; activeFolderId: number | null; activeFolderIds?: number[]; activeView?: View; refreshToken?:number; refreshPaths?:string[]; onFileDrop?:(ids:number[],path:string,copy:boolean)=>void; onCreateFolder?:(path:string)=>void ; onFolderRule?: FolderRuleAction };

export default function LeftPanel({ onNavigate, activeFolderId, activeFolderIds = [], activeView, refreshToken=0, refreshPaths=[], onFileDrop, onCreateFolder, onFolderRule }: Props) {
  const folderRules = useFolderRules();
  const [tab, setTab] = useState<"folders" | "calendar">("folders");
  const [shortcuts, setShortcuts] = useState<Shortcut[]>(() => { const v = readStored<Shortcut[]>("ddreams.shortcuts", DEFAULT_SHORTCUTS); return Array.isArray(v) ? v : DEFAULT_SHORTCUTS; });
  useEffect(() => { writeStored("ddreams.shortcuts", shortcuts); }, [shortcuts]);
  const currentPath = activeView?.kind === "folder" ? (activeView.folderPath ?? "") : "";
  const samePath = (x: string, y: string) => x.replace(/[\\/]+$/, "").toLowerCase() === y.replace(/[\\/]+$/, "").toLowerCase();
  const addShortcut = () => { if (!currentPath || shortcuts.some(s => samePath(s.path, currentPath))) return; const name = currentPath.split(/[\\/]/).filter(Boolean).pop() ?? currentPath; setShortcuts([...shortcuts, { name, path: currentPath }]); };
  const [shortcutsHeight, setShortcutsHeight] = useState<number | null>(() => readStored<number | null>("ddreams.shortcutsHeight", null));
  const shortcutsRef = useRef<HTMLElement | null>(null);
  const paneRef = useRef<HTMLElement | null>(null);
  useEffect(() => { writeStored("ddreams.shortcutsHeight", shortcutsHeight); }, [shortcutsHeight]);
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
  return <FolderRulesContext.Provider value={{ rules: folderRules, onRule: onFolderRule }}><aside className="left-pane" ref={paneRef}>
    <section className="shortcuts-pane" ref={shortcutsRef} style={shortcutsHeight == null ? undefined : { height: shortcutsHeight, flex: "none" }}><div className="pane-title">Skratky<button className="shortcut-add" disabled={!currentPath||shortcuts.some(s=>samePath(s.path,currentPath))} title={currentPath?`Pridať do skratiek: ${currentPath}`:"Otvor priečinok, ktorý chceš pridať"} onClick={addShortcut}>+</button></div>{shortcuts.length===0&&<small className="color-hint">Otvor priečinok a klikni na +</small>}{shortcuts.map(s => <div className="shortcut-row" key={s.path}><button className="shortcut" onClick={() => void clickShortcut(s.path)}><span className="folder-glyph"/><span title={s.path}><b>{s.name}</b></span></button><button className="shortcut-remove" title={`Odstrániť skratku ${s.name} (priečinok ostane)`} onClick={()=>setShortcuts(shortcuts.filter(x=>x.path!==s.path))}>×</button></div>)}</section>
    <Splitter direction="y" value={() => shortcutsHeight ?? shortcutsRef.current?.offsetHeight ?? 120} min={36} max={Math.max(36, (paneRef.current?.offsetHeight ?? 600) - 160)} onChange={setShortcutsHeight} onReset={() => setShortcutsHeight(null)}/>
    <div className="left-tabs"><button className={tab === "folders" ? "active" : ""} onClick={() => setTab("folders")}>Priečinky</button><button className={tab === "calendar" ? "active" : ""} onClick={() => setTab("calendar")}>Kalendár</button></div>
    {tab === "calendar" ? <CalendarPane onNavigate={onNavigate} activeView={activeView} refreshToken={refreshToken}/> : <div className="folder-tree">
      {error && <div className="inline-error">{error}<button onClick={() => setError("")}>×</button></div>}
      {roots.map(folder => <FolderRow key={folder.id} folder={folder} depth={0} activeIds={activeSet} expanded={expanded} childrenMap={children} loading={loading.has(folder.id)} rowRefs={rowRefs} onToggle={toggle} onNavigate={navigate} onFileDrop={onFileDrop} onCreateFolder={onCreateFolder} />)}
    </div>}
  </aside></FolderRulesContext.Provider>;
}

function FolderRow({ folder, depth, activeIds, expanded, childrenMap, loading, rowRefs, onToggle, onNavigate, onFileDrop, onCreateFolder }: { folder: Folder; depth: number; activeIds: Set<number>; expanded: Set<number>; childrenMap: Map<number, Folder[]>; loading: boolean; rowRefs: MutableRefObject<Map<number, HTMLDivElement>>; onToggle: (folder: Folder) => void; onNavigate: (folder: Folder) => void; onFileDrop?: (ids:number[],path:string,copy:boolean)=>void; onCreateFolder?:(path:string)=>void }) {
  const open = expanded.has(folder.id); const nodes = childrenMap.get(folder.id) ?? []; const spring=useSpringOpen(open,true,()=>{void onToggle(folder)});
  const [dropTarget,setDropTarget]=useState(false); const [folderMenu,setFolderMenu]=useState<{x:number;y:number}|null>(null);
  const { rules, onRule } = useContext(FolderRulesContext); const rule = effectiveRule(rules, folder.path); const explicit = rule && folder.path && rule.path.replace(/[\\/]+$/,"").toLocaleLowerCase("sk") === folder.path.replace(/[\\/]+$/,"").toLocaleLowerCase("sk") ? rule : null;
  return <div>
    <div ref={el => { if (el) for (const id of folder.folderIds ?? [folder.id]) rowRefs.current.set(id, el); }} className={`tree-row folder-row ${dropTarget?"drop-target":""} ${(folder.folderIds ?? [folder.id]).some(id => activeIds.has(id)) ? "active" : ""}`} style={{ paddingLeft: 8 + depth * 14 }} onDragEnter={spring.onDragEnter} onDragOver={e=>{spring.onDragOver(e);if(!e.dataTransfer.types.includes("application/x-ddreams-asset-ids"))return;e.preventDefault();e.dataTransfer.dropEffect=e.ctrlKey?"copy":"move";setDropTarget(true)}} onDragLeave={e=>{spring.onDragLeave(e);setDropTarget(false)}} onDrop={e=>{spring.onDrop();const raw=e.dataTransfer.getData("application/x-ddreams-asset-ids");if(!raw)return;e.preventDefault();e.stopPropagation();setDropTarget(false);try{const ids=JSON.parse(raw) as number[];if(ids.length&&folder.path)onFileDrop?.(ids,folder.path,e.ctrlKey)}catch{}}} onContextMenu={e=>{e.preventDefault();if((onCreateFolder||onRule)&&folder.path)setFolderMenu({x:e.clientX,y:e.clientY})}}>
      <button className="tree-toggle" aria-label={open ? "Zbaliť" : "Rozbaliť"} onClick={() => void onToggle(folder)}>{loading ? "·" : open ? "▾" : "▸"}</button>
      <button className="tree-label" onClick={() => onNavigate(folder)} title={folder.path ?? folder.name}><span className={depth===0?"drive-glyph":"folder-glyph"}/><span className="tree-text">{folder.name || "(bez názvu)"}</span>{rule&&<span className={`folder-rule-mark ${rule.mode} ${explicit?"explicit":"inherited"}`} title={`${rule.mode==="include"?"Zahrnutý v katalógu":"Vylúčený z katalógu"}${explicit?"":` (podľa ${rule.path})`}`}>{rule.mode==="include"?"●":"⊘"}</span>}<small>{folder.offline ? "OFFLINE" : depth === 0 ? folder.count || "" : ""}</small></button>
    </div>
    {folderMenu&&createPortal(<div className="category-context-menu" style={{left:Math.min(folderMenu.x,window.innerWidth-240),top:Math.min(folderMenu.y,window.innerHeight-140)}} onMouseLeave={()=>setFolderMenu(null)}>{onCreateFolder&&<button onClick={()=>{setFolderMenu(null);if(folder.path)onCreateFolder?.(folder.path)}}>Nový priečinok…</button>}{onRule&&folder.path&&<>{explicit?.mode!=="include"&&<button onClick={()=>{setFolderMenu(null);onRule(folder.path!,"include")}}>Zahrnúť do katalógu</button>}{explicit?.mode!=="exclude"&&<button onClick={()=>{setFolderMenu(null);onRule(folder.path!,"exclude")}}>Vylúčiť z katalógu…</button>}{explicit&&<button onClick={()=>{setFolderMenu(null);onRule(folder.path!,null)}}>Zrušiť pravidlo</button>}</>}</div>,document.body)}
    {open && nodes.map(child => <FolderRow key={child.id} folder={child} depth={depth + 1} activeIds={activeIds} expanded={expanded} childrenMap={childrenMap} loading={false} rowRefs={rowRefs} onToggle={onToggle} onNavigate={onNavigate} onFileDrop={onFileDrop} onCreateFolder={onCreateFolder} />)}
  </div>;
}

import { useCallback, useContext, useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createPortal } from "react-dom";
import type { Folder, View } from "../types";
import CalendarPane from "./CalendarPane";
import { FOLDER_RULES_EVENT, FolderRulesContext, type FolderCatalogueState, type FolderRuleAction } from "../folderRules";
import Splitter from "./Splitter";
import { readStored, writeStored } from "../storage";
import { expandedNodes, folderTreeKey, toggleSetItem, usePersistentSet } from "../uiState";
import { useSpringOpen } from "../hooks/useSpringOpen";

type Shortcut = { name: string; path: string };
const DEFAULT_SHORTCUTS: Shortcut[] = [];
type Props = { hosts: { folders: HTMLElement; calendar: HTMLElement }; onNavigate: (view: View) => void; activeFolderId: number | null; activeFolderIds?: number[]; activeView?: View; refreshToken?:number; calendarRefreshToken:number; refreshPaths?:string[]; onFileDrop?:(ids:number[],path:string,copy:boolean)=>void; onCreateFolder?:(path:string)=>void ; onFolderRule?: FolderRuleAction; onToast?:(message:string)=>void; onFolderAction?:(action:"reveal"|"copyPath"|"rename"|"delete"|"cut"|"copy"|"paste",path:string)=>void; clipboardPath?:string|null; clipboardAvailable?:boolean };

export default function LeftPanel({ hosts, onNavigate, activeFolderId, activeFolderIds = [], activeView, refreshToken=0, calendarRefreshToken, refreshPaths=[], onFileDrop, onCreateFolder, onFolderRule, onToast, onFolderAction, clipboardPath=null, clipboardAvailable=false }: Props) {
  const [shortcuts, setShortcuts] = useState<Shortcut[]>(() => { const v = readStored<Shortcut[]>("ddreams.shortcuts", DEFAULT_SHORTCUTS); return Array.isArray(v) ? v : DEFAULT_SHORTCUTS; });
  useEffect(() => { writeStored("ddreams.shortcuts", shortcuts);void Promise.all(shortcuts.map(async item=>[item.path,!(await invoke<boolean>("folder_exists",{path:item.path}).catch(()=>false))] as const)).then(rows=>setMissingShortcuts(new Set(rows.filter(row=>row[1]).map(row=>row[0].toLowerCase())))); }, [shortcuts]);
  const currentPath = activeView?.kind === "folder" ? (activeView.folderPath ?? "") : "";
  const samePath = (x: string, y: string) => x.replace(/[\\/]+$/, "").toLowerCase() === y.replace(/[\\/]+$/, "").toLowerCase();
  const addShortcutPath = (path: string) => setShortcuts(prev => { if (!path || prev.some(s => samePath(s.path, path))) return prev; const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path; return [...prev, { name, path }]; });
  const addShortcut = () => addShortcutPath(currentPath);
  useEffect(() => { const h = (e: Event) => addShortcutPath((e as CustomEvent<string>).detail); window.addEventListener("ddreams-add-shortcut", h); return () => window.removeEventListener("ddreams-add-shortcut", h); }, []);
  useEffect(()=>{const changed=(event:Event)=>{const detail=(event as CustomEvent<{oldPath:string;newPath:string}>).detail;if(!detail)return;const old=detail.oldPath.replace(/[\\/]+$/g,"").toLowerCase();setShortcuts(items=>items.map(item=>{const key=item.path.replace(/[\\/]+$/g,"").toLowerCase();if(key!==old&&!key.startsWith(old+"\\"))return item;const suffix=item.path.slice(detail.oldPath.length);const path=detail.newPath+suffix;return {...item,path,name:key===old?detail.newPath.split(/[\\/]/).filter(Boolean).pop()??item.name:item.name}}))};window.addEventListener("ddreams-folder-path-changed",changed);return()=>window.removeEventListener("ddreams-folder-path-changed",changed)},[]);
  const [shortcutMenu, setShortcutMenu] = useState<{ x: number; y: number; path: string } | null>(null);
  const [renaming, setRenaming] = useState<{ path: string; name: string } | null>(null);
  const commitRename = () => { if (!renaming) return; const name = renaming.name.trim(); if (name) setShortcuts(prev => prev.map(s => s.path === renaming.path ? { ...s, name } : s)); setRenaming(null); };
  const moveShortcut = (path: string, delta: number) => setShortcuts(prev => { const i = prev.findIndex(s => s.path === path); const j = i + delta; if (i < 0 || j < 0 || j >= prev.length) return prev; const next = [...prev]; [next[i], next[j]] = [next[j], next[i]]; return next; });
  const [shortcutsHeight, setShortcutsHeight] = useState<number | null>(() => readStored<number | null>("ddreams.shortcutsHeight", null));
  const shortcutsRef = useRef<HTMLElement | null>(null);
  const paneRef = useRef<HTMLElement | null>(null);
  useEffect(() => { writeStored("ddreams.shortcutsHeight", shortcutsHeight); }, [shortcutsHeight]);
  const [roots, setRoots] = useState<Folder[]>([]);
  const [missingShortcuts,setMissingShortcuts]=useState(new Set<string>());
  const [expanded, setExpanded] = usePersistentSet<string>("ddreams.ui.folders.expanded", "");
  const [children, setChildren] = useState(new Map<number, Folder[]>());
  const childrenRef = useRef(children);
  const pendingChildren = useRef(new Map<number, Promise<Folder[]>>());
  const failedChildren = useRef(new Set<number>());
  const [loading, setLoading] = useState(new Set<number>());
  const [error, setError] = useState("");
  const rowRefs = useRef(new Map<number, HTMLDivElement>());
  const lastEnsuredFolder = useRef<number | null>(null);
  const storeChildren = useCallback((id: number, nodes: Folder[]) => {
    childrenRef.current = new Map(childrenRef.current).set(id, nodes);
    setChildren(childrenRef.current);
  }, []);

  useEffect(() => { void invoke<Folder[]>("get_folder_children", { parentId: null, parentIds: null, offset: 0, limit: 500 }).then(setRoots).catch(e => setError(String(e))); }, []);
  useEffect(()=>{if(!refreshToken)return;let cancelled=false;void(async()=>{const nextRoots=await invoke<Folder[]>("get_folder_children",{parentId:null,parentIds:null,offset:0,limit:500}).catch(()=>null);if(!nextRoots||cancelled)return;setRoots(nextRoots);const wanted=new Set(refreshPaths.flatMap(path=>{const p=path.split("/").join("\\").replace(/\\+$/ ,"").toLowerCase();const i=p.lastIndexOf("\\");return i>0?[p,p.slice(0,i)]:[p];}));const known=new Map<number,Folder>();const collect=(nodes:Folder[])=>{for(const node of nodes){known.set(node.id,node);const nested=childrenRef.current.get(node.id);if(nested)collect(nested);}};collect(nextRoots);for(const nodes of childrenRef.current.values())collect(nodes);for(const [id,node] of known){if(!expanded.has(folderTreeKey(node)))continue;const nodePath=(node.path??"").split("/").join("\\").replace(/\\+$/ ,"").toLowerCase();if(wanted.size&&!wanted.has(nodePath))continue;const pending=pendingChildren.current.get(id);if(pending)await pending;if(cancelled)return;const refreshed=await invoke<Folder[]>("get_folder_children",{parentId:node.virtualNode?null:node.id,parentIds:node.folderIds?.length?node.folderIds:null,offset:0,limit:500}).catch(()=>null);if(cancelled) return;if(refreshed){storeChildren(id,refreshed);}}})();return()=>{cancelled=true};},[refreshToken]);
  const navigate = (folder: Folder) => {
    if (folder.virtualNode) return;
    const ids = folder.folderIds?.length ? folder.folderIds : [folder.id];
    onNavigate({ kind: "folder", id: folder.id, ids, title: folder.path ?? folder.name, folderPath: folder.path ?? undefined });
  };
  const loadChildren = useCallback((folder: Folder): Promise<Folder[]> => {
    const cached = childrenRef.current.get(folder.id);
    if (cached) return Promise.resolve(cached);
    const pending = pendingChildren.current.get(folder.id);
    if (pending) return pending;

    setLoading(previous => new Set(previous).add(folder.id));
    const request = invoke<Folder[]>("get_folder_children", {
      parentId: folder.virtualNode ? null : folder.id, parentIds: folder.folderIds?.length ? folder.folderIds : null,
      offset: 0, limit: 500,
    }).then(nodes => {
      storeChildren(folder.id, nodes);
      return nodes;
    }).catch(error => {
      failedChildren.current.add(folder.id);
      setError(String(error));
      return [];
    }).finally(() => {
      pendingChildren.current.delete(folder.id);
      setLoading(previous => {
        const next = new Set(previous);
        next.delete(folder.id);
        return next;
      });
    });
    pendingChildren.current.set(folder.id, request);
    return request;
  }, [storeChildren]);

  const openFolders = useMemo(() => [...expandedNodes(roots,
    folder => expanded.has(folderTreeKey(folder)), folder => children.get(folder.id) ?? [],
  )], [roots, expanded, children]);
  useEffect(() => {
    for (const folder of openFolders) {
      if (!children.has(folder.id) && !failedChildren.current.has(folder.id)) void loadChildren(folder);
    }
  }, [openFolders, children, loadChildren]);

  const toggle = (folder: Folder) => {
    failedChildren.current.delete(folder.id);
    setExpanded(previous => toggleSetItem(previous, folderTreeKey(folder)));
  };
  const visiblePaths = useMemo(() => [...new Set(
    [...roots, ...openFolders.flatMap(folder => children.get(folder.id) ?? [])]
      .flatMap(folder => folder.path && !folder.virtualNode ? [folder.path] : []),
  )], [roots, openFolders, children]);
  const [folderStates, setFolderStates] = useState<Record<string, FolderCatalogueState>>({});
  const [autoCatalogueRevision, setAutoCatalogueRevision] = useState(0);
  useEffect(() => {
    let active = true;
    const refresh = () => { if (active) setAutoCatalogueRevision(value => value + 1); };
    const subscription = listen("folder-catalogue-changed", refresh);
    void subscription.then(refresh).catch(() => {});
    return () => {
      active = false;
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, []);
  useEffect(() => {
    let latest = 0;
    let timer: number | undefined;
    const load = () => {
      const request = ++latest;
      window.clearTimeout(timer);
      if (!visiblePaths.length) return;
      // Restoring a level can reveal several batches of children in quick succession.
      timer = window.setTimeout(() => {
        void invoke<Record<string, FolderCatalogueState>>("get_folder_catalogue_states", { paths: visiblePaths })
          .then(states => { if (request === latest) setFolderStates(states); }).catch(() => {});
      }, 40);
    };
    load();
    window.addEventListener(FOLDER_RULES_EVENT, load);
    window.addEventListener("ddreams-folder-path-changed", load);
    return () => {
      latest++;
      window.clearTimeout(timer);
      window.removeEventListener(FOLDER_RULES_EVENT, load);
      window.removeEventListener("ddreams-folder-path-changed", load);
    };
  }, [visiblePaths, calendarRefreshToken, autoCatalogueRevision]);

  const clickShortcut = async (path: string) => {
    try { if(!await invoke<boolean>("folder_exists",{path})){onToast?.("Priečinok neexistuje alebo disk nie je pripojený");return;}const folder = await invoke<Folder | null>("find_folder_path", { path }); if (folder) navigate(folder); else onToast?.("Priečinok neexistuje alebo disk nie je pripojený"); }
    catch (e) { onToast?.(String(e)); }
  };

  useEffect(() => {
    if (activeFolderId == null) { lastEnsuredFolder.current=null; return; }
    if (lastEnsuredFolder.current === activeFolderId) return;
    let cancelled = false;
    void (async () => {
      const ancestors = await invoke<Folder[]>("get_folder_ancestors", { id: activeFolderId }).catch(() => []);
      if (cancelled || !ancestors.length) return;
      let level = roots;
      for (const ancestor of ancestors) {
        if (cancelled) return;
        const norm = (v?: string | null) => (v ?? "").replace(/[\\/]+$/, "").toLowerCase();
        const node = level.find(item => (item.folderIds ?? [item.id]).includes(ancestor.id)) ?? (ancestor.path ? level.find(item => norm(item.path) === norm(ancestor.path)) : undefined);
        // The tree shows only folders that exist on disk; catalogue-only ancestors (e.g. a synthetic "\\" root) are skipped.
        if (!node) { if (level === roots) continue; return; }
        setExpanded(prev => prev.has(folderTreeKey(node)) ? prev : new Set(prev).add(folderTreeKey(node)));
        level = await loadChildren(node);
        if (cancelled) return;
      }
      lastEnsuredFolder.current=activeFolderId;
      window.requestAnimationFrame(() => window.requestAnimationFrame(() => rowRefs.current.get(activeFolderId)?.scrollIntoView({ block: "nearest" })));
    })();
    return () => { cancelled = true; };
  }, [activeFolderId, roots]);

  const activeSet = new Set([activeFolderId, ...activeFolderIds].filter((v): v is number => v != null));
  return <FolderRulesContext.Provider value={{ states: folderStates, onRule: onFolderRule }}>{createPortal(<aside className="left-pane" ref={paneRef}>
    <section className="shortcuts-pane" ref={shortcutsRef} style={shortcutsHeight == null ? undefined : { height: shortcutsHeight, flex: "none" }}><div className="pane-title">Skratky<button className="shortcut-add" disabled={!currentPath||shortcuts.some(s=>samePath(s.path,currentPath))} title={currentPath?`Pridať do skratiek: ${currentPath}`:"Otvor priečinok, ktorý chceš pridať"} onClick={addShortcut}>+</button></div>{shortcuts.length===0&&<small className="color-hint">Otvor priečinok a klikni na +</small>}{shortcuts.map(s => <div className="shortcut-row" key={s.path} onContextMenu={e=>{e.preventDefault();setShortcutMenu({x:e.clientX,y:e.clientY,path:s.path})}}>{renaming?.path===s.path?<input className="shortcut-rename" autoFocus value={renaming.name} onFocus={e=>e.currentTarget.select()} onChange={e=>setRenaming({path:s.path,name:e.target.value})} onBlur={commitRename} onKeyDown={e=>{if(e.key==="Enter")commitRename();else if(e.key==="Escape")setRenaming(null);e.stopPropagation()}}/>:<button className={`shortcut ${missingShortcuts.has(s.path.toLowerCase())?"shortcut-missing":""}`} onClick={() => void clickShortcut(s.path)} onDoubleClick={()=>setRenaming({path:s.path,name:s.name})}><span className="folder-glyph"/><span title={missingShortcuts.has(s.path.toLowerCase())?"Priečinok neexistuje":`${s.path}\nPravý klik: premenovať, presú, odstrániť`}><b>{s.name}</b></span></button>}<button className="shortcut-remove" title={`Odstrániť skratku ${s.name} (priečinok ostane)`} onClick={()=>setShortcuts(shortcuts.filter(x=>x.path!==s.path))}>×</button></div>)}</section>
    {shortcutMenu&&(()=>{const m=shortcutMenu;const i=shortcuts.findIndex(x=>x.path===m.path);const sc=shortcuts[i];if(!sc)return null;const close=()=>setShortcutMenu(null);return createPortal(<div className="category-context-menu" style={{left:Math.min(m.x,window.innerWidth-240),top:Math.min(m.y,window.innerHeight-150)}} onMouseLeave={close}><button onClick={()=>{close();setRenaming({path:sc.path,name:sc.name})}}>Premenovať…</button><button disabled={i===0} onClick={()=>{close();moveShortcut(sc.path,-1)}}>Posunúť hore</button><button disabled={i===shortcuts.length-1} onClick={()=>{close();moveShortcut(sc.path,1)}}>Posunúť dole</button><button onClick={()=>{close();setShortcuts(prev=>prev.filter(x=>x.path!==sc.path))}}>Odstrániť zo skratiek</button></div>,document.body)})()}
    <Splitter direction="y" value={() => shortcutsHeight ?? shortcutsRef.current?.offsetHeight ?? 120} min={36} max={Math.max(36, (paneRef.current?.offsetHeight ?? 600) - 160)} onChange={setShortcutsHeight} onReset={() => setShortcutsHeight(null)}/>
    <div className="folder-tree">
      {error && <div className="inline-error">{error}<button onClick={() => setError("")}>×</button></div>}
      {roots.map(folder => <FolderRow key={folder.id} folder={folder} depth={0} activeIds={activeSet} expanded={expanded} childrenMap={children} loading={loading} rowRefs={rowRefs} onToggle={toggle} onNavigate={navigate} onFileDrop={onFileDrop} onCreateFolder={onCreateFolder} onFolderAction={onFolderAction} clipboardPath={clipboardPath} clipboardAvailable={clipboardAvailable} />)}
    </div>
  </aside>, hosts.folders)}{createPortal(<CalendarPane onNavigate={onNavigate} activeView={activeView} refreshToken={calendarRefreshToken}/>, hosts.calendar)}</FolderRulesContext.Provider>;
}

function FolderRow({ folder, depth, activeIds, expanded, childrenMap, loading, rowRefs, onToggle, onNavigate, onFileDrop, onCreateFolder, onFolderAction, clipboardPath, clipboardAvailable }: { folder: Folder; depth: number; activeIds: Set<number>; expanded: Set<string>; childrenMap: Map<number, Folder[]>; loading: Set<number>; rowRefs: MutableRefObject<Map<number, HTMLDivElement>>; onToggle: (folder: Folder) => void; onNavigate: (folder: Folder) => void; onFileDrop?: (ids:number[],path:string,copy:boolean)=>void; onCreateFolder?:(path:string)=>void; onFolderAction?:(action:"reveal"|"copyPath"|"rename"|"delete"|"cut"|"copy"|"paste",path:string)=>void; clipboardPath?:string|null; clipboardAvailable?:boolean }) {
  const open = expanded.has(folderTreeKey(folder)); const nodes = childrenMap.get(folder.id) ?? []; const spring=useSpringOpen(open,true,()=>{void onToggle(folder)});
  const [dropTarget,setDropTarget]=useState(false); const [folderMenu,setFolderMenu]=useState<{x:number;y:number}|null>(null);
  const { states, onRule } = useContext(FolderRulesContext);
  const rule = folder.path ? states[folder.path] : undefined;
  const listedCount = rule?.listedCount ?? 0;
  const directlyCatalogued = listedCount > 0 || rule?.explicitMode === "include";
  const hasDot = rule?.effective !== "exclude"
    && (directlyCatalogued || rule?.hasCataloguedDescendant || rule?.partiallyExcluded);
  const catalogueTitle = [
    listedCount > 0 ? `V katalógu: ${listedCount.toLocaleString("sk-SK")} fotiek`
      : rule?.explicitMode === "include" ? "Zahrnutý pravidlom"
      : rule?.hasCataloguedDescendant ? "Podpriečinky sú v katalógu" : "",
    rule?.partiallyExcluded ? "Katalogizovaný, niektoré podpriečinky vylúčené" : "",
  ].filter(Boolean).join("\n");
  return <div>
    <div ref={el => { if (el) for (const id of folder.folderIds ?? [folder.id]) rowRefs.current.set(id, el); }} className={`tree-row folder-row ${clipboardPath&&folder.path?.toLowerCase()===clipboardPath.toLowerCase()?"clipboard-dim":""} ${dropTarget?"drop-target":""} ${(folder.folderIds ?? [folder.id]).some(id => activeIds.has(id)) ? "active" : ""}`} style={{ paddingLeft: 8 + depth * 14 }} onDragEnter={spring.onDragEnter} onDragOver={e=>{spring.onDragOver(e);if(!e.dataTransfer.types.includes("application/x-ddreams-asset-ids"))return;e.preventDefault();e.dataTransfer.dropEffect=e.ctrlKey?"copy":"move";setDropTarget(true)}} onDragLeave={e=>{spring.onDragLeave(e);setDropTarget(false)}} onDrop={e=>{spring.onDrop();const raw=e.dataTransfer.getData("application/x-ddreams-asset-ids");if(!raw)return;e.preventDefault();e.stopPropagation();setDropTarget(false);try{const ids=JSON.parse(raw) as number[];if(ids.length&&folder.path)onFileDrop?.(ids,folder.path,e.ctrlKey)}catch{}}} tabIndex={0} onFocus={()=>{if(folder.path)window.dispatchEvent(new CustomEvent("ddreams-tree-focus",{detail:folder.path}))}} onKeyDown={e=>{if(folder.path&&(e.key==="F2"||e.key==="Delete"||(e.ctrlKey&&["x","c","v"].includes(e.key.toLowerCase()))))e.stopPropagation();if(e.key==="F2"&&folder.path){e.preventDefault();onFolderAction?.("rename",folder.path)}else if(e.key==="Delete"&&folder.path){e.preventDefault();onFolderAction?.("delete",folder.path)}else if(e.ctrlKey&&e.key.toLowerCase()==="x"&&folder.path){e.preventDefault();onFolderAction?.("cut",folder.path)}else if(e.ctrlKey&&e.key.toLowerCase()==="c"&&folder.path){e.preventDefault();onFolderAction?.("copy",folder.path)}else if(e.ctrlKey&&e.key.toLowerCase()==="v"&&folder.path){e.preventDefault();onFolderAction?.("paste",folder.path)}}} onContextMenu={e=>{e.preventDefault();if(folder.path)setFolderMenu({x:e.clientX,y:e.clientY})}}>
      <button className="tree-toggle" aria-label={open ? "Zbaliť" : "Rozbaliť"} onClick={() => void onToggle(folder)}>{loading.has(folder.id) ? "·" : open ? "▾" : "▸"}</button>
      <button className="tree-label" onClick={() => onNavigate(folder)} title={folder.path ?? folder.name}><span className={depth===0?"drive-glyph":"folder-glyph"}>{hasDot && <span
        className={`folder-catalogue-dot${directlyCatalogued ? "" : " light"}${rule?.partiallyExcluded ? " partial" : ""}`}
        title={catalogueTitle} aria-label={catalogueTitle} role="img"/>}</span><span className="tree-text">{folder.name || "(bez názvu)"}</span>{rule?.effective === "exclude" && <span className={`folder-rule-mark exclude${rule.explicitMode === "exclude" ? "" : " inherited"}`}
        title={`Vylúčený z katalógu${rule.explicitMode === "exclude" ? "" : ` (podľa ${rule.rulePath})`}`}>⊙</span>}</button>
    </div>
    {folderMenu&&createPortal(<div className="category-context-menu folder-context-menu" style={{left:Math.min(folderMenu.x,window.innerWidth-260),top:Math.min(folderMenu.y,window.innerHeight-350)}} onMouseLeave={()=>setFolderMenu(null)}><button onClick={()=>{setFolderMenu(null);onFolderAction?.("reveal",folder.path!)}}>Otvoriť v Prieskumníkovi</button><button onClick={()=>{setFolderMenu(null);onFolderAction?.("copyPath",folder.path!)}}>Kopírovať cestu</button><hr/><button onClick={()=>{setFolderMenu(null);onFolderAction?.("cut",folder.path!)}}>Vystrihnúť</button><button onClick={()=>{setFolderMenu(null);onFolderAction?.("copy",folder.path!)}}>Kopírovať</button><button disabled={!clipboardAvailable} onClick={()=>{setFolderMenu(null);onFolderAction?.("paste",folder.path!)}}>Vložiť</button><hr/><button onClick={()=>{setFolderMenu(null);onFolderAction?.("rename",folder.path!)}}>Premenovať…</button><button onClick={()=>{setFolderMenu(null);onFolderAction?.("delete",folder.path!)}}>Odstrániť do Koša…</button><hr/><button onClick={()=>{setFolderMenu(null);window.dispatchEvent(new CustomEvent("ddreams-add-shortcut",{detail:folder.path}))}}>Pridať do skratiek</button>{onCreateFolder&&<button onClick={()=>{setFolderMenu(null);if(folder.path)onCreateFolder(folder.path)}}>Nový priečinok…</button>}{onRule&&folder.path&&<>{rule?.explicitMode!=="include"&&<button onClick={()=>{setFolderMenu(null);onRule(folder.path!,"include")}}>Zahrnúť do katalógu</button>}{rule?.explicitMode!=="exclude"&&<button onClick={()=>{setFolderMenu(null);onRule(folder.path!,"exclude")}}>Vylúčiť z katalógu…</button>}{rule?.explicitMode&&<button onClick={()=>{setFolderMenu(null);onRule(folder.path!,null)}}>Zrušiť pravidlo</button>}</>}</div>,document.body)}
    {open && nodes.map(child => <FolderRow key={child.id} folder={child} depth={depth + 1} activeIds={activeIds} expanded={expanded} childrenMap={childrenMap} loading={loading} rowRefs={rowRefs} onToggle={onToggle} onNavigate={onNavigate} onFileDrop={onFileDrop} onCreateFolder={onCreateFolder} onFolderAction={onFolderAction} clipboardPath={clipboardPath} clipboardAvailable={clipboardAvailable} />)}
  </div>;
}

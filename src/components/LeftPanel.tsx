import { useEffect, useRef, useState, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Folder, View } from "../types";
import CalendarPane from "./CalendarPane";

const shortcuts = [
  { name: "Fotky", path: String.raw`F:\My_docs\My Pictures\Fotky` },
  { name: "Panasonic", path: String.raw`F:\My_docs\My Pictures\Fotky\Panasonic` },
];
type Props = { onNavigate: (view: View) => void; activeFolderId: number | null; activeFolderIds?: number[]; activeView?: View; refreshToken?:number };

export default function LeftPanel({ onNavigate, activeFolderId, activeFolderIds = [], activeView, refreshToken=0 }: Props) {
  const [tab, setTab] = useState<"folders" | "calendar">("folders");
  const [roots, setRoots] = useState<Folder[]>([]);
  const [expanded, setExpanded] = useState(new Set<number>());
  const [children, setChildren] = useState(new Map<number, Folder[]>());
  const childrenRef = useRef(children);
  const [loading, setLoading] = useState(new Set<number>());
  const [error, setError] = useState("");
  const rowRefs = useRef(new Map<number, HTMLDivElement>());
  useEffect(() => { childrenRef.current = children; }, [children]);

  useEffect(() => { void invoke<Folder[]>("get_folder_children", { parentId: null, parentIds: null, offset: 0, limit: 500 }).then(setRoots).catch(e => setError(String(e))); }, []);
  useEffect(()=>{if(!refreshToken)return;setChildren(new Map());childrenRef.current=new Map();void invoke<Folder[]>("get_folder_children",{parentId:null,parentIds:null,offset:0,limit:500}).then(setRoots);},[refreshToken]);
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
    if (activeFolderId == null) return;
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
      window.requestAnimationFrame(() => window.requestAnimationFrame(() => rowRefs.current.get(activeFolderId)?.scrollIntoView({ block: "nearest" })));
    })();
    return () => { cancelled = true; };
  }, [activeFolderId, roots, refreshToken]);

  const activeSet = new Set([activeFolderId, ...activeFolderIds].filter((v): v is number => v != null));
  return <aside className="left-pane">
    <section className="shortcuts-pane"><div className="pane-title">SKRATKY</div>{shortcuts.map(s => <button className="shortcut" key={s.path} onClick={() => void clickShortcut(s.path)}><span>▣</span><span><b>{s.name}</b><small>{s.path}</small></span></button>)}</section>
    <div className="left-tabs"><button className={tab === "folders" ? "active" : ""} onClick={() => setTab("folders")}>Priečinky</button><button className={tab === "calendar" ? "active" : ""} onClick={() => setTab("calendar")}>Kalendár</button></div>
    {tab === "calendar" ? <CalendarPane onNavigate={onNavigate} activeView={activeView} refreshToken={refreshToken}/> : <div className="folder-tree">
      {error && <div className="inline-error">{error}<button onClick={() => setError("")}>×</button></div>}
      {roots.map(folder => <FolderRow key={folder.id} folder={folder} depth={0} activeIds={activeSet} expanded={expanded} childrenMap={children} loading={loading.has(folder.id)} rowRefs={rowRefs} onToggle={toggle} onNavigate={navigate} />)}
    </div>}
  </aside>;
}

function FolderRow({ folder, depth, activeIds, expanded, childrenMap, loading, rowRefs, onToggle, onNavigate }: { folder: Folder; depth: number; activeIds: Set<number>; expanded: Set<number>; childrenMap: Map<number, Folder[]>; loading: boolean; rowRefs: MutableRefObject<Map<number, HTMLDivElement>>; onToggle: (folder: Folder) => void; onNavigate: (folder: Folder) => void }) {
  const open = expanded.has(folder.id); const nodes = childrenMap.get(folder.id) ?? [];
  return <div>
    <div ref={el => { if (el) for (const id of folder.folderIds ?? [folder.id]) rowRefs.current.set(id, el); }} className={`tree-row folder-row ${(folder.folderIds ?? [folder.id]).some(id => activeIds.has(id)) ? "active" : ""}`} style={{ paddingLeft: 8 + depth * 14 }}>
      <button className="tree-toggle" aria-label={open ? "Zbaliť" : "Rozbaliť"} onClick={() => void onToggle(folder)}>{loading ? "·" : open ? "▾" : "▸"}</button>
      <button className="tree-label" onClick={() => onNavigate(folder)} title={folder.path ?? folder.name}><span className="folder-icon">▰</span><span className="tree-text">{folder.name || "(bez názvu)"}</span><small>{folder.offline ? "OFFLINE" : depth === 0 ? folder.count || "" : ""}</small></button>
    </div>
    {open && nodes.map(child => <FolderRow key={child.id} folder={child} depth={depth + 1} activeIds={activeIds} expanded={expanded} childrenMap={childrenMap} loading={false} rowRefs={rowRefs} onToggle={onToggle} onNavigate={onNavigate} />)}
  </div>;
}

import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { expandedNodes, folderTreeKey, toggleSetItem, usePersistentSet } from "../uiState";
import type { Folder } from "../types";

type Props={mode:"move"|"copy"|"select"; initialPath:string; onChoose:(path:string)=>void; onClose:()=>void; onCreate?:(parent:string)=>void};
export default function FileOperationDialog({mode,initialPath,onChoose,onClose,onCreate}:Props){
  const [roots,setRoots]=useState<Folder[]>([]);const [children,setChildren]=useState(new Map<number,Folder[]>());const [open,setOpen]=usePersistentSet<string>("ddreams.ui.folderPicker.expanded", "");const [selected,setSelected]=useState<Folder|null>(null);
  const requested = useRef(new Set<number>());
  useEffect(()=>{void invoke<Folder[]>("get_folder_children",{parentId:null,parentIds:null,offset:0,limit:500}).then(setRoots).catch(()=>setRoots([]))},[]);
  useEffect(() => {
    for (const folder of expandedNodes(roots, folder => open.has(folderTreeKey(folder)), folder => children.get(folder.id) ?? [])) {
      if (requested.current.has(folder.id)) continue;
      requested.current.add(folder.id);
      void invoke<Folder[]>("get_folder_children", {
        parentId: folder.virtualNode ? null : folder.id, parentIds: folder.folderIds?.length ? folder.folderIds : null,
        offset: 0, limit: 500,
      }).catch(() => []).then(nodes => setChildren(previous => new Map(previous).set(folder.id, nodes)));
    }
  }, [roots, children, open]);
  const toggle = (folder: Folder) => setOpen(previous => toggleSetItem(previous, folderTreeKey(folder)));
  const row=(f:Folder,depth:number):ReactNode=><div key={`${f.id}:${depth}`}><div className={`folder-picker-row ${selected?.id===f.id?"active":""}`} style={{paddingLeft:depth*16}}><button onClick={()=>void toggle(f)}>{open.has(folderTreeKey(f))?"▾":"▸"}</button><button onClick={()=>setSelected(f)}>{f.name}</button></div>{open.has(folderTreeKey(f))&&(children.get(f.id)??[]).map(c=>row(c,depth+1))}</div>;
  return <div className="modal-backdrop"><section className="file-op-dialog"><header>{mode==="select"?"Priečinok s novými fotkami":mode==="move"?"Presunúť do":"Kopírovať do"}</header><div className="folder-picker">{roots.map(f=>row(f,0))}</div><footer>{onCreate&&<button onClick={()=>{const path=selected?.path??initialPath;if(path)onCreate(path)}}>Nový priečinok…</button>}<span/><button onClick={onClose}>Zrušiť</button><button disabled={!selected?.path} onClick={()=>selected?.path&&onChoose(selected.path)}>Vybrať</button></footer></section></div>;
}

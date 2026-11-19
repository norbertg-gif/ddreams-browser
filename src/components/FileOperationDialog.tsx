import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Folder } from "../types";

type Props={mode:"move"|"copy"|"select"; initialPath:string; onChoose:(path:string)=>void; onClose:()=>void; onCreate?:(parent:string)=>void};
export default function FileOperationDialog({mode,initialPath,onChoose,onClose,onCreate}:Props){
  const [roots,setRoots]=useState<Folder[]>([]);const [children,setChildren]=useState(new Map<number,Folder[]>());const [open,setOpen]=useState(new Set<number>());const [selected,setSelected]=useState<Folder|null>(null);
  useEffect(()=>{void invoke<Folder[]>("get_folder_children",{parentId:null,parentIds:null,offset:0,limit:500}).then(setRoots).catch(()=>setRoots([]))},[]);
  const toggle=async(folder:Folder)=>{if(open.has(folder.id)){setOpen(v=>{const n=new Set(v);n.delete(folder.id);return n});return}setOpen(v=>new Set(v).add(folder.id));if(!children.has(folder.id)){const rows=await invoke<Folder[]>("get_folder_children",{parentId:folder.virtualNode?null:folder.id,parentIds:folder.folderIds?.length?folder.folderIds:null,offset:0,limit:500}).catch(()=>[]);setChildren(v=>new Map(v).set(folder.id,rows))}};
  const row=(f:Folder,depth:number):ReactNode=><div key={`${f.id}:${depth}`}><div className={`folder-picker-row ${selected?.id===f.id?"active":""}`} style={{paddingLeft:depth*16}}><button onClick={()=>void toggle(f)}>{open.has(f.id)?"▾":"▸"}</button><button onClick={()=>setSelected(f)}>{f.name}</button></div>{open.has(f.id)&&(children.get(f.id)??[]).map(c=>row(c,depth+1))}</div>;
  return <div className="modal-backdrop"><section className="file-op-dialog"><header>{mode==="select"?"Priečinok s novými fotkami":mode==="move"?"Presunúť do":"Kopírovať do"}</header><div className="folder-picker">{roots.map(f=>row(f,0))}</div><footer>{onCreate&&<button onClick={()=>{const path=selected?.path??initialPath;if(path)onCreate(path)}}>Nový priečinok…</button>}<span/><button onClick={onClose}>Zrušiť</button><button disabled={!selected?.path} onClick={()=>selected?.path&&onChoose(selected.path)}>Vybrať</button></footer></section></div>;
}

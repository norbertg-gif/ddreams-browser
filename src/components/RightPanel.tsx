import { useEffect, useMemo, useRef, useState, type MouseEvent, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Node as CatalogNode, Metadata, MetadataRow, Root, ValueCount, View } from "../types";
import { labelColor } from "./AssetGrid";

type Props = { primaryId: number | null; selectedIds: number[]; onNavigate: (view: View) => void };
export default function RightPanel({ primaryId, selectedIds, onNavigate }: Props) {
  const [tab, setTab] = useState<"properties" | "catalog">("properties");
  const [metadata, setMetadata] = useState<Metadata | null>(null);
  const [loading, setLoading] = useState(false);
  const [openGroups, setOpenGroups] = useState(new Set(["IPTC", "EXIF", "ACDSee", "Ostatné"]));
  const [selectedRows, setSelectedRows] = useState<string[]>([]);
  const [rowAnchor, setRowAnchor] = useState("");
  const [copyMenu, setCopyMenu] = useState(false);
  const [copyMessage, setCopyMessage] = useState("");
  const metadataRequest = useRef(0);
  const toastTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const requestId = ++metadataRequest.current;
    setMetadata(null); setSelectedRows([]); if (primaryId == null) return;
    const timer = window.setTimeout(() => { setLoading(true); void invoke<Metadata | null>("get_metadata", { assetId: primaryId }).then(value=>{if(metadataRequest.current===requestId)setMetadata(value)}).catch(e=>{if(metadataRequest.current===requestId)setCopyMessage(String(e))}).finally(()=>{if(metadataRequest.current===requestId)setLoading(false)}); }, 260);
    return () => { window.clearTimeout(timer); metadataRequest.current++; };
  }, [primaryId]);
  const groups = useMemo(() => ["IPTC", "EXIF", "ACDSee", "Ostatné"].map(name => ({ name, rows: metadata?.rows.filter(row => row.group === name) ?? [] })), [metadata]);
  const rowKey = (row: MetadataRow) => `${row.group}\u0000${row.name}`;
  const rowList = groups.flatMap(g => g.rows);
  const selectRow = (row: MetadataRow, event: MouseEvent) => {
    const key = rowKey(row); setRowAnchor(key);
    if (event.shiftKey && rowAnchor) {
      const a = rowList.findIndex(r => rowKey(r) === rowAnchor), b = rowList.findIndex(r => rowKey(r) === key);
      setSelectedRows(rowList.slice(Math.min(a,b),Math.max(a,b)+1).map(rowKey));
    } else if (event.ctrlKey || event.metaKey) setSelectedRows(prev => prev.includes(key) ? prev.filter(v => v !== key) : [...prev,key]);
    else setSelectedRows([key]);
  };
  const copy = async (mode: "selected" | "values" | "section", group?: string) => {
    const ids = selectedIds.length > 1 ? selectedIds : primaryId == null ? [] : [primaryId];
    if (!ids.length) return;
    const keys = mode === "section" ? null : selectedRows;
    if (keys && !keys.length) return;
    const payloads = await Promise.all(ids.map(assetId => invoke<Metadata | null>("get_metadata", { assetId })));
    let rowCount = 0;
    const blocks = payloads.filter((m): m is Metadata => !!m).map(m => {
      const rows = m.rows.filter(r => (!group || r.group === group) && (keys == null || keys.includes(rowKey(r))));
      rowCount += rows.length;
      if (!rows.length) return "";
      const lines = rows.map(r => mode === "values" ? r.value : `${r.name}: ${r.value}`);
      return `${m.asset.name}\n${lines.join("\n")}`;
    }).filter(Boolean);
    if (!blocks.length) return;
    const plural=(n:number,one:string,few:string,many:string)=>n===1?one:n>=2&&n<=4?few:many;
    const rowsText=`${rowCount} ${plural(rowCount,"riadok","riadky","riadkov")}`;
    const message=ids.length===1?`Skop?rovan?: ${rowsText}`:`Skop?rovan?: ${rowsText} ? ${blocks.length} ${plural(blocks.length,"s?bor","s?bory","s?borov")}`;
    try { await navigator.clipboard.writeText(blocks.join("\n\n")); setCopyMessage(message); }
    catch { setCopyMessage("Kop?rovanie do schr?nky sa nepodarilo"); }
    setCopyMenu(false);
    if(toastTimer.current!==undefined)window.clearTimeout(toastTimer.current);
    toastTimer.current=window.setTimeout(() => setCopyMessage(""), 1500);
  };
  const handlePropertiesKeyDown=(event:ReactKeyboardEvent<HTMLDivElement>)=>{
    if(!event.ctrlKey&&!event.metaKey)return;
    if(!event.currentTarget.contains(document.activeElement))return;
    if(event.key.toLowerCase()==="a"){event.preventDefault();setSelectedRows(rowList.map(rowKey));}
    else if(event.key.toLowerCase()==="c"&&selectedRows.length){event.preventDefault();void copy("selected");}
  };
  const toggleGroup = (name: string) => setOpenGroups(prev => { const next = new Set(prev); if (next.has(name)) next.delete(name); else next.add(name); return next; });
  return <aside className="right-pane">
    <div className="right-tabs"><button className={tab === "properties" ? "active" : ""} onClick={() => setTab("properties")}>Vlastnosti</button><button className={tab === "catalog" ? "active" : ""} onClick={() => setTab("catalog")}>Katalóg</button></div>
    {tab === "properties" ? <div className="properties-content" tabIndex={0} onClick={event=>event.currentTarget.focus()} onKeyDown={handlePropertiesKeyDown}>
      <div className="selected-summary">{metadata?.asset.name ?? (primaryId == null ? "Nie je vybraný súbor" : "Načítavam vlastnosti…")}{selectedIds.length > 1 && <small>{selectedIds.length} vybraných súborov</small>}</div>
      {metadata?.offline && <div className="offline-note">Súbor je offline. Zobrazené sú údaje z katalógu.</div>}
      {loading && <div className="subtle">Načítavam metadáta…</div>}
      {groups.map(group => <section className="property-group" key={group.name}>
        <button className="accordion-head" onClick={() => toggleGroup(group.name)}><span>{openGroups.has(group.name) ? "▾" : "▸"} {group.name}</span><span>{group.rows.length}</span></button>
        {openGroups.has(group.name) && <div className="metadata-rows">
          {group.rows.map(row => <div key={rowKey(row)} className={`meta-row ${selectedRows.includes(rowKey(row)) ? "selected" : ""}`} onClick={e => selectRow(row,e)} onContextMenu={e => { e.preventDefault(); setSelectedRows([rowKey(row)]); setRowAnchor(rowKey(row)); setCopyMenu(true); }}>
            <span title={row.name}>{row.name}</span><b title={row.value}>{row.value}</b>
          </div>)}
          {!group.rows.length && <div className="tree-empty">Žiadne údaje</div>}
        </div>}
      </section>)}
      <div className="copy-controls"><button disabled={!metadata || selectedRows.length === 0} onClick={() => void copy("selected")}>Kopírovať výber</button><button disabled={!metadata} onClick={() => setCopyMenu(v => !v)}>▾</button>
        {copyMenu && <div className="copy-menu"><button onClick={() => void copy("values")}>Kopírovať len hodnoty</button><button onClick={() => void copy("section", "IPTC")}>Kopírovať celú sekciu IPTC</button><button onClick={() => void copy("section", "EXIF")}>Kopírovať celú sekciu EXIF</button><button onClick={() => void copy("section", "ACDSee")}>Kopírovať celú sekciu ACDSee</button><button onClick={() => void copy("section", "Ostatné")}>Kopírovať celú sekciu Ostatné</button></div>}
      </div>{copyMessage && <div className="copy-message">{copyMessage}</div>}
    </div> : <CatalogPanel onNavigate={onNavigate} />}
  </aside>;
}

function CatalogPanel({ onNavigate }: { onNavigate: (view: View) => void }) {
  const [roots,setRoots]=useState<Root[]>([]);const [openRoots,setOpenRoots]=useState(new Set<number>());const [cats,setCats]=useState(new Map<number,CatalogNode[]>());const [openCats,setOpenCats]=useState(new Set<number>());const [catChildren,setCatChildren]=useState(new Map<number,CatalogNode[]>());
  const [keywords,setKeywords]=useState<CatalogNode[]>([]);const [keywordResults,setKeywordResults]=useState<CatalogNode[]>([]);const [openKeywords,setOpenKeywords]=useState(new Set<number>());const [keywordChildren,setKeywordChildren]=useState(new Map<number,CatalogNode[]>());const [keywordQuery,setKeywordQuery]=useState("");
  const [ratings,setRatings]=useState<ValueCount[]>([]);const [labels,setLabels]=useState<ValueCount[]>([]);
  useEffect(()=>{void invoke<Root[]>("get_category_roots",{offset:0,limit:500}).then(setRoots);void invoke<ValueCount[]>("get_catalog_values",{kind:"rating",offset:0,limit:20}).then(values=>{const counts=new Map(values.map(v=>[v.value,v.count]));setRatings(["unrated","1","2","3","4","5"].map(value=>({value,count:counts.get(value)??0}))) });void invoke<ValueCount[]>("get_catalog_values",{kind:"label",offset:0,limit:100}).then(setLabels);void invoke<CatalogNode[]>("get_catalog_nodes",{kind:"keyword",parentId:null,rootId:null,offset:0,limit:500}).then(setKeywords);},[]);
  useEffect(()=>{if(!keywordQuery.trim()){setKeywordResults([]);return}const timer=window.setTimeout(()=>void invoke<CatalogNode[]>("search_keywords",{query:keywordQuery,offset:0,limit:500}).then(setKeywordResults),160);return()=>window.clearTimeout(timer)},[keywordQuery]);
  const toggleRoot=async(root:Root)=>{if(openRoots.has(root.id)){setOpenRoots(p=>{const n=new Set(p);n.delete(root.id);return n});return}setOpenRoots(p=>new Set(p).add(root.id));if(!cats.has(root.id)){const nodes=await invoke<CatalogNode[]>("get_catalog_nodes",{kind:"category",parentId:null,rootId:root.id,offset:0,limit:500});setCats(p=>new Map(p).set(root.id,nodes));}};
  const toggleCat=async(node:CatalogNode)=>{if(openCats.has(node.id)){setOpenCats(p=>{const n=new Set(p);n.delete(node.id);return n});return}setOpenCats(p=>new Set(p).add(node.id));if(!catChildren.has(node.id)){const nodes=await invoke<CatalogNode[]>("get_catalog_nodes",{kind:"category",parentId:node.id,rootId:node.rootId,offset:0,limit:500});setCatChildren(p=>new Map(p).set(node.id,nodes));}};
  const toggleKeyword=async(node:CatalogNode)=>{if(openKeywords.has(node.id)){setOpenKeywords(p=>{const n=new Set(p);n.delete(node.id);return n});return}setOpenKeywords(p=>new Set(p).add(node.id));if(!keywordChildren.has(node.id)){const nodes=await invoke<CatalogNode[]>("get_catalog_nodes",{kind:"keyword",parentId:node.id,rootId:null,offset:0,limit:500});setKeywordChildren(p=>new Map(p).set(node.id,nodes));}};
  const navigateCategory=(node:CatalogNode)=>onNavigate({kind:"category",id:node.id,title:`Kategória: ${node.name}`});
  const navigateKeyword=(node:CatalogNode)=>onNavigate({kind:"keyword",id:node.id,title:`Kľúčové slovo: ${node.name}`});
  const filteredKeywords=keywordQuery.trim()?keywordResults:keywords;
  return <div className="catalog-content">
    <section className="catalog-section"><div className="section-heading">Kategórie</div>{roots.map(root=><div key={root.id}><div className="tree-row"><button className="tree-toggle" onClick={()=>void toggleRoot(root)}>{openRoots.has(root.id)?"▾":"▸"}</button><button className="tree-label" onClick={()=>void toggleRoot(root)}><span className="category-icon">▤</span>{root.name}</button></div>{openRoots.has(root.id)&&(cats.get(root.id)??[]).map(cat=><CategoryNode key={cat.id} node={cat} depth={1} open={openCats} childrenMap={catChildren} onToggle={toggleCat} onNavigate={navigateCategory}/>)}</div>)}</section>
    <section className="catalog-section"><div className="section-heading">Hodnotenie</div>{ratings.map(r=><button className="value-row" key={r.value} onClick={()=>onNavigate({kind:"rating",value:r.value,title:r.value==="unrated"?"Bez hodnotenia":`Hodnotenie ${r.value} ★`})}><span>{r.value==="unrated"?"Bez hodnotenia":"★".repeat(Number(r.value))}</span><small>{r.count}</small></button>)}</section>
    <section className="catalog-section"><div className="section-heading">Farebné označenia</div>{labels.filter(l=>l.value).map(l=><button className="value-row" key={l.value} onClick={()=>onNavigate({kind:"label",value:l.value,title:`Označenie: ${l.value}`})}><span><i className="label-dot" style={{background:labelColor(l.value)}}/>{l.value}</span><small>{l.count}</small></button>)}</section>
    <section className="catalog-section keyword-section"><div className="section-heading">Kľúčové slová</div><input className="keyword-filter" value={keywordQuery} onChange={e=>setKeywordQuery(e.target.value)} placeholder="Filtrovať…" />{filteredKeywords.map(node=>keywordQuery?<button className="tree-row tree-label keyword-result" key={node.id} onClick={()=>navigateKeyword(node)}>{node.name}<small>{node.count}</small></button>:<KeywordNode key={node.id} node={node} depth={0} open={openKeywords} childrenMap={keywordChildren} onToggle={toggleKeyword} onNavigate={navigateKeyword}/>)}</section>
  </div>;
}
function CategoryNode({node,depth,open,childrenMap,onToggle,onNavigate}:{node:CatalogNode;depth:number;open:Set<number>;childrenMap:Map<number,CatalogNode[]>;onToggle:(node:CatalogNode)=>void;onNavigate:(node:CatalogNode)=>void}){return <div><div className="tree-row" style={{paddingLeft:8+depth*14}}><button className="tree-toggle" onClick={()=>onToggle(node)}>{open.has(node.id)?"▾":"▸"}</button><button className="tree-label" onClick={()=>onNavigate(node)}><span className="category-icon">▤</span><span className="tree-text">{node.name}</span><small>{node.count||""}</small></button></div>{open.has(node.id)&&(childrenMap.get(node.id)??[]).map(child=><CategoryNode key={child.id} node={child} depth={depth+1} open={open} childrenMap={childrenMap} onToggle={onToggle} onNavigate={onNavigate}/>)}</div>}
function KeywordNode({node,depth,open,childrenMap,onToggle,onNavigate}:{node:CatalogNode;depth:number;open:Set<number>;childrenMap:Map<number,CatalogNode[]>;onToggle:(node:CatalogNode)=>void;onNavigate:(node:CatalogNode)=>void}){return <div><div className="tree-row" style={{paddingLeft:8+depth*14}}><button className="tree-toggle" onClick={()=>onToggle(node)}>{open.has(node.id)?"▾":"▸"}</button><button className="tree-label" onClick={()=>onNavigate(node)}><span className="keyword-icon">⌑</span><span className="tree-text">{node.name}</span><small>{node.count||""}</small></button></div>{open.has(node.id)&&(childrenMap.get(node.id)??[]).map(child=><KeywordNode key={child.id} node={child} depth={depth+1} open={open} childrenMap={childrenMap} onToggle={onToggle} onNavigate={onNavigate}/>)}</div>}

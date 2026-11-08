import { useEffect, useMemo, useRef, useState, type MouseEvent, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { CatalogAssetLinks, Node as CatalogNode, Metadata, MetadataRow, Root, ValueCount, View } from "../types";
import { labelColor, labelText } from "./AssetGrid";
import { readStored, writeStored } from "../storage";
import "./CategoryManagement.css";

type CategoryMutation = { changeId:number|null; summary:string; categoryId:number|null; parentId:number|null; rootId:number|null; ancestorIds:number[]; deletedCategoryIds:number[]; deletedRootIds:number[] };
type KeywordMutation = { changeId:number|null;summary:string;keywordId:number|null;parentId:number|null;ancestorIds:number[];deletedKeywordIds:number[] };
type ColorItem={value:string;name:string;hex:string;auto:boolean;effective:boolean;manualState:boolean|null};type AssetColorInfo={assetId:number;computed:boolean;colors:ColorItem[]};
type AiSuggestion={labelId:number;path:string;score:number;count?:number;assetIds?:number[]};
type Props = { primaryId: number | null; selectedIds: number[]; includeChildren: boolean; onIncludeChildrenChange: (value: boolean) => void; onNavigate: (view: View) => void; onSetValues:(ids:number[],changes:Record<string,unknown>)=>void; onSetColors:(ids:number[],color:string,state:boolean|null)=>void; onSetCategory:(ids:number[],categoryId:number,assign:boolean)=>void; onCategoryChanged:(result:CategoryMutation,deleted:boolean)=>void; onSetKeyword:(ids:number[],keywordId:number,assign:boolean)=>void; onCreateAssignKeyword:(ids:number[],parentId:number|null,name:string)=>void; onKeywordChanged:(result:KeywordMutation,deleted:boolean)=>void; onCategoryError:(error:unknown)=>void; readOnly:boolean; refreshToken: number; catalogRefreshToken?: number;aiAvailable:boolean;aiReady:boolean;aiReason:string };
export default function RightPanel({ primaryId, selectedIds, includeChildren, onIncludeChildrenChange, onNavigate, onSetValues, onSetColors, onSetCategory, onCategoryChanged, onSetKeyword, onCreateAssignKeyword, onKeywordChanged, onCategoryError, readOnly, refreshToken, catalogRefreshToken = 0,aiAvailable,aiReady,aiReason }: Props) {
  const [tab, setTab] = useState<"properties" | "catalog">("properties");
  const [metadata, setMetadata] = useState<Metadata | null>(null);
  const [assetLinks,setAssetLinks]=useState<CatalogAssetLinks|null>(null);
  const [loading, setLoading] = useState(false);
  const [openGroups, setOpenGroups] = useState<Set<string>>(() => new Set(readStored<string[]>("ddreams.metadataOpen", ["IPTC", "EXIF", "Katalóg"]).map(name => name === "ACDSee" ? "Katalóg" : name)));
  const [selectedRows, setSelectedRows] = useState<string[]>([]);
  const [rowAnchor, setRowAnchor] = useState("");
  const [copyMenu, setCopyMenu] = useState(false);
  const [copyMessage, setCopyMessage] = useState("");
  const [assetColors,setAssetColors]=useState<AssetColorInfo[]>([]);
  const [aiSuggestions,setAiSuggestions]=useState<{category:AiSuggestion[];keyword:AiSuggestion[]}>({category:[],keyword:[]});
  useEffect(()=>{if(!aiReady){setAiSuggestions({category:[],keyword:[]});return;}const ids=selectedIds.length?selectedIds:primaryId==null?[]:[primaryId];if(!ids.length){setAiSuggestions({category:[],keyword:[]});return;}let active=true;const timer=window.setTimeout(()=>{void Promise.all([invoke<AiSuggestion[]>("ai_suggest",{assetIds:ids,kind:"category"}),invoke<AiSuggestion[]>("ai_suggest",{assetIds:ids,kind:"keyword"})]).then(([category,keyword])=>{if(active)setAiSuggestions({category,keyword})}).catch(()=>{if(active)setAiSuggestions({category:[],keyword:[]})})},300);return()=>{active=false;window.clearTimeout(timer)}},[primaryId,selectedIds.join(","),aiReady,refreshToken]);
  const colorIds=selectedIds.length?selectedIds:primaryId==null?[]:[primaryId];
  const metadataRequest = useRef(0);
  const toastTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const requestId = ++metadataRequest.current;
    setMetadata(null); setSelectedRows([]); if (primaryId == null) return;
    const timer = window.setTimeout(() => { setLoading(true); void invoke<Metadata | null>("get_metadata", { assetId: primaryId }).then(value=>{if(metadataRequest.current===requestId)setMetadata(value)}).catch(e=>{if(metadataRequest.current===requestId)setCopyMessage(String(e))}).finally(()=>{if(metadataRequest.current===requestId)setLoading(false)}); }, 260);
    return () => { window.clearTimeout(timer); metadataRequest.current++; };
  }, [primaryId, refreshToken]);
  useEffect(()=>{let active=true;setAssetColors([]);if(!colorIds.length)return;void invoke<AssetColorInfo[]>("get_asset_colors",{assetIds:colorIds}).then(v=>{if(active)setAssetColors(v)}).catch(()=>{if(active)setAssetColors([])});return()=>{active=false}},[colorIds.join(","),refreshToken]);
  useEffect(()=>{let active=true;setAssetLinks(null);if(primaryId==null)return;void invoke<CatalogAssetLinks|null>("get_catalog_asset_links",{assetId:primaryId}).then(v=>{if(active)setAssetLinks(v)}).catch(()=>{if(active)setAssetLinks(null)});return()=>{active=false}},[primaryId,refreshToken]);
  useEffect(() => writeStored("ddreams.metadataOpen", [...openGroups]), [openGroups]);
  const groups = useMemo(() => {
    const rows = metadata?.rows ?? [];
    const fixed = ["IPTC", "EXIF", "Katalóg"];
    const extra = [...new Set(rows.map(row => row.group).filter(name => !fixed.includes(name)))].sort((a,b) => {
      if (a === "Všetky metadáta") return 1; if (b === "Všetky metadáta") return -1; return a.localeCompare(b);
    });
    return [...fixed, ...extra].map(name => ({name, rows: rows.filter(row => row.group === name)})).filter(group => group.rows.length);
  }, [metadata]);
  const rowKey = (row: MetadataRow) => `${row.group}\u0000${row.subgroup}\u0000${row.name}`;
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
    const message=ids.length===1?`Skopírované: ${rowsText}`:`Skopírované: ${rowsText} · ${blocks.length} ${plural(blocks.length,"súbor","súbory","súborov")}`;
    try { await navigator.clipboard.writeText(blocks.join("\n\n")); setCopyMessage(message); }
    catch { setCopyMessage("Kopírovanie do schránky sa nepodarilo"); }
    setCopyMenu(false);
    if(toastTimer.current!==undefined)window.clearTimeout(toastTimer.current);
    toastTimer.current=window.setTimeout(() => setCopyMessage(""), 1500);
  };
  const handlePropertiesKeyDown=(event:ReactKeyboardEvent<HTMLDivElement>)=>{
    if(!event.ctrlKey&&!event.metaKey)return;
    if(event.target instanceof HTMLElement&&event.target.closest("input,textarea,select,[contenteditable='true']"))return;
    if(!event.currentTarget.contains(document.activeElement))return;
    if(event.key.toLowerCase()==="a"){event.preventDefault();setSelectedRows(rowList.map(rowKey));}
    else if(event.key.toLowerCase()==="c"&&selectedRows.length){event.preventDefault();void copy("selected");}
  };
  const toggleGroup = (name: string) => setOpenGroups(prev => { const next = new Set(prev); if (next.has(name)) next.delete(name); else next.add(name); return next; });
  return <aside className="right-pane">
    <div className="right-tabs"><button className={tab === "properties" ? "active" : ""} onClick={() => setTab("properties")}>Vlastnosti</button><button className={tab === "catalog" ? "active" : ""} onClick={() => setTab("catalog")}>Katalóg</button></div>
    {tab === "properties" ? <div className="properties-content" tabIndex={0} onClick={event=>{if(!(event.target instanceof HTMLElement)||!event.target.closest("input,textarea,select,button"))event.currentTarget.focus()}} onKeyDown={handlePropertiesKeyDown}>
      <div className="selected-summary">{metadata?.asset.name ?? (primaryId == null ? "Nie je vybraný súbor" : "Načítavam vlastnosti…")}{selectedIds.length > 1 && <small>{selectedIds.length} vybraných súborov</small>}</div>
      {!aiReady&&aiAvailable&&<div className="subtle" title={aiReason}>AI sa načítava…</div>}
      {aiReady&&(["category","keyword"] as const).map(kind=>{const rows=aiSuggestions[kind];return rows.length?<section className="property-group" key={`ai-${kind}`}><div className="accordion-head"><span>Návrhy AI · {kind==="category"?"Kategórie":"Kľúčové slová"}</span></div><div className="ai-suggestion-chips">{rows.map(item=><span className="ai-suggestion-chip" key={item.labelId} title={`${item.path}\n${selectedIds.length>1?`${item.count??0} z ${selectedIds.length} vybraných`:`podobných fotiek: ${item.count??0}`}`}><button disabled={readOnly} onClick={()=>{const ids=item.assetIds??(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId]);if(kind==="category")onSetCategory(ids,item.labelId,true);else onSetKeyword(ids,item.labelId,true);setAiSuggestions(current=>({...current,[kind]:current[kind].filter(row=>row.labelId!==item.labelId)}))}}>{item.path.split("/").slice(-2).join(" / ")} {Math.round(item.score*100)}%</button><button aria-label="Odmietnuť návrh" onClick={()=>{const ids=selectedIds.length?selectedIds:primaryId==null?[]:[primaryId];void invoke("ai_reject_suggestion",{assetIds:ids,kind,labelId:item.labelId}).then(()=>setAiSuggestions(current=>({...current,[kind]:current[kind].filter(row=>row.labelId!==item.labelId)})))}}>×</button></span>)}</div></section>:null})}
      {metadata?.offline && <div className="offline-note">Súbor je offline. Zobrazené sú údaje z katalógu.</div>}
      {loading && <div className="subtle">Načítavam metadáta…</div>}
      {groups.map(group => <section className="property-group" key={group.name}>
        <button className="accordion-head" onClick={() => toggleGroup(group.name)}><span>{openGroups.has(group.name) ? "▾" : "▸"} {group.name}</span><span>{group.rows.length}</span></button>
        {openGroups.has(group.name) && <div className="metadata-rows">
          {[...new Set(group.rows.map(row=>row.subgroup))].map(subgroup=><div className="metadata-subgroup" key={subgroup}>
            {subgroup && subgroup !== group.name && <div className="metadata-subgroup-title">{subgroup}</div>}
            {group.rows.filter(row=>row.subgroup===subgroup).map(row => <div key={rowKey(row)} className={`meta-row ${selectedRows.includes(rowKey(row)) ? "selected" : ""}`} onClick={e => selectRow(row,e)} onContextMenu={e => { e.preventDefault(); setSelectedRows([rowKey(row)]); setRowAnchor(rowKey(row)); setCopyMenu(true); }}>
              <span title={row.name}>{row.name}</span><b title={row.value}>{group.name === "Katalóg" && row.name === "Hodnotenie" ? <span className="property-rating">{[1,2,3,4,5].map(value=><button key={value} disabled={readOnly} className={Number(row.value)>=value?"active":""} title={`${value} / 5`} onClick={e=>{e.stopPropagation();onSetValues(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId],{rating:value})}}>★</button>)}<button disabled={readOnly} title="Bez hodnotenia" onClick={e=>{e.stopPropagation();onSetValues(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId],{rating:null})}}>×</button></span> : group.name === "Katalóg" && row.name === "Farebný štítok" ? <span className="property-labels">{[["Red","Červená"],["Yellow","Žltá"],["Green","Zelená"],["Blue","Modrá"],["Purple","Fialová"]].map(([value,label])=><button key={value} disabled={readOnly} title={label} className={row.value.toLowerCase()===value.toLowerCase()?"active":""} onClick={e=>{e.stopPropagation();onSetValues(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId],{label:value})}}><i style={{background:labelColor(value)}}/></button>)}<button disabled={readOnly} title="Bez štítku" onClick={e=>{e.stopPropagation();onSetValues(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId],{label:null})}}>×</button></span> : group.name === "Katalóg" && row.name === "Označené" ? <input type="checkbox" disabled={readOnly} checked={row.value==="Áno"} onClick={e=>e.stopPropagation()} onChange={e=>onSetValues(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId],{tagged:e.target.checked})}/> : group.name === "Katalóg" && row.name === "Kategórie" ? <span className="property-categories">{(assetLinks?.categoryPaths??[]).map(category=><span key={category.id} title={category.path}>{category.path}<button aria-label={`Odobrať z kategórie ${category.path}`} onClick={e=>{e.stopPropagation();onSetCategory(selectedIds.length?selectedIds:primaryId==null?[]:[primaryId],category.id,false)}}>×</button></span>)}</span> : group.name === "Katalóg" && row.name === "Kľúčové slová" ? <KeywordProperties links={assetLinks} primaryId={primaryId} selectedIds={selectedIds} onSetKeyword={onSetKeyword} onCreateAssignKeyword={onCreateAssignKeyword} readOnly={readOnly} onError={onCategoryError}/> : row.value}</b>
            </div>)}
          </div>)}
        </div>}
        {group.name==="Katalóg"&&<div className="meta-row color-properties"><span>Farby</span><b><span className="property-color-chips">{assetColors[0]?.colors.map(color=>{const count=assetColors.filter(asset=>asset.colors.find(c=>c.value===color.value)?.effective).length;const manual=assetColors.some(asset=>asset.colors.find(c=>c.value===color.value)?.manualState!==null);const cls=count===0?"empty":count===assetColors.length?"full":"partial";return <button key={color.value} disabled={readOnly||!colorIds.length} className={`property-color-chip ${cls} ${manual?"manual":"auto"}`} title={`${color.name}${count?` · ${count}/${assetColors.length}`:""}`} onClick={e=>{e.stopPropagation();onSetColors(colorIds,color.value,e.altKey?null:count===assetColors.length?false:true)}}><i style={{background:color.hex}}/></button>})}</span></b>{assetColors.length>0&&assetColors.some(asset=>!asset.computed)&&<small className="color-not-computed">Farby ešte nie sú vypočítané</small>}{!aiAvailable&&<small className="color-not-computed">AI modul nie je nainštalovaný</small>}</div>}
      </section>)}
      <div className="copy-controls"><button disabled={!metadata || selectedRows.length === 0} onClick={() => void copy("selected")}>Kopírovať výber</button><button disabled={!metadata} onClick={() => setCopyMenu(v => !v)}>▾</button>
        {copyMenu && <div className="copy-menu"><button onClick={() => void copy("values")}>Kopírovať len hodnoty</button>{groups.map(group=><button key={group.name} onClick={() => void copy("section", group.name)}>Kopírovať celú sekciu {group.name}</button>)}</div>}
      </div>{copyMessage && <div className="copy-message">{copyMessage}</div>}
    </div> : <CatalogPanel onNavigate={onNavigate} includeChildren={includeChildren} onIncludeChildrenChange={onIncludeChildrenChange} primaryId={primaryId} selectedIds={selectedIds} onSetCategory={onSetCategory} onSetKeyword={onSetKeyword} onCategoryChanged={onCategoryChanged} onKeywordChanged={onKeywordChanged} onCategoryError={onCategoryError} refreshToken={refreshToken} catalogRefreshToken={catalogRefreshToken} aiAvailable={aiAvailable} aiReady={aiReady} aiReason={aiReason} />}
  </aside>;
}

function CatalogPanel({ onNavigate, includeChildren, onIncludeChildrenChange, primaryId, selectedIds, onSetCategory, onSetKeyword, onCategoryChanged, onKeywordChanged, onCategoryError, refreshToken, catalogRefreshToken = 0,aiAvailable,aiReady,aiReason }: { onNavigate: (view: View) => void; includeChildren: boolean; onIncludeChildrenChange: (value: boolean) => void; primaryId: number | null; selectedIds:number[]; onSetCategory:(ids:number[],categoryId:number,assign:boolean)=>void; onSetKeyword:(ids:number[],keywordId:number,assign:boolean)=>void; onCategoryChanged:(result:CategoryMutation,deleted:boolean)=>void; onKeywordChanged:(result:KeywordMutation,deleted:boolean)=>void; onCategoryError:(error:unknown)=>void;refreshToken: number; catalogRefreshToken?: number;aiAvailable:boolean;aiReady:boolean;aiReason:string }) {
  const [roots,setRoots]=useState<Root[]>([]);
  const [openRoots,setOpenRoots]=useState(new Set<number>());
  const [cats,setCats]=useState(new Map<number,CatalogNode[]>());
  const [openCats,setOpenCats]=useState(new Set<number>());
  const [catChildren,setCatChildren]=useState(new Map<number,CatalogNode[]>());
  const [keywords,setKeywords]=useState<CatalogNode[]>([]);
  const [keywordResults,setKeywordResults]=useState<CatalogNode[]>([]);
  const [categoryResults,setCategoryResults]=useState<CatalogNode[]>([]);
  const [openKeywords,setOpenKeywords]=useState(new Set<number>());
  const [keywordChildren,setKeywordChildren]=useState(new Map<number,CatalogNode[]>());
  const [keywordMenu,setKeywordMenu]=useState<{x:number;y:number;id:number;name:string;parentId:number|null}|null>(null);
  const [activeKeyword,setActiveKeyword]=useState<{id:number;name:string;parentId:number|null}|null>(null);
  const [keywordDialog,setKeywordDialog]=useState<{kind:"create";parentId:number|null;name:string}|{kind:"rename";id:number;name:string}|null>(null);
  const [keywordDeleteDialog,setKeywordDeleteDialog]=useState<{info:{id:number;name:string;assignedFiles:number;subkeywords:number};blocked:boolean}|null>(null);
  const [highlightKeyword,setHighlightKeyword]=useState<number|null>(null);
  const [keywordDropTarget,setKeywordDropTarget]=useState<number|null>(null);
  const [keywordTopDrop,setKeywordTopDrop]=useState(false);
  const [keywordQuery,setKeywordQuery]=useState("");
  const [ratings,setRatings]=useState<ValueCount[]>([]);
  const [labels,setLabels]=useState<ValueCount[]>([]);
  const [colorCounts,setColorCounts]=useState<{value:string;name:string;hex:string;count:number}[]>([]);
  const [special,setSpecial]=useState<ValueCount[]>([]);
  const [suggestionKind,setSuggestionKind]=useState<"category"|"keyword">("category");const [suggestionGroups,setSuggestionGroups]=useState<{labelId:number;path:string;count:number;assetIds:number[]}[]>([]);const [suggestionLoading,setSuggestionLoading]=useState(false);
  const suggestionsLoaded=useRef(false);
  useEffect(()=>{if(!suggestionsLoaded.current)return;let active=true;void invoke<typeof suggestionGroups>("ai_suggestion_groups",{kind:suggestionKind}).then(g=>{if(active)setSuggestionGroups(g)}).catch(()=>{});return()=>{active=false}},[catalogRefreshToken]);
  const [links,setLinks]=useState<CatalogAssetLinks|null>(null);
  const [collapsed,setCollapsed]=useState<Record<string,boolean>>(()=>readStored("ddreams.catalogCollapsed",{}));
  const [showAssignSquares,setShowAssignSquares]=useState(()=>readStored("ddreams.showAssignSquares",true));
  const [selectionCounts,setSelectionCounts]=useState<{totalFiles:number;assigned:Record<number,number>}>({totalFiles:0,assigned:{}});
  const [keywordSelectionCounts,setKeywordSelectionCounts]=useState<{totalFiles:number;assigned:Record<number,number>}>({totalFiles:0,assigned:{}});
  const [categoryMenu,setCategoryMenu]=useState<{x:number;y:number;id:number;name:string;parentId:number|null;rootId:number|null}|null>(null);
  const [activeCategory,setActiveCategory]=useState<{id:number;name:string;parentId:number|null;rootId:number|null}|null>(null);
  const [categoryDialog,setCategoryDialog]=useState<{kind:"create";parentId:number|null;name:string}|{kind:"rename";id:number;name:string}|null>(null);
  const [deleteDialog,setDeleteDialog]=useState<{info:{id:number;name:string;assignedFiles:number;subcategories:number};blocked:boolean}|null>(null);
  const [highlightCategory,setHighlightCategory]=useState<number|null>(null);
  const [dropTarget,setDropTarget]=useState<number|null>(null);
  const [topDropActive,setTopDropActive]=useState(false);
  const linksRequest=useRef(0);
  const labelTimer=useRef<number|undefined>(undefined);

  useEffect(()=>writeStored("ddreams.catalogCollapsed",collapsed),[collapsed]);
  useEffect(()=>writeStored("ddreams.showAssignSquares",showAssignSquares),[showAssignSquares]);
  useEffect(()=>{if(highlightCategory==null)return;const timer=window.setTimeout(()=>setHighlightCategory(null),2400);return()=>window.clearTimeout(timer)},[highlightCategory]);
  useEffect(()=>{if(highlightKeyword==null)return;const timer=window.setTimeout(()=>setHighlightKeyword(null),2400);return()=>window.clearTimeout(timer)},[highlightKeyword]);
  useEffect(()=>{const handle=(event:globalThis.KeyboardEvent)=>{if(event.key!=="F2"||!activeCategory)return;const target=event.target instanceof HTMLElement?event.target:null;if(target?.closest("input,textarea,select,[contenteditable='true']"))return;event.preventDefault();setCategoryDialog({kind:"rename",id:activeCategory.id,name:activeCategory.name});};window.addEventListener("keydown",handle);return()=>window.removeEventListener("keydown",handle)},[activeCategory]);
  useEffect(()=>{const handle=(event:globalThis.KeyboardEvent)=>{if(event.key!=="F2"||!activeKeyword)return;const target=event.target instanceof HTMLElement?event.target:null;if(target?.closest("input,textarea,select,[contenteditable='true']"))return;event.preventDefault();setKeywordDialog({kind:"rename",id:activeKeyword.id,name:activeKeyword.name});};window.addEventListener("keydown",handle);return()=>window.removeEventListener("keydown",handle)},[activeKeyword]);
  useEffect(()=>{let active=true;if(!selectedIds.length){setSelectionCounts({totalFiles:0,assigned:{}});return;}void invoke<{totalFiles:number;assigned:Record<number,number>}>("get_category_selection_counts",{assetIds:selectedIds}).then(value=>{if(active)setSelectionCounts(value)}).catch(()=>{if(active)setSelectionCounts({totalFiles:0,assigned:{}})});return()=>{active=false}},[selectedIds,refreshToken]);
  useEffect(()=>{let active=true;if(!selectedIds.length){setKeywordSelectionCounts({totalFiles:0,assigned:{}});return;}void invoke<{totalFiles:number;assigned:Record<number,number>}>('get_keyword_selection_counts',{assetIds:selectedIds}).then(value=>{if(active)setKeywordSelectionCounts(value)}).catch(()=>{if(active)setKeywordSelectionCounts({totalFiles:0,assigned:{}})});return()=>{active=false}},[selectedIds,refreshToken]);
  useEffect(()=>{
    const request=++linksRequest.current;
    setLinks(null);
    if(primaryId==null)return;
    void invoke<CatalogAssetLinks|null>("get_catalog_asset_links",{assetId:primaryId}).then(value=>{if(linksRequest.current===request)setLinks(value)}).catch(()=>{if(linksRequest.current===request)setLinks(null)});
    return()=>{linksRequest.current++};
  },[primaryId,refreshToken]);

  useEffect(()=>{
    let active=true;
    const load=<T,>(promise:Promise<T>, apply:(value:T)=>void)=>{void promise.then(value=>{if(active)apply(value)});};
    load(invoke<Root[]>("get_category_roots",{includeChildren,offset:0,limit:500}),setRoots);
    load(invoke<ValueCount[]>("get_catalog_values",{kind:"rating",offset:0,limit:20}),values=>{const counts=new Map(values.map(v=>[v.value,v.count]));setRatings(["unrated","1","2","3","4","5"].map(value=>({value,count:counts.get(value)??0})));});
    load(invoke<ValueCount[]>("get_catalog_values",{kind:"label",offset:0,limit:100}),setLabels);
    load(invoke<ValueCount[]>("get_catalog_values",{kind:"special",offset:0,limit:20}),setSpecial);
    load(invoke<{value:string;name:string;hex:string;count:number}[]>("get_color_counts",{}),setColorCounts);
    load(invoke<CatalogNode[]>("get_catalog_nodes",{kind:"keyword",parentId:null,rootId:null,includeChildren,offset:0,limit:500}),setKeywords);
    for(const rootId of openRoots) load(invoke<CatalogNode[]>("get_catalog_nodes",{kind:"category",parentId:null,rootId,includeChildren,offset:0,limit:500}),nodes=>setCats(p=>new Map(p).set(rootId,nodes)));
    const allCategories=[...cats.values(),...catChildren.values()].flat();
    for(const id of openCats){const node=allCategories.find(n=>n.id===id);if(node)load(invoke<CatalogNode[]>("get_catalog_nodes",{kind:"category",parentId:id,rootId:node.rootId,includeChildren,offset:0,limit:500}),nodes=>setCatChildren(p=>new Map(p).set(id,nodes)));}
    const allKeywords=[...keywords,...[...keywordChildren.values()].flat()];
    for(const id of openKeywords){if(allKeywords.some(n=>n.id===id))load(invoke<CatalogNode[]>("get_catalog_nodes",{kind:"keyword",parentId:id,rootId:null,includeChildren,offset:0,limit:500}),nodes=>setKeywordChildren(p=>new Map(p).set(id,nodes)));}
    return()=>{active=false};
  },[includeChildren,refreshToken]);

  useEffect(()=>{
    if(!keywordQuery.trim()){setKeywordResults([]);setCategoryResults([]);return;}
    let active=true;
    const timer=window.setTimeout(()=>{void Promise.all([
      invoke<CatalogNode[]>("search_catalog_tree",{kind:"category",query:keywordQuery,includeChildren,offset:0,limit:500}),
      invoke<CatalogNode[]>("search_catalog_tree",{kind:"keyword",query:keywordQuery,includeChildren,offset:0,limit:500})
    ]).then(([c,k])=>{if(active){setCategoryResults(c);setKeywordResults(k)}});},180);
    return()=>{active=false;window.clearTimeout(timer)};
  },[keywordQuery,includeChildren]);

  const toggleSection=(key:string)=>setCollapsed(p=>({...p,[key]:!p[key]}));
  const toggleRoot=async(root:Root)=>{
    if(openRoots.has(root.id)){setOpenRoots(p=>{const n=new Set(p);n.delete(root.id);return n});return;}
    setOpenRoots(p=>new Set(p).add(root.id));
    if(!cats.has(root.id)){const nodes=await invoke<CatalogNode[]>("get_catalog_nodes",{kind:"category",parentId:null,rootId:root.id,includeChildren,offset:0,limit:500});setCats(p=>new Map(p).set(root.id,nodes));}
  };
  const toggleCat=async(node:CatalogNode)=>{
    if(!node.hasChildren)return;
    if(openCats.has(node.id)){setOpenCats(p=>{const n=new Set(p);n.delete(node.id);return n});return;}
    setOpenCats(p=>new Set(p).add(node.id));
    if(!catChildren.has(node.id)){const nodes=await invoke<CatalogNode[]>("get_catalog_nodes",{kind:"category",parentId:node.id,rootId:node.rootId,includeChildren,offset:0,limit:500});setCatChildren(p=>new Map(p).set(node.id,nodes));}
  };
  const toggleKeyword=async(node:CatalogNode)=>{
    if(!node.hasChildren)return;
    if(openKeywords.has(node.id)){setOpenKeywords(p=>{const n=new Set(p);n.delete(node.id);return n});return;}
    setOpenKeywords(p=>new Set(p).add(node.id));
    if(!keywordChildren.has(node.id)){const nodes=await invoke<CatalogNode[]>("get_catalog_nodes",{kind:"keyword",parentId:node.id,rootId:null,includeChildren,offset:0,limit:500});setKeywordChildren(p=>new Map(p).set(node.id,nodes));}
  };
  const delayLabelClick=(action:()=>void)=>{if(labelTimer.current!==undefined)window.clearTimeout(labelTimer.current);labelTimer.current=window.setTimeout(action,450);};
  const cancelLabelClick=()=>{if(labelTimer.current!==undefined){window.clearTimeout(labelTimer.current);labelTimer.current=undefined;}};
  const navigateCategory=(node:CatalogNode)=>onNavigate({kind:"category",id:node.id,title:`Kategória: ${node.name}`});
  const navigateCategoryRoot=(root:Root)=>onNavigate({kind:"category",id:root.mergedCategoryId??undefined,rootId:root.mergedCategoryId==null?root.id:undefined,title:`Kategória: ${root.name}`});
  const navigateRoot=(root:Root)=>{if(root.mergedCategoryId!=null)navigateCategoryRoot(root);else if(includeChildren)navigateCategoryRoot(root);else void toggleRoot(root);};
  const navigateKeyword=(node:CatalogNode)=>onNavigate({kind:"keyword",id:node.id,title:`Kľúčové slovo: ${node.name}`});
  const filteredKeywords=keywordQuery.trim()?keywordResults:keywords;
  const assignedCategory=(id:number)=>links?.categoryIds.includes(id)??false;
  const categoryAncestor=(id:number)=>links?.categoryAncestorIds.includes(id)??false;
  const rootRelated=(id:number)=>links?.rootIds.includes(id)??false;
  const assignedCount=(id:number)=>selectionCounts.assigned[id]??0;
  const assignCategory=(id:number)=>onSetCategory(selectedIds,id,assignedCount(id)<selectionCounts.totalFiles);
  const finishCategoryMutation=(result:CategoryMutation,deleted=false)=>{if(result.categoryId!=null){setHighlightCategory(result.categoryId);setActiveCategory(prev=>prev?.id===result.categoryId?{...prev,parentId:result.parentId,rootId:result.rootId}:prev);if(result.rootId!=null)setOpenRoots(prev=>new Set(prev).add(result.rootId!));if(result.ancestorIds.length)setOpenCats(prev=>new Set([...prev,...result.ancestorIds]));}if(deleted&&activeCategory&&result.deletedCategoryIds.includes(activeCategory.id))setActiveCategory(null);setCategoryMenu(null);setCategoryDialog(null);setDeleteDialog(null);onCategoryChanged(result,deleted);};
  const runCategoryCommand=async(command:string,args:Record<string,unknown>,deleted=false)=>{try{const result=await invoke<CategoryMutation>(command,args);if(command==="rename_category"&&result.categoryId!=null)setActiveCategory(prev=>prev?.id===result.categoryId?{...prev,name:String(args.name).trim()}:prev);finishCategoryMutation(result,deleted);}catch(error){setCategoryMenu(null);onCategoryError(error);}};
  const saveCategoryDialog=()=>{if(!categoryDialog)return;if(categoryDialog.kind==="create")void runCategoryCommand("create_category",{parentId:categoryDialog.parentId,name:categoryDialog.name});else void runCategoryCommand("rename_category",{id:categoryDialog.id,name:categoryDialog.name});};
  const requestDeleteCategory=async(id:number)=>{setCategoryMenu(null);try{const info=await invoke<{id:number;name:string;assignedFiles:number;subcategories:number}>("get_category_delete_info",{id});setDeleteDialog({info,blocked:info.assignedFiles>0});}catch(error){onCategoryError(error);}};
  const confirmDeleteCategory=()=>{if(deleteDialog)void runCategoryCommand("delete_category",{id:deleteDialog.info.id},true);};
  const categoryDragStart=(event:React.DragEvent,id:number)=>{event.dataTransfer.setData("application/x-ddreams-category-id",String(id));event.dataTransfer.effectAllowed="move";};
  const categoryDragOver=(event:React.DragEvent,id:number)=>{if(!event.dataTransfer.types.includes("application/x-ddreams-category-id")&&!event.dataTransfer.types.includes("application/x-ddreams-asset-ids"))return;event.preventDefault();event.stopPropagation();setDropTarget(id);};
  const categoryDrop=(event:React.DragEvent,id:number)=>{event.preventDefault();event.stopPropagation();setDropTarget(null);setTopDropActive(false);const moving=event.dataTransfer.getData("application/x-ddreams-category-id");if(moving){const source=Number(moving);if(Number.isFinite(source)&&source!==id)void runCategoryCommand("move_category",{id:source,newParentId:id});return;}try{const ids:number[]=JSON.parse(event.dataTransfer.getData("application/x-ddreams-asset-ids"));if(ids.length)onSetCategory(ids,id,true);}catch{}};
  const categoryDragOverTop=(event:React.DragEvent)=>{if(!event.dataTransfer.types.includes("application/x-ddreams-category-id"))return;event.preventDefault();event.stopPropagation();setTopDropActive(true);};
  const categoryDropTop=(event:React.DragEvent)=>{event.preventDefault();event.stopPropagation();setTopDropActive(false);setDropTarget(null);const moving=event.dataTransfer.getData("application/x-ddreams-category-id");if(moving){const id=Number(moving);if(Number.isFinite(id))void runCategoryCommand("move_category",{id,newParentId:null});}};
  const categoryContext=(event:React.MouseEvent,id:number,name:string,parentId:number|null,rootId:number|null)=>{event.preventDefault();event.stopPropagation();const selected={id,name,parentId,rootId};setActiveCategory(selected);setCategoryMenu({x:event.clientX,y:event.clientY,...selected});};
  const keywordAssignedCount=(id:number)=>keywordSelectionCounts.assigned[id]??0;
  const assignKeyword=(id:number)=>onSetKeyword(selectedIds,id,keywordAssignedCount(id)<keywordSelectionCounts.totalFiles);
  const finishKeywordMutation=(result:KeywordMutation,deleted=false)=>{if(result.keywordId!=null){setHighlightKeyword(result.keywordId);setActiveKeyword(prev=>prev?.id===result.keywordId?{...prev,parentId:result.parentId}:prev);if(result.ancestorIds.length)setOpenKeywords(prev=>new Set([...prev,...result.ancestorIds]));}if(deleted&&activeKeyword&&result.deletedKeywordIds.includes(activeKeyword.id))setActiveKeyword(null);setKeywordMenu(null);setKeywordDialog(null);setKeywordDeleteDialog(null);onKeywordChanged(result,deleted);};
  const runKeywordCommand=async(command:string,args:Record<string,unknown>,deleted=false)=>{try{const result=await invoke<KeywordMutation>(command,args);if(command==="rename_keyword"&&result.keywordId!=null)setActiveKeyword(prev=>prev?.id===result.keywordId?{...prev,name:String(args.name).trim()}:prev);finishKeywordMutation(result,deleted);}catch(error){setKeywordMenu(null);onCategoryError(error);}};
  const saveKeywordDialog=()=>{if(!keywordDialog)return;if(keywordDialog.kind==="create")void runKeywordCommand("create_keyword",{parentId:keywordDialog.parentId,name:keywordDialog.name});else void runKeywordCommand("rename_keyword",{id:keywordDialog.id,name:keywordDialog.name});};
  const requestDeleteKeyword=async(id:number)=>{setKeywordMenu(null);try{const info=await invoke<{id:number;name:string;assignedFiles:number;subkeywords:number}>("get_keyword_delete_info",{id});setKeywordDeleteDialog({info,blocked:info.assignedFiles>0});}catch(error){onCategoryError(error);}};
  const confirmDeleteKeyword=()=>{if(keywordDeleteDialog)void runKeywordCommand("delete_keyword",{id:keywordDeleteDialog.info.id},true);};
  const keywordDragStart=(event:React.DragEvent,id:number)=>{event.dataTransfer.setData("application/x-ddreams-keyword-id",String(id));event.dataTransfer.effectAllowed="move";};
  const keywordDragOver=(event:React.DragEvent,id:number)=>{if(!event.dataTransfer.types.includes("application/x-ddreams-keyword-id")&&!event.dataTransfer.types.includes("application/x-ddreams-asset-ids"))return;event.preventDefault();event.stopPropagation();setKeywordDropTarget(id);};
  const keywordDrop=(event:React.DragEvent,id:number)=>{event.preventDefault();event.stopPropagation();setKeywordDropTarget(null);setKeywordTopDrop(false);const moving=event.dataTransfer.getData("application/x-ddreams-keyword-id");if(moving){const source=Number(moving);if(Number.isFinite(source)&&source!==id)void runKeywordCommand("move_keyword",{id:source,newParentId:id});return;}const files=event.dataTransfer.getData("application/x-ddreams-asset-ids");if(files){try{const ids:number[]=JSON.parse(files);if(ids.length)onSetKeyword(ids,id,true);}catch{}}};
  const keywordDragOverTop=(event:React.DragEvent)=>{if(!event.dataTransfer.types.includes("application/x-ddreams-keyword-id"))return;event.preventDefault();event.stopPropagation();setKeywordTopDrop(true);};
  const keywordDropTop=(event:React.DragEvent)=>{event.preventDefault();event.stopPropagation();setKeywordTopDrop(false);setKeywordDropTarget(null);const moving=event.dataTransfer.getData("application/x-ddreams-keyword-id");if(moving){const id=Number(moving);if(Number.isFinite(id))void runKeywordCommand("move_keyword",{id,newParentId:null});}};
  const keywordContext=(event:React.MouseEvent,id:number,name:string,parentId:number|null)=>{event.preventDefault();event.stopPropagation();const selected={id,name,parentId};setActiveKeyword(selected);setKeywordMenu({x:event.clientX,y:event.clientY,...selected});};
  const sectionHead=(key:string,title:string,tools?:ReactNode)=><div className="catalog-section-heading"><button onClick={()=>toggleSection(key)} aria-expanded={!collapsed[key]}><span>{collapsed[key]?"▸":"▾"}</span>{title}</button>{tools}</div>;
  const categoryTools=<><button className="category-add" title="Nová kategória" onClick={()=>setCategoryDialog({kind:"create",parentId:null,name:""})}>+</button><button className="collapse-all" onClick={()=>{setOpenRoots(new Set());setOpenCats(new Set())}}>Zbaliť všetko</button><label className="include-children"><input type="checkbox" checked={includeChildren} onChange={e=>onIncludeChildrenChange(e.target.checked)}/>Vrátane podkategórií</label><label className="include-children" title="Zobraziť štvorčeky na priraďovanie vybraných súborov do kategórií"><input type="checkbox" checked={showAssignSquares} onChange={e=>setShowAssignSquares(e.target.checked)}/>Štvorčeky</label></>;
  const keywordTools=<><button className="category-add" title="Nové kľúčové slovo" onClick={()=>setKeywordDialog({kind:"create",parentId:null,name:""})}>+</button><button className="collapse-all" onClick={()=>setOpenKeywords(new Set())}>Zbaliť všetko</button></>;
  return <div className="catalog-content">
    <section className="catalog-section ai-review-section">{sectionHead("aiSuggestions","NÁVRHY AI")}{!collapsed.aiSuggestions&&<>{!aiReady?<small className="color-not-computed" title={aiReason}>{aiAvailable?"AI sa načítava…":"AI modul nie je nainštalovaný"}</small>:<><div className="ai-review-controls"><button className={suggestionKind==="category"?"active":""} onClick={()=>{setSuggestionKind("category");setSuggestionGroups([]);suggestionsLoaded.current=false}}>Kategórie</button><button className={suggestionKind==="keyword"?"active":""} onClick={()=>{setSuggestionKind("keyword");setSuggestionGroups([]);suggestionsLoaded.current=false}}>Kľúčové slová</button><button disabled={suggestionLoading} onClick={()=>{setSuggestionLoading(true);void invoke<typeof suggestionGroups>("ai_suggestion_groups",{kind:suggestionKind}).then(g=>{suggestionsLoaded.current=true;setSuggestionGroups(g)}).catch(()=>setSuggestionGroups([])).finally(()=>setSuggestionLoading(false))}}>{suggestionLoading?"Načítavam…":"Načítať návrhy"}</button></div>{suggestionGroups.map(group=><button className="value-row" key={group.labelId} title={group.path} onClick={()=>onNavigate({kind:"ai",ids:group.assetIds,title:`Návrh AI: ${group.path}`,suggestion:{kind:suggestionKind,labelId:group.labelId,path:group.path}})}><span>{group.path}</span><small>· {group.count}</small></button>)}</>}</>}</section>
    <input className="keyword-filter" value={keywordQuery} onChange={e=>setKeywordQuery(e.target.value)} placeholder="Filtrovať kategórie a keywordy…" />
    {keywordQuery.trim()&&<section className="catalog-section">{sectionHead("matches","Zhody kategórií")}{!collapsed.matches&&categoryResults.map(node=><button className={`tree-row tree-label keyword-result ${assignedCategory(node.id)?"catalog-assigned":categoryAncestor(node.id)?"catalog-ancestor":""}`} key={node.id} onClick={()=>navigateCategory(node)}><span>{assignedCategory(node.id)?"✓":categoryAncestor(node.id)?"•":""}</span>{node.name}<small>{node.count}</small></button>)}</section>}
    <section className="catalog-section">{sectionHead("special","Špeciálne položky")}{!collapsed.special&&[["all_photos","Všetky fotky"],["uncategorized","Bez kategórie"],["unkeyworded","Bez keywordov"],["tagged","Označené"]].map(([value,label])=><button className="value-row" key={value} onClick={()=>onNavigate({kind:"special",value,title:label})}><span>{label}</span><small>{special.find(s=>s.value===value)?.count??0}</small></button>)}</section>
    <section className="catalog-section" onDragOver={categoryDragOverTop} onDragLeave={()=>setTopDropActive(false)} onDrop={categoryDropTop}>{sectionHead("categories","Kategórie",categoryTools)}{topDropActive&&<div className="category-top-drop">Presunúť na najvyššiu úroveň</div>}{!collapsed.categories&&roots.map(root=>{
      const direct=root.mergedCategoryId!=null&&assignedCategory(root.mergedCategoryId);
      const related=rootRelated(root.id)&&!direct;
      return <div key={root.id}><div className={`tree-row ${root.mergedCategoryId!=null&&dropTarget===root.mergedCategoryId?"category-drop-target":""}`} onDragOver={root.mergedCategoryId==null?undefined:e=>categoryDragOver(e,root.mergedCategoryId!)} onDragLeave={()=>setDropTarget(null)} onDrop={root.mergedCategoryId==null?undefined:e=>categoryDrop(e,root.mergedCategoryId!)}>{showAssignSquares&&root.mergedCategoryId!=null&&<AssignSquare disabled={!selectedIds.length||selectionCounts.totalFiles===0} count={assignedCount(root.mergedCategoryId)} total={selectionCounts.totalFiles} onClick={()=>assignCategory(root.mergedCategoryId!)}/>}<TreeArrow visible={root.hasChildren} open={openRoots.has(root.id)} onClick={()=>void toggleRoot(root)}/><button draggable={root.mergedCategoryId!=null} className={`tree-label ${highlightCategory===root.mergedCategoryId?"category-new-highlight":""} ${direct?"catalog-assigned":related?"catalog-ancestor":""}`} onDragStart={root.mergedCategoryId==null?undefined:e=>categoryDragStart(e,root.mergedCategoryId!)} onContextMenu={root.mergedCategoryId==null?undefined:e=>categoryContext(e,root.mergedCategoryId!,root.name,null,root.id)} onClick={e=>{if(root.mergedCategoryId!=null)setActiveCategory({id:root.mergedCategoryId,name:root.name,parentId:null,rootId:root.id});if(e.detail<=1)delayLabelClick(()=>navigateRoot(root));}} onDoubleClick={()=>{cancelLabelClick();void toggleRoot(root);}}><span className="category-icon">◈</span><span className="tree-text">{direct?"✓ ":related?"• ":""}{root.name}</span><small>{root.count||""}</small></button></div>{openRoots.has(root.id)&&(cats.get(root.id)??[]).map(cat=><CategoryNode key={cat.id} node={cat} depth={1} open={openCats} childrenMap={catChildren} onToggle={toggleCat} onNavigate={navigateCategory} onLabelClick={delayLabelClick} onLabelDoubleClick={cancelLabelClick} assignedIds={new Set(links?.categoryIds??[])} ancestorIds={new Set(links?.categoryAncestorIds??[])} showSquare={showAssignSquares} count={assignedCount} total={selectionCounts.totalFiles} disabled={!selectedIds.length} onAssign={assignCategory} onActivate={setActiveCategory} onContextMenu={categoryContext} onDragStart={categoryDragStart} highlightId={highlightCategory} dropTarget={dropTarget} onDragOver={categoryDragOver} onDragLeave={()=>setDropTarget(null)} onDrop={categoryDrop}/>)}</div>;
    })}</section>
    <section className="catalog-section">{sectionHead("ratings","Hodnotenie")}{!collapsed.ratings&&ratings.map(r=><button className="value-row" key={r.value} onClick={()=>onNavigate({kind:"rating",value:r.value,title:r.value==="unrated"?"Bez hodnotenia":`Hodnotenie ${r.value} ★`})}><span>{r.value==="unrated"?"Bez hodnotenia":"★".repeat(Number(r.value))}</span><small>{r.count}</small></button>)}</section>
    <section className="catalog-section">{sectionHead("labels","Farebné štítky")}{!collapsed.labels&&labels.filter(l=>l.value).map(l=><button className="value-row" key={l.value} onClick={()=>onNavigate({kind:"label",value:l.value,title:`Farebný štítok: ${labelText(l.value)}`})}><span><i className="label-dot" style={{background:labelColor(l.value)}}/>{labelText(l.value)}</span><small>{l.count}</small></button>)}</section>
    <section className="catalog-section">{sectionHead("colors","FARBY")}{!collapsed.colors&&(aiAvailable?colorCounts.map(color=><button className="value-row" key={color.value} onClick={()=>onNavigate({kind:"color",value:color.value,title:`Farba: ${color.name}`})}><span><i className="label-dot" style={{background:color.hex}}/>{color.name}</span><small>{color.count.toLocaleString("sk-SK")}</small></button>):<small className="color-not-computed">AI modul nie je nainštalovaný</small>)}</section>
     <section className="catalog-section keyword-section" onDragOver={keywordDragOverTop} onDragLeave={()=>setKeywordTopDrop(false)} onDrop={keywordDropTop}>{sectionHead("keywords","Kľúčové slová",keywordTools)}{keywordTopDrop&&<div className="category-top-drop">Presunúť na najvyššiu úroveň</div>}{!collapsed.keywords&&filteredKeywords.map(node=><KeywordNode key={node.id} node={node} depth={0} open={openKeywords} childrenMap={keywordChildren} onToggle={toggleKeyword} onNavigate={navigateKeyword} onLabelClick={delayLabelClick} onLabelDoubleClick={cancelLabelClick} assignedIds={new Set(links?.keywordIds??[])} ancestorIds={new Set(links?.keywordAncestorIds??[])} showSquare={showAssignSquares} count={keywordAssignedCount} total={keywordSelectionCounts.totalFiles} disabled={!selectedIds.length} onAssign={assignKeyword} onActivate={setActiveKeyword} onContextMenu={keywordContext} onDragStart={keywordDragStart} highlightId={highlightKeyword} dropTarget={keywordDropTarget} onDragOver={keywordDragOver} onDragLeave={()=>setKeywordDropTarget(null)} onDrop={keywordDrop}/>)}</section>
     {keywordMenu&&<div className="category-context-menu" style={{left:Math.min(keywordMenu.x,window.innerWidth-250),top:Math.min(keywordMenu.y,window.innerHeight-280)}} onMouseLeave={()=>setKeywordMenu(null)}>
       <button onClick={()=>{setKeywordMenu(null);setKeywordDialog({kind:"create",parentId:null,name:""})}}>Nové kľúčové slovo…</button>
       <button onClick={()=>{setKeywordMenu(null);setKeywordDialog({kind:"create",parentId:keywordMenu.id,name:""})}}>Nové podriadené slovo…</button>
       <button onClick={()=>{setKeywordDialog({kind:"rename",id:keywordMenu.id,name:keywordMenu.name});setKeywordMenu(null)}}>Premenovať (F2)</button>
       <button disabled={keywordMenu.parentId==null} onClick={()=>{void runKeywordCommand("move_keyword",{id:keywordMenu.id,newParentId:null});setKeywordMenu(null)}}>Presunúť na najvyššiu úroveň</button>
       <button onClick={()=>void requestDeleteKeyword(keywordMenu.id)}>Zmazať</button><hr />
       <button disabled={!selectedIds.length||keywordSelectionCounts.totalFiles===0||keywordAssignedCount(keywordMenu.id)===keywordSelectionCounts.totalFiles} onClick={()=>{onSetKeyword(selectedIds,keywordMenu.id,true);setKeywordMenu(null)}}>Pridať vybraným súborom</button>
       <button disabled={!selectedIds.length||keywordAssignedCount(keywordMenu.id)===0} onClick={()=>{onSetKeyword(selectedIds,keywordMenu.id,false);setKeywordMenu(null)}}>Odobrať vybraným súborom</button>
     </div>}
     {keywordDialog&&<div className="category-editor-backdrop" onMouseDown={e=>{if(e.target===e.currentTarget)setKeywordDialog(null)}}><form className="category-editor-dialog" onSubmit={e=>{e.preventDefault();saveKeywordDialog()}}><strong>{keywordDialog.kind==="create"?(keywordDialog.parentId==null?"Nové kľúčové slovo":"Nové podriadené slovo"):"Premenovať kľúčové slovo"}</strong><label>Názov<input autoFocus maxLength={100} value={keywordDialog.name} onChange={e=>setKeywordDialog({...keywordDialog,name:e.target.value})} onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();setKeywordDialog(null)}}}/></label><div><button type="submit">OK</button><button type="button" onClick={()=>setKeywordDialog(null)}>Zrušiť</button></div></form></div>}
     {keywordDeleteDialog&&<div className="category-editor-backdrop"><div className="category-editor-dialog"><strong>Zmazať kľúčové slovo</strong>{keywordDeleteDialog.blocked?<><p>Kľúčové slovo {keywordDeleteDialog.info.name} nie je možné zmazať, obsahuje {keywordDeleteDialog.info.assignedFiles} {slovakFileCount(keywordDeleteDialog.info.assignedFiles)} (vrátane podriadených slov). Najprv z neho súbory odoberte.</p><div><button onClick={()=>{onIncludeChildrenChange(true);onNavigate({kind:"keyword",id:keywordDeleteDialog.info.id,title:`Kľúčové slovo: ${keywordDeleteDialog.info.name}`});setKeywordDeleteDialog(null)}}>Zobraziť súbory</button><button onClick={()=>setKeywordDeleteDialog(null)}>Zavrieť</button></div></>:<><p>{keywordDeleteDialog.info.subkeywords>0?`Zmazať kľúčové slovo ${keywordDeleteDialog.info.name} aj s ${keywordDeleteDialog.info.subkeywords} podriadenými slovami?`:`Zmazať kľúčové slovo ${keywordDeleteDialog.info.name}?`}</p><div><button onClick={confirmDeleteKeyword}>Zmazať</button><button onClick={()=>setKeywordDeleteDialog(null)}>Zrušiť</button></div></>}</div></div>}
     {categoryMenu&&<div className="category-context-menu" style={{left:Math.min(categoryMenu.x,window.innerWidth-250),top:Math.min(categoryMenu.y,window.innerHeight-250)}} onMouseLeave={()=>setCategoryMenu(null)}>
       <button onClick={()=>{setCategoryMenu(null);setCategoryDialog({kind:"create",parentId:null,name:""})}}>Nová kategória…</button>
       <button onClick={()=>{setCategoryMenu(null);setCategoryDialog({kind:"create",parentId:categoryMenu.id,name:""})}}>Nová podkategória…</button>
       <button onClick={()=>{setCategoryDialog({kind:"rename",id:categoryMenu.id,name:categoryMenu.name});setCategoryMenu(null)}}>Premenovať (F2)</button>
       <button disabled={categoryMenu.parentId==null} onClick={()=>{void runCategoryCommand("move_category",{id:categoryMenu.id,newParentId:null});setCategoryMenu(null)}}>Presunúť na najvyššiu úroveň</button>
       <button onClick={()=>void requestDeleteCategory(categoryMenu.id)}>Zmazať</button>
       <hr />
       <button disabled={!selectedIds.length||selectionCounts.totalFiles===0||assignedCount(categoryMenu.id)===selectionCounts.totalFiles} onClick={()=>{onSetCategory(selectedIds,categoryMenu.id,true);setCategoryMenu(null)}}>Priradiť vybrané súbory</button><button disabled={!selectedIds.length||assignedCount(categoryMenu.id)===0} onClick={()=>{onSetCategory(selectedIds,categoryMenu.id,false);setCategoryMenu(null)}}>Odobrať vybrané súbory</button>
     </div>}
     {categoryDialog&&<div className="category-editor-backdrop" onMouseDown={e=>{if(e.target===e.currentTarget)setCategoryDialog(null)}}><form className="category-editor-dialog" onSubmit={e=>{e.preventDefault();saveCategoryDialog()}}><strong>{categoryDialog.kind==="create"?(categoryDialog.parentId==null?"Nová kategória":"Nová podkategória"):"Premenovať kategóriu"}</strong><label>Názov<input autoFocus maxLength={100} value={categoryDialog.name} onChange={e=>setCategoryDialog({...categoryDialog,name:e.target.value})} onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();setCategoryDialog(null)}}}/></label><div><button type="submit">OK</button><button type="button" onClick={()=>setCategoryDialog(null)}>Zrušiť</button></div></form></div>}
     {deleteDialog&&<div className="category-editor-backdrop"><div className="category-editor-dialog"><strong>Zmazať kategóriu</strong>{deleteDialog.blocked?<><p>Kategóriu {deleteDialog.info.name} nie je možné zmazať, obsahuje {deleteDialog.info.assignedFiles} {deleteDialog.info.assignedFiles===1?"súbor":deleteDialog.info.assignedFiles<=4?"súbory":"súborov"} (vrátane podkategórií). Najprv z nej súbory odoberte.</p><div><button onClick={()=>{onIncludeChildrenChange(true);onNavigate({kind:"category",id:deleteDialog.info.id,title:`Kategória: ${deleteDialog.info.name}`});setDeleteDialog(null)}}>Zobraziť súbory</button><button onClick={()=>setDeleteDialog(null)}>Zavrieť</button></div></>:<><p>{deleteDialog.info.subcategories>0?`Zmazať kategóriu ${deleteDialog.info.name} aj s ${deleteDialog.info.subcategories} ${deleteDialog.info.subcategories===1?"podkategóriou":"podkategóriami"}?`:`Zmazať kategóriu ${deleteDialog.info.name}?`}</p><div><button onClick={confirmDeleteCategory}>Zmazať</button><button onClick={()=>setDeleteDialog(null)}>Zrušiť</button></div></>}</div></div>}
  </div>;
}
function TreeArrow({visible,open,onClick}:{visible:boolean;open:boolean;onClick:()=>void}){return visible?<button className="tree-toggle" aria-label={open?"Zbaliť":"Rozbaliť"} onClick={onClick}>{open?"▾":"▸"}</button>:<span className="tree-toggle-placeholder"/>;}
function slovakFileCount(value:number){return value===1?"súbor":value>=2&&value<=4?"súbory":"súborov";}
function AssignSquare({disabled,count,total,onClick,noun="kategórie"}:{disabled:boolean;count:number;total:number;onClick:()=>void;noun?:string}){const checked=total>0&&count===total,partial=count>0&&!checked;return <button type="button" role="checkbox" aria-checked={partial?"mixed":checked} disabled={disabled} className={`category-assign-box ${checked?"checked":""} ${partial?"partial":""}`} title={partial?`${count} z ${total} súborov priradených`:checked?`Odobrať z ${noun}`:`Priradiť do ${noun}`} onClick={e=>{e.stopPropagation();onClick()}}>{checked?"✓":partial?"–":""}</button>}
function CategoryNode({node,depth,open,childrenMap,onToggle,onNavigate,onLabelClick,onLabelDoubleClick,assignedIds,ancestorIds,showSquare,count,total,disabled,onAssign,onActivate,onContextMenu,onDragStart,highlightId,dropTarget,onDragOver,onDragLeave,onDrop}:{node:CatalogNode;depth:number;open:Set<number>;childrenMap:Map<number,CatalogNode[]>;onToggle:(node:CatalogNode)=>void;onNavigate:(node:CatalogNode)=>void;onLabelClick:(action:()=>void)=>void;onLabelDoubleClick:()=>void;assignedIds:Set<number>;ancestorIds:Set<number>;showSquare:boolean;count:(id:number)=>number;total:number;disabled:boolean;onAssign:(id:number)=>void;onActivate:(value:{id:number;name:string;parentId:number|null;rootId:number|null})=>void;onContextMenu:(event:React.MouseEvent,id:number,name:string,parentId:number|null,rootId:number|null)=>void;onDragStart:(event:React.DragEvent,id:number)=>void;highlightId:number|null;dropTarget:number|null;onDragOver:(event:React.DragEvent,id:number)=>void;onDragLeave:()=>void;onDrop:(event:React.DragEvent,id:number)=>void}){const assigned=assignedIds.has(node.id),ancestor=!assigned&&ancestorIds.has(node.id);return <div><div className={`tree-row ${dropTarget===node.id?"category-drop-target":""}`} style={{paddingLeft:8+depth*14}} onDragOver={e=>onDragOver(e,node.id)} onDragLeave={onDragLeave} onDrop={e=>onDrop(e,node.id)}>{showSquare&&<AssignSquare disabled={disabled||total===0} count={count(node.id)} total={total} onClick={()=>onAssign(node.id)}/>}<TreeArrow visible={node.hasChildren} open={open.has(node.id)} onClick={()=>onToggle(node)}/><button draggable className={`tree-label ${highlightId===node.id?"category-new-highlight":""} ${assigned?"catalog-assigned":ancestor?"catalog-ancestor":""}`} onDragStart={e=>onDragStart(e,node.id)} onContextMenu={e=>onContextMenu(e,node.id,node.name,node.parentId,node.rootId)} onClick={e=>{onActivate({id:node.id,name:node.name,parentId:node.parentId,rootId:node.rootId});if(e.detail<=1)onLabelClick(()=>onNavigate(node));}} onDoubleClick={()=>{onLabelDoubleClick();onToggle(node);}}><span className="category-icon">{assigned?"✓":ancestor?"•":"▤"}</span><span className="tree-text">{node.name}</span><small>{node.count||""}</small></button></div>{open.has(node.id)&&(childrenMap.get(node.id)??[]).map(child=><CategoryNode key={child.id} node={child} depth={depth+1} open={open} childrenMap={childrenMap} onToggle={onToggle} onNavigate={onNavigate} onLabelClick={onLabelClick} onLabelDoubleClick={onLabelDoubleClick} assignedIds={assignedIds} ancestorIds={ancestorIds} showSquare={showSquare} count={count} total={total} disabled={disabled} onAssign={onAssign} onActivate={onActivate} onContextMenu={onContextMenu} onDragStart={onDragStart} highlightId={highlightId} dropTarget={dropTarget} onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}/>)}</div>}
function KeywordProperties({links,primaryId,selectedIds,onSetKeyword,onCreateAssignKeyword,readOnly,onError}:{links:CatalogAssetLinks|null;primaryId:number|null;selectedIds:number[];onSetKeyword:(ids:number[],keywordId:number,assign:boolean)=>void;onCreateAssignKeyword:(ids:number[],parentId:number|null,name:string)=>void;readOnly:boolean;onError:(error:unknown)=>void}){
  const [text,setText]=useState("");const [options,setOptions]=useState<{id:number;name:string;path:string}[]>([]);const [offerCreate,setOfferCreate]=useState(false);const ids=selectedIds.length?selectedIds:primaryId==null?[]:[primaryId];
  useEffect(()=>{const query=text.trim();if(!query){setOptions([]);setOfferCreate(false);return;}let active=true;const timer=window.setTimeout(()=>{void invoke<{id:number;name:string;path:string}[]>("search_keyword_options",{query}).then(rows=>{if(active)setOptions(rows)}).catch(error=>{if(active){setOptions([]);onError(error)}})},140);return()=>{active=false;window.clearTimeout(timer)}},[text]);
  const exact=options.find(item=>item.name.toLowerCase()===text.trim().toLowerCase());const add=(id:number)=>{if(!ids.length||readOnly)return;onSetKeyword(ids,id,true);setText("");setOptions([]);setOfferCreate(false)};const create=()=>{if(!ids.length||readOnly||!text.trim())return;onCreateAssignKeyword(ids,null,text.trim());setText("");setOptions([]);setOfferCreate(false)};
  return <div className="property-keyword-editor" onClick={event=>event.stopPropagation()}><div className="property-keyword-chips">{(links?.keywordPaths??[]).map(keyword=><span className="property-keyword-chip" key={keyword.id} title={keyword.path}>{keyword.path}<button disabled={readOnly||!ids.length} aria-label={`Odobrať kľúčové slovo ${keyword.path}`} onClick={()=>onSetKeyword(ids,keyword.id,false)}>×</button></span>)}</div><div className="keyword-autocomplete"><input disabled={readOnly} value={text} placeholder="Pridať kľúčové slovo…" onChange={event=>{setText(event.target.value);setOfferCreate(false)}} onKeyDown={event=>{if(event.key==="Enter"){event.preventDefault();if(exact){add(exact.id);return;}const query=text.trim();if(!query)return;void invoke<{id:number;name:string;path:string}[]>("search_keyword_options",{query}).then(rows=>{setOptions(rows);const match=rows.find(item=>item.name.toLowerCase()===query.toLowerCase());if(match)add(match.id);else setOfferCreate(true)}).catch(onError)}else if(event.key==="Escape"){setText("");setOptions([]);setOfferCreate(false)}}}/>{text.trim()&&<div className="keyword-autocomplete-options">{options.map(item=><button type="button" key={item.id} onMouseDown={event=>event.preventDefault()} onClick={()=>add(item.id)}><strong>{item.name}</strong>{item.path!==item.name&&<small>{item.path}</small>}</button>)}{offerCreate&&!exact&&<button type="button" disabled={!ids.length} onMouseDown={event=>event.preventDefault()} onClick={create}>Vytvoriť kľúčové slovo „{text.trim()}“ a pridať</button>}</div>}</div></div>
}

function KeywordNode({node,depth,open,childrenMap,onToggle,onNavigate,onLabelClick,onLabelDoubleClick,assignedIds,ancestorIds,showSquare,count,total,disabled,onAssign,onActivate,onContextMenu,onDragStart,highlightId,dropTarget,onDragOver,onDragLeave,onDrop}:{node:CatalogNode;depth:number;open:Set<number>;childrenMap:Map<number,CatalogNode[]>;onToggle:(node:CatalogNode)=>void;onNavigate:(node:CatalogNode)=>void;onLabelClick:(action:()=>void)=>void;onLabelDoubleClick:()=>void;assignedIds:Set<number>;ancestorIds:Set<number>;showSquare:boolean;count:(id:number)=>number;total:number;disabled:boolean;onAssign:(id:number)=>void;onActivate:(value:{id:number;name:string;parentId:number|null})=>void;onContextMenu:(event:React.MouseEvent,id:number,name:string,parentId:number|null)=>void;onDragStart:(event:React.DragEvent,id:number)=>void;highlightId:number|null;dropTarget:number|null;onDragOver:(event:React.DragEvent,id:number)=>void;onDragLeave:()=>void;onDrop:(event:React.DragEvent,id:number)=>void}){
  const assigned=assignedIds.has(node.id),ancestor=!assigned&&ancestorIds.has(node.id);return <div><div className={`tree-row ${dropTarget===node.id?"category-drop-target":""}`} style={{paddingLeft:8+depth*14}} onDragOver={event=>onDragOver(event,node.id)} onDragLeave={onDragLeave} onDrop={event=>onDrop(event,node.id)}>{showSquare&&<AssignSquare disabled={disabled||total===0} count={count(node.id)} total={total} onClick={()=>onAssign(node.id)} noun="kľúčového slova"/>}<TreeArrow visible={node.hasChildren} open={open.has(node.id)} onClick={()=>onToggle(node)}/><button draggable className={`tree-label ${highlightId===node.id?"category-new-highlight":""} ${assigned?"catalog-assigned":ancestor?"catalog-ancestor":""}`} onDragStart={event=>onDragStart(event,node.id)} onContextMenu={event=>onContextMenu(event,node.id,node.name,node.parentId)} onClick={event=>{onActivate({id:node.id,name:node.name,parentId:node.parentId});if(event.detail<=1)onLabelClick(()=>onNavigate(node));}} onDoubleClick={()=>{onLabelDoubleClick();onToggle(node);}}><span className="keyword-icon">{assigned?"✓":ancestor?"•":"⌑"}</span><span className="tree-text">{node.name}</span><small>{node.count||""}</small></button></div>{open.has(node.id)&&(childrenMap.get(node.id)??[]).map(child=><KeywordNode key={child.id} node={child} depth={depth+1} open={open} childrenMap={childrenMap} onToggle={onToggle} onNavigate={onNavigate} onLabelClick={onLabelClick} onLabelDoubleClick={onLabelDoubleClick} assignedIds={assignedIds} ancestorIds={ancestorIds} showSquare={showSquare} count={count} total={total} disabled={disabled} onAssign={onAssign} onActivate={onActivate} onContextMenu={onContextMenu} onDragStart={onDragStart} highlightId={highlightId} dropTarget={dropTarget} onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}/>)}</div>
}

import { useEffect, useMemo, useRef, useState, type MouseEvent, type DragEvent as ReactDragEvent,
  type KeyboardEvent as ReactKeyboardEvent, type ReactElement, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AssetQuality, CatalogAssetLinks, Metadata, MetadataRow, ValueCount, View } from "../types";
import { labelColor, labelText, type AssetValuePatch } from "./AssetGrid";
import { readStored, writeStored } from "../storage";
import { propertySection } from "../dockLayout";
import { DockChrome, DockSection, SectionHeading, useRegisterSections, useSectionCollapse } from "./Dock";
import { AiPanel } from "./AiPanel";
import { CatalogPanel, type CatalogValueTokens } from "./CatalogPanel";
import { RightSectionContext, useRightSectionData } from "./rightSectionContext";
import { fetchFaceThumbs, type AssetColorInfo, type AiSuggestion, type CategoryMutation,
  type FaceInfo, type FacePerson, type FaceStatus, type KeywordMutation } from "./rightPanelData";

type Props = { primaryReadId: number | null; selectedReadIds: number[]; labelCounts: ValueCount[]; primaryValues?: AssetValuePatch; valueTokens: CatalogValueTokens; incomingTools?: ReactNode; thumbEpoch?: number; catalogReady?: boolean; activeColors?: string; activeQuality?: string; primaryId: number | null; selectedIds: number[]; includeChildren: boolean; onIncludeChildrenChange: (value: boolean) => void; onNavigate: (view: View) => void; onSetValues:(ids:number[],changes:Record<string,unknown>)=>void; onSetColors:(ids:number[],color:string,state:boolean|null)=>void; onSetCategory:(ids:number[],categoryId:number,assign:boolean)=>void|Promise<boolean>; onCategoryChanged:(result:CategoryMutation,deleted:boolean)=>void; onRefresh:()=>void; onClusterNotice:(message:string)=>void; onCategoryError:(error:unknown)=>void; onSetKeyword:(ids:number[],keywordId:number,assign:boolean)=>Promise<boolean>; onCreateAssignKeyword:(ids:number[],parentId:number|null,name:string)=>void; onKeywordChanged:(result:KeywordMutation,deleted:boolean)=>void; readOnly:boolean; refreshToken: number; catalogRefreshToken?: number; colorToken?: number;aiAvailable:boolean;aiReady:boolean;aiReason:string };
function useRightPanelModel({ primaryReadId, selectedReadIds, labelCounts, primaryValues, valueTokens, incomingTools, thumbEpoch = 0, catalogReady = true, activeColors = "", activeQuality = "", primaryId, selectedIds, includeChildren, onIncludeChildrenChange, onNavigate, onSetValues, onSetColors, onSetCategory, onCategoryChanged, onRefresh, onClusterNotice, onSetKeyword, onCreateAssignKeyword, onKeywordChanged, onCategoryError, readOnly, refreshToken, catalogRefreshToken = 0, colorToken = 0,aiAvailable,aiReady,aiReason }: Props) {
  const [loadedMetadata, setMetadata] = useState<Metadata | null>(null);
  const metadata = useMemo(() => {
    if (!loadedMetadata || !primaryValues || (loadedMetadata.asset.id !== primaryValues.id && loadedMetadata.asset.id !== primaryReadId)) return loadedMetadata;
    const rows = loadedMetadata.rows.map(row => {
      if (row.group !== "Katalóg") return row;
      if (row.name === "Hodnotenie") return { ...row, value: String(primaryValues.rating ?? "") };
      if (row.name === "Farebný štítok") return { ...row, value: primaryValues.label ?? "" };
      if (row.name === "Označené") return { ...row, value: primaryValues.tagged ? "Áno" : "Nie" };
      return row;
    });
    return { ...loadedMetadata, asset: { ...loadedMetadata.asset, ...primaryValues }, rows };
  }, [loadedMetadata, primaryValues, primaryReadId]);
  const [assetLinks,setAssetLinks]=useState<CatalogAssetLinks|null>(null);
  const [loading, setLoading] = useState(false);
  const { isCollapsed } = useSectionCollapse();
  const [selectedRows, setSelectedRows] = useState<string[]>(() => { const v = readStored<string[]>("ddreams.metadataChecked", []); return Array.isArray(v) ? v : []; });
  useEffect(() => writeStored("ddreams.metadataChecked", selectedRows), [selectedRows]);
  const [rowAnchor, setRowAnchor] = useState("");
  const [copyMenu, setCopyMenu] = useState(false);
  const [copyMessage, setCopyMessage] = useState("");
  const [assetColors,setAssetColors]=useState<AssetColorInfo[]>([]);
  const [assetQuality,setAssetQuality]=useState<AssetQuality[]>([]);
  const [facePersons,setFacePersons]=useState<FacePerson[]>([]);
  const [faceStatus,setFaceStatus]=useState<FaceStatus>({available:false,scanned:0,pending:0,faces:0,named:0,unknown:0,persons:0});
  const [assetFaces,setAssetFaces]=useState<FaceInfo[]>([]);
  const [assetFaceThumbs,setAssetFaceThumbs]=useState<Record<number,string>>({});
  const assignedAssetIds = useMemo(() => selectedIds.length ? selectedIds : primaryId == null ? [] : [primaryId], [selectedIds, primaryId]);
  const suggestionIds = useMemo(() => assignedAssetIds.slice(0, 200), [assignedAssetIds]);
  const selectionReadKey = useMemo(() => selectedReadIds.join(","), [selectedReadIds]);
  const suggestionKey = useMemo(() => selectedReadIds.length
    ? selectedReadIds.slice(0, 200).join(",") : String(primaryReadId ?? ""), [selectedReadIds, primaryReadId]);
  const sampled = assignedAssetIds.length > suggestionIds.length;
  const sampleSet = useMemo(() => new Set(suggestionIds), [suggestionIds]);
  const [suggestionResult, setSuggestionResult] = useState<{key:string; category:AiSuggestion[]; keyword:AiSuggestion[]} | null>(null);
  const aiSuggestions = aiReady && suggestionResult?.key === suggestionKey ? suggestionResult : {category:[], keyword:[]};
  const aiSuggestDone = aiReady && suggestionResult?.key === suggestionKey;
  useEffect(() => {
    setSuggestionResult(null);
    if (!aiReady || !suggestionIds.length) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void Promise.all([
        invoke<AiSuggestion[]>("ai_suggest", {assetIds: suggestionIds, kind: "category"}),
        invoke<AiSuggestion[]>("ai_suggest", {assetIds: suggestionIds, kind: "keyword"}),
      ]).then(([category, keyword]) => {
        if (active) setSuggestionResult({key: suggestionKey, category, keyword});
      }).catch(() => { if (active) setSuggestionResult(null); });
    }, 300);
    return () => { active = false; window.clearTimeout(timer); };
  }, [suggestionKey, aiReady, refreshToken]);
  const removeAiSuggestion = (kind: "category" | "keyword", labelId: number) => {
    setSuggestionResult(current => current?.key === suggestionKey
      ? { ...current, [kind]: current[kind].filter(row => row.labelId !== labelId) }
      : current);
  };
  const colorIds = assignedAssetIds;
  const facePersonsRequest=useRef(0);
  const facePersonsLastLoad=useRef(0);
  const facePersonsTimer=useRef<number|undefined>(undefined);
  const reloadFacePersons=async()=>{const request=++facePersonsRequest.current;facePersonsLastLoad.current=Date.now();if(facePersonsTimer.current!==undefined){window.clearTimeout(facePersonsTimer.current);facePersonsTimer.current=undefined}try{const [status,result]=await Promise.all([invoke<FaceStatus>('ai_faces_status',{}),invoke<{persons:FacePerson[]}>('ai_faces_persons',{})]);if(facePersonsRequest.current===request){setFaceStatus(status);setFacePersons(result.persons??[])}}catch{if(facePersonsRequest.current===request){setFacePersons([]);setFaceStatus({available:false,scanned:0,pending:0,faces:0,named:0,unknown:0,persons:0})}}};
  const [assetPersonCounts,setAssetPersonCounts]=useState<Record<number,number>>({});
  const [personInput,setPersonInput]=useState("");
  const faceRequest=useRef(0);
  const selectionKey=`${primaryReadId}:${suggestionKey}`;
  const currentSelection=useRef(selectionKey);
  currentSelection.current=selectionKey;
  const reloadAssignedPeople=async()=>{const key=selectionKey;if(currentSelection.current!==key)return;const request=++faceRequest.current;const valid=()=>faceRequest.current===request&&currentSelection.current===key;const ids=primaryId!=null&&!sampleSet.has(primaryId)?[...suggestionIds,primaryId]:suggestionIds;if(!ids.length){if(valid()){setAssetPersonCounts({});setAssetFaces([]);setAssetFaceThumbs({})}return}try{const byPerson=new Map<number,Set<number>>();const primaryRows:FaceInfo[]=[];for(let start=0;start<ids.length;start+=500){const rows=await invoke<FaceInfo[]>("ai_faces_for_assets",{assetIds:ids.slice(start,start+500)});if(!valid())return;for(const row of rows){if(row.assetId===primaryId)primaryRows.push(row);if(row.personId!=null&&sampleSet.has(row.assetId)){const assets=byPerson.get(row.personId)??new Set<number>();assets.add(row.assetId);byPerson.set(row.personId,assets)}}}const counts:Record<number,number>={};for(const [person,assets] of byPerson)counts[person]=assets.size;if(!valid())return;setAssetPersonCounts(counts);setAssetFaces(primaryRows);setAssetFaceThumbs({});const thumbs=await fetchFaceThumbs(primaryRows.map(row=>row.faceId));if(valid())setAssetFaceThumbs(thumbs)}catch{if(valid()){setAssetPersonCounts({});setAssetFaces([]);setAssetFaceThumbs({})}}};
  useEffect(()=>{++faceRequest.current;setAssetPersonCounts({});setAssetFaces([]);setAssetFaceThumbs({});const timer=window.setTimeout(()=>void reloadAssignedPeople(),120);return()=>{window.clearTimeout(timer);++faceRequest.current}},[selectionKey,refreshToken,catalogRefreshToken]);
  const assignPersonToSelection=async(input=personInput)=>{const name=input.trim();if(!name||!assignedAssetIds.length||readOnly||!faceStatus.available)return;try{let person=facePersons.find(p=>p.name.toLocaleLowerCase("sk")===name.toLocaleLowerCase("sk"));if(!person){const id=await invoke<number>("ai_faces_person_create",{name});person={id,name,confirmed:0,auto:0,assets:0,cover:null}}let n=0;for(let start=0;start<assignedAssetIds.length;start+=2000)n+=await invoke<number>("ai_faces_asset_assign",{assetIds:assignedAssetIds.slice(start,start+2000),personId:person.id});setPersonInput("");await Promise.all([reloadAssignedPeople(),reloadFacePersons()]);onRefresh();onClusterNotice(`Osoba ${person.name} pridaná: ${n} fotiek`)}catch(error){onCategoryError(error)}};
  const removePersonFromSelection=async(person:FacePerson)=>{if(readOnly||!faceStatus.available)return;try{for(let start=0;start<assignedAssetIds.length;start+=2000)await invoke<number>("ai_faces_asset_remove",{assetIds:assignedAssetIds.slice(start,start+2000),personId:person.id});await Promise.all([reloadAssignedPeople(),reloadFacePersons()]);onRefresh();onClusterNotice(`Osoba ${person.name} odobraná`)}catch(error){onCategoryError(error)}};

  const metadataRequest = useRef(0);
  const toastTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const requestId = ++metadataRequest.current;
    setMetadata(null); if (primaryId == null) return;
    const timer = window.setTimeout(() => { setLoading(true); void invoke<Metadata | null>("get_metadata", { assetId: primaryId }).then(value=>{if(metadataRequest.current===requestId)setMetadata(value)}).catch(e=>{if(metadataRequest.current===requestId)setCopyMessage(String(e))}).finally(()=>{if(metadataRequest.current===requestId)setLoading(false)}); }, 260);
    return () => { window.clearTimeout(timer); metadataRequest.current++; };
  }, [primaryReadId, refreshToken]);
  useEffect(()=>{let active=true;setAssetColors([]);if(!colorIds.length)return;const timer=window.setTimeout(()=>{void invoke<AssetColorInfo[]>("get_asset_colors",{assetIds:colorIds}).then(v=>{if(active)setAssetColors(v)}).catch(()=>{if(active)setAssetColors([])})},120);return()=>{active=false;window.clearTimeout(timer)}},[selectionReadKey,primaryReadId,refreshToken,colorToken]);
  useEffect(()=>{let active=true;setAssetQuality([]);if(primaryId==null)return;const timer=window.setTimeout(()=>{void invoke<AssetQuality[]>("get_asset_quality",{assetIds:[primaryId]}).then(rows=>{if(active)setAssetQuality(rows)}).catch(()=>{if(active)setAssetQuality([])})},120);return()=>{active=false;window.clearTimeout(timer)}},[primaryReadId,refreshToken,colorToken]);
  useEffect(()=>{let active=true;setAssetLinks(null);if(primaryId==null)return;const timer=window.setTimeout(()=>{void invoke<CatalogAssetLinks|null>("get_catalog_asset_links",{assetId:primaryId}).then(v=>{if(active)setAssetLinks(v)}).catch(()=>{if(active)setAssetLinks(null)})},120);return()=>{active=false;window.clearTimeout(timer)}},[primaryReadId,refreshToken]);
  useEffect(()=>{if(aiAvailable&&aiReady)void reloadFacePersons();else{++facePersonsRequest.current;if(facePersonsTimer.current!==undefined){window.clearTimeout(facePersonsTimer.current);facePersonsTimer.current=undefined}setFacePersons([]);setFaceStatus({available:false,scanned:0,pending:0,faces:0,named:0,unknown:0,persons:0})}return()=>{++facePersonsRequest.current;if(facePersonsTimer.current!==undefined){window.clearTimeout(facePersonsTimer.current);facePersonsTimer.current=undefined}}},[aiAvailable,catalogRefreshToken,aiReady]);
  const previousColorToken=useRef(colorToken);
  useEffect(()=>{if(previousColorToken.current===colorToken)return;previousColorToken.current=colorToken;if(!aiAvailable||!aiReady||facePersonsTimer.current!==undefined)return;const delay=Math.max(0,20000-(Date.now()-facePersonsLastLoad.current));facePersonsTimer.current=window.setTimeout(()=>{facePersonsTimer.current=undefined;void reloadFacePersons()},delay)},[colorToken,aiAvailable,aiReady]);
  // User-defined row order, per group + subgroup, as a list of row names. Names missing from the
  // list keep their original order after the listed ones.
  const [rowOrder,setRowOrder]=useState<Record<string,string[]>>(()=>{const v=readStored<Record<string,string[]>>("ddreams.metadataOrder",{});return v&&typeof v==="object"&&!Array.isArray(v)?v:{};});
  useEffect(()=>writeStored("ddreams.metadataOrder",rowOrder),[rowOrder]);
  const bucketKey=(row:MetadataRow)=>`${row.group}\u0000${row.subgroup}`;
  const orderRows=(rows:MetadataRow[])=>{const first=new Map<string,number>();rows.forEach((r,i)=>{const b=bucketKey(r);if(!first.has(b))first.set(b,i)});const rank=(r:MetadataRow)=>{const i=rowOrder[bucketKey(r)]?.indexOf(r.name)??-1;return i<0?Number.MAX_SAFE_INTEGER:i};return rows.map((row,i)=>({row,i})).sort((a,b)=>first.get(bucketKey(a.row))!-first.get(bucketKey(b.row))!||rank(a.row)-rank(b.row)||a.i-b.i).map(x=>x.row)};
  const dragRow=useRef<MetadataRow|null>(null);
  const [dropMark,setDropMark]=useState<{key:string;after:boolean}|null>(null);
  const groups = useMemo(() => {
    const rows = orderRows(metadata?.rows ?? []);
    const fixed = ["IPTC", "EXIF", "Katalóg"];
    const extra = [...new Set(rows.map(row => row.group).filter(name => !fixed.includes(name)))].sort((a,b) => {
      if (a === "Všetky metadáta") return 1; if (b === "Všetky metadáta") return -1; return a.localeCompare(b);
    });
    return [...fixed, ...extra].map(name => ({name, rows: rows.filter(row => row.group === name)})).filter(group => group.rows.length);
  }, [metadata, rowOrder]);
  const propertyGroups = useMemo(() => groups.map(group => propertySection(group.name)), [groups]);
  useRegisterSections(propertyGroups);
  const rowKey = (row: MetadataRow) => `${row.group}\u0000${row.subgroup}\u0000${row.name}`;
  const rowList = groups.flatMap(g => g.rows);
  const dropAfter=(event:ReactDragEvent<HTMLElement>)=>{const box=event.currentTarget.getBoundingClientRect();return event.clientY>box.top+box.height/2};
  const moveRow=(target:MetadataRow,after:boolean)=>{
    const source=dragRow.current;dragRow.current=null;setDropMark(null);
    if(!source||bucketKey(source)!==bucketKey(target)||source.name===target.name)return;
    const bucket=bucketKey(target);const visible=rowList.filter(r=>bucketKey(r)===bucket).map(r=>r.name);
    setRowOrder(prev=>{const stored=prev[bucket]??[];const list=[...stored,...visible.filter(n=>!stored.includes(n))].filter(n=>n!==source.name);const at=list.indexOf(target.name);list.splice(at<0?list.length:at+(after?1:0),0,source.name);return{...prev,[bucket]:list}});
  };
  const selectRow = (row: MetadataRow, event: MouseEvent) => {
    const key = rowKey(row); setRowAnchor(key);
    if (event.shiftKey && rowAnchor) {
      const a = rowList.findIndex(r => rowKey(r) === rowAnchor), b = rowList.findIndex(r => rowKey(r) === key);
      const range = rowList.slice(Math.min(a,b),Math.max(a,b)+1).map(rowKey);
      setSelectedRows(prev => [...new Set([...prev, ...range])]);
    } else setSelectedRows(prev => prev.includes(key) ? prev.filter(v => v !== key) : [...prev,key]);
  };
  const acceptAiSuggestion = async (kind: "category" | "keyword", item: AiSuggestion) => {
    const ids = (item.assetIds ?? suggestionIds).filter(id => sampleSet.has(id));
    if (readOnly || !ids.length) return;
    const assigned = kind === "category"
      ? await onSetCategory(ids, item.labelId, true)
      : await onSetKeyword(ids, item.labelId, true);
    if (assigned && currentSelection.current === selectionKey) {
      removeAiSuggestion(kind, item.labelId);
    }
  };
  const copy = async (mode: "selected" | "values" | "section", group?: string) => {
    const ids = selectedIds.length > 1 ? selectedIds : primaryId == null ? [] : [primaryId];
    if (!ids.length) return;
    const keys = mode === "section" ? null : selectedRows;
    if (keys && !keys.length) return;
    const payloads = await Promise.all(ids.map(assetId => invoke<Metadata | null>("get_metadata", { assetId })));
    let rowCount = 0;
    const blocks = payloads.filter((m): m is Metadata => !!m).map(m => {
      const rows = orderRows(m.rows).filter(r =>(!group || r.group === group) && (keys == null || keys.includes(rowKey(r))));
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
  const editIds=selectedIds.length?selectedIds:primaryId==null?[]:[primaryId];
  const catalogValue=(name:string)=>metadata?.rows.find(r=>r.group==="Katalóg"&&r.name===name)?.value??"";
  // Properties are read-only: catalogue values shown as text; editing lives on the Katalóg tab.
  const catalogText=(row:MetadataRow)=>{if(row.name==="Hodnotenie"){const n=Math.min(5,Number(row.value)||0);return n?"★".repeat(n):"—";}if(row.name==="Farebný štítok")return row.value&&row.value!=="-"?labelText(row.value):"—";return row.value||"—";};
  const colorNames=(assetColors[0]?.colors??[]).filter(c=>c.effective).map(c=>c.name).join(", ");
  const quality=assetQuality.find(item=>item.assetId===primaryId);
  const qualityText=quality?.flags.map(flag=>({blur:"Neostrá",over:"Prepálená",under:"Tmavá"}[flag]??flag)).join(", ")||"V poriadku";
  const qualityTitle=quality?`ostrosť: ${quality.sharp.toFixed(1)}, svetlá: ${(quality.hi*100).toFixed(1)} %, tiene: ${(quality.lo*100).toFixed(1)} %, jas: ${(quality.mean*100).toFixed(1)} %`:"";
  const editorRow=(name:string,content:ReactNode)=><div className="meta-row editor-row"><span>{name}</span><b>{content}</b></div>;
  const ratingValue = primaryValues ? primaryValues.rating ?? 0 : Number(catalogValue("Hodnotenie")) || 0;
  const labelValue = primaryValues ? primaryValues.label ?? "" : catalogValue("Farebný štítok");
  const taggedValue = primaryValues ? !!primaryValues.tagged : catalogValue("Označené") === "Áno";
  const pendingSince = primaryValues && "editPendingSince" in primaryValues ? primaryValues.editPendingSince : metadata?.asset.editPendingSince;
  const pendingDays = pendingSince == null ? null : Math.max(0, Math.floor((Date.now() / 1000 - pendingSince) / 86400));
  const catalogEditor=!editIds.length?<div className="catalog-editor empty"><div className="catalog-zone-title edit-zone">Zadávanie – mení vybrané fotky</div><small>Vyber súbory, ktoré chceš triediť.</small></div>:<section className="catalog-editor">
    <div className="catalog-zone-title edit-zone">Zadávanie – mení vybrané fotky</div><div className="catalog-editor-title">{selectedIds.length>1?`${selectedIds.length} vybraných súborov`:(metadata?.asset.name??"Vybraný súbor")}</div>
    {editorRow("Hodnotenie",<span className="property-rating">{[1,2,3,4,5].map(value=><button key={value} disabled={readOnly} className={ratingValue>=value?"active":""} title={`${value} / 5`} onClick={()=>onSetValues(editIds,{rating:value})}>★</button>)}<button disabled={readOnly} title="Bez hodnotenia" onClick={()=>onSetValues(editIds,{rating:null})}>×</button></span>)}
    {editorRow("Farebný štítok",<span className="property-labels">{[["Red","Červená"],["Yellow","Žltá"],["Green","Zelená"],["Blue","Modrá"],["Purple","Fialová"],["Gray","Na vyradenie"]].map(([value,label])=><button key={value} disabled={readOnly} title={label} className={labelValue.toLowerCase()===value.toLowerCase()?"active":""} onClick={()=>onSetValues(editIds,{label:value})}><i style={{background:labelColor(value)}}/></button>)}<button disabled={readOnly} title="Bez štítku" onClick={()=>onSetValues(editIds,{label:null})}>×</button></span>)}
    {editorRow("Označené",<input type="checkbox" disabled={readOnly} checked={taggedValue} onChange={e=>onSetValues(editIds,{tagged:e.target.checked})}/>)}
    {!metadata?.asset.isDirectory && editorRow("Na úpravu", <span className="edit-pending-controls">
      <button type="button" title="U" aria-pressed={pendingDays != null} disabled={readOnly}
        onClick={() => onSetValues(editIds, { editPending: "toggle" })}>{pendingDays == null ? "Na úpravu" : `čaká ${pendingDays} ${pendingDays === 1 ? "deň" : pendingDays >= 2 && pendingDays <= 4 ? "dni" : "dní"}`}</button>
      {pendingDays != null && <button type="button" disabled={readOnly}
        onClick={() => onSetValues(editIds, { editPending: false })}>Hotovo</button>}
    </span>)}
    {editorRow("Kategórie",<span className="property-categories">{(assetLinks?.categoryPaths??[]).length?(assetLinks?.categoryPaths??[]).map(category=><span key={category.id} title={category.path}>{category.path}<button aria-label={`Odobrať z kategórie ${category.path}`} onClick={()=>onSetCategory(editIds,category.id,false)}>×</button></span>):<small className="subtle">— priraď štvorčekom v strome nižšie</small>}{aiReady&&aiSuggestions.category.length>0&&<span className="keyword-ai-suggestions category-ai-suggestions" title="Návrhy AI podľa podobných fotiek – kliknutím priradíš, × odmietne">{[...aiSuggestions.category].filter(item=>!(assetLinks?.categoryIds??[]).includes(item.labelId)||selectedIds.length>1).sort((a,b)=>b.score-a.score).slice(0,5).map(item=><span className="ai-suggestion-chip" key={item.labelId} title={`${item.path}\n${selectedIds.length>1?`${item.count??0} z ${suggestionIds.length} fotiek vo vzorke`:`podobných fotiek: ${item.count??0}`}`}><button type="button" disabled={readOnly} onClick={()=>void acceptAiSuggestion("category",item)}>{item.path.split("/").slice(-2).join(" / ")} <small>{Math.round(item.score*100)} %</small></button><button type="button" aria-label="Odmietnuť návrh" onClick={()=>{const ids=suggestionIds;void invoke("ai_reject_suggestion",{assetIds:ids,kind:"category",labelId:item.labelId}).then(()=>removeAiSuggestion("category",item.labelId))}}>×</button></span>)}</span>}</span>)}
    {editorRow("Kľúčové slová",<KeywordProperties selectionReadKey={selectionReadKey} links={assetLinks} primaryId={primaryId} selectedIds={selectedIds} onSetKeyword={onSetKeyword} onCreateAssignKeyword={onCreateAssignKeyword} readOnly={readOnly} onError={onCategoryError}
      contentSuggestions={aiReady && editIds.length === 1 && editIds[0] > 0
        ? <ContentKeywordSuggestions key={`${editIds[0]}:${refreshToken}`} assetId={editIds[0]} readOnly={readOnly}
            onAssign={onSetKeyword} onCreate={onCreateAssignKeyword}/>
        : null}
      suggestions={aiReady?[...aiSuggestions.keyword].filter(item=>!(assetLinks?.keywordIds??[]).includes(item.labelId)||selectedIds.length>1).sort((a,b)=>b.score-a.score).slice(0,5):[]}
      onAcceptSuggestion={item=>void acceptAiSuggestion("keyword",item)}
      onRejectSuggestion={item=>{const ids=suggestionIds;void invoke("ai_reject_suggestion",{assetIds:ids,kind:"keyword",labelId:item.labelId}).then(()=>removeAiSuggestion("keyword",item.labelId))}}/>)}
    {editorRow("Osoby",<div className="assignment-people"><div className="person-assignment-chips">{facePersons.filter(person=>assetPersonCounts[person.id]).map(person=><span key={person.id} className={assetPersonCounts[person.id]===suggestionIds.length?"full":"partial"} title={`${assetPersonCounts[person.id]} z ${suggestionIds.length} fotiek`}><span>{person.name}</span><button disabled={readOnly||!faceStatus.available} aria-label={`Odobrať ${person.name}`} onClick={()=>void removePersonFromSelection(person)}>×</button></span>)}</div><form onSubmit={event=>{event.preventDefault();void assignPersonToSelection()}}><input list="face-person-options" value={personInput} disabled={readOnly||!faceStatus.available||!assignedAssetIds.length} placeholder="Pridať osobu…" onChange={event=>{const value=event.target.value;setPersonInput(value);if(facePersons.some(person=>person.name.toLocaleLowerCase("sk")===value.trim().toLocaleLowerCase("sk")))void assignPersonToSelection(value)}}/><datalist id="face-person-options">{facePersons.map(person=><option key={person.id} value={person.name}/>)}</datalist><button disabled={readOnly||!faceStatus.available||!personInput.trim()}>Pridať</button></form>{!faceStatus.available&&<small className="people-muted">Model tvárí nie je dostupný</small>}</div>)}
    {editorRow("Farby",<><span className="property-color-chips">{assetColors[0]?.colors.map(color=>{const count=assetColors.filter(asset=>asset.colors.find(c=>c.value===color.value)?.effective).length;const manual=assetColors.some(asset=>asset.colors.find(c=>c.value===color.value)?.manualState!==null);const cls=count===0?"empty":count===assetColors.length?"full":"partial";return <button key={color.value} disabled={readOnly||!colorIds.length} className={`property-color-chip ${cls} ${manual?"manual":"auto"}`} title={`${color.name}${count?` · ${count}/${assetColors.length}`:""}`} onClick={e=>onSetColors(colorIds,color.value,e.altKey?null:count===assetColors.length?false:true)}><i style={{background:color.hex}}/></button>})}</span>{assetColors.length>0&&assetColors.some(asset=>!asset.computed)&&<small className="color-not-computed">Farby ešte nie sú vypočítané</small>}{!aiAvailable&&<small className="color-not-computed">AI modul nie je nainštalovaný</small>}</>)}
    {sampled&&<small className="subtle">Návrhy AI a počty osôb vychádzajú z prvých {suggestionIds.length} z {assignedAssetIds.length} vybraných fotiek.</small>}
    {!aiReady&&aiAvailable&&<div className="subtle" title={aiReason}>AI sa načítava…</div>}
    
    {aiReady&&aiSuggestDone&&!aiSuggestions.category.length&&!aiSuggestions.keyword.length&&assetColors.length>0&&<small className="ai-no-suggestion">{assetColors.some(asset=>!asset.computed)?"Návrhy AI: fotka ešte nie je zaindexovaná – AI index stále beží.":"Návrhy AI: pre túto fotku nie je dosť istý návrh."}</small>}
  </section>;
  const stat=(pattern:RegExp)=>metadata?.rows.find(row=>pattern.test(row.name)&&row.value&&row.value!=="-")?.value;
  const stats=([["Čas uzávierky",stat(/^(Čas uzávierky|ExposureTime)/i)],["Clona",stat(/^(Clona|FNumber)/i)],["ISO",stat(/^ISO/i)],["Ohnisková vzdialenosť",stat(/^(Ohnisk|FocalLength$)/i)]] as const).filter(item=>item[1]);
  const refreshAssetFaces=async()=>{await Promise.all([reloadAssignedPeople(),reloadFacePersons()])};
  const updateFace=async(faceId:number,action:'confirm'|'reject'|'assign'|'clear'|'ignore',personId?:number)=>{if(readOnly)return;try{await invoke<number>('ai_faces_set',{faceIds:[faceId],action,personId:personId??null});await refreshAssetFaces();onClusterNotice(action==='confirm'?'Tvár potvrdená':action==='reject'?'Návrh odmietnutý':'Osoba aktualizovaná')}catch(error){onCategoryError(error)}};
  const chooseFacePerson=async(face:FaceInfo,value:string)=>{if(value==='__new__'){const name=window.prompt('Meno osoby');if(!name?.trim())return;try{const id=await invoke<number>('ai_faces_person_create',{name:name.trim()});await updateFace(face.faceId,'assign',id)}catch(error){onCategoryError(error)}return}if(value==='__clear__'){await updateFace(face.faceId,'clear');return}if(value==='__ignore__'){await updateFace(face.faceId,'ignore');return}const id=Number(value);if(Number.isInteger(id)&&id>0)await updateFace(face.faceId,'assign',id);else if(face.personId!=null)await updateFace(face.faceId,'clear')};
  const propertyEvents = {
    tabIndex: 0,
    onClick: (event: MouseEvent<HTMLElement>) => {
      if (!(event.target instanceof HTMLElement) || !event.target.closest("input,textarea,select,button")) event.currentTarget.focus();
    },
    onKeyDown: handlePropertiesKeyDown,
  };
  return {
    sections: {
      assetFaceThumbs, assetFaces, bucketKey, catalogText, chooseFacePerson, colorNames, dragRow, dropAfter,
      dropMark, facePersons, isCollapsed, moveRow, propertyEvents, quality, qualityText, qualityTitle,
      readOnly, rowKey, selectRow, selectedRows, setCopyMenu, setDropMark, updateFace,
    },
    content: <><DockChrome id="propertiesTop"><div className="dock-properties-top" {...propertyEvents}>
      <div className="selected-summary">{metadata?.asset.name ?? (primaryId == null ? "Nie je vybraný súbor" : "Načítavam vlastnosti…")}{selectedIds.length > 1 && <small>{selectedIds.length} vybraných súborov</small>}</div>
      {stats.length>0&&<div className="stat-cards">{stats.map(([name,value])=><div key={name}><small>{name}</small><b title={value}>{value?.split(" (")[0]}</b></div>)}</div>}
      {metadata?.offline && <div className="offline-note">Súbor je offline. Zobrazené sú údaje z katalógu.</div>}
      {loading && <div className="subtle">Načítavam metadáta…</div>}
    </div></DockChrome>
      {groups.map(group => <MetadataGroup key={group.name} group={group}/>)}
      <FacesSection/>
      <DockChrome id="propertiesBottom"><div className="dock-copy-controls" {...propertyEvents}><div className="copy-controls">{selectedRows.length > 0 && <button title="Zrušiť všetky zaškrtnutia" onClick={() => setSelectedRows([])}>✕</button>}<span className="copy-label">Kopírovať{selectedRows.length ? ` (${selectedRows.length})` : ""}:</span><button disabled={!metadata || selectedRows.length === 0} title="Skopíruje zaškrtnuté riadky ako „Názov: hodnota“ (Ctrl+C)" onClick={() => void copy("selected")}>S popisom</button><button disabled={!metadata || selectedRows.length === 0} title="Skopíruje len hodnoty zaškrtnutých riadkov" onClick={() => void copy("values")}>Bez popisu</button><button disabled={!metadata} title="Ďalšie možnosti" onClick={() => setCopyMenu(v => !v)}>▾</button>
        {copyMenu && <div className="copy-menu">{groups.map(group=><button key={group.name} onClick={() => void copy("section", group.name)}>Kopírovať celú sekciu {group.name}</button>)}{Object.keys(rowOrder).length>0&&<button onClick={()=>{setRowOrder({});setCopyMenu(false)}}>Obnoviť pôvodné poradie riadkov</button>}</div>}
      </div>{copyMessage && <div className="copy-message">{copyMessage}</div>}
    </div></DockChrome>
  <IncomingSection>{incomingTools}</IncomingSection><AiPanel activeQuality={activeQuality} thumbEpoch={thumbEpoch} primaryId={primaryId} onNavigate={onNavigate} aiReady={aiReady} aiAvailable={aiAvailable} aiReason={aiReason} catalogRefreshToken={catalogRefreshToken} onSetCategory={onSetCategory} onCategoryChanged={onCategoryChanged} onCategoryError={onCategoryError} onRefresh={onRefresh} onClusterNotice={onClusterNotice} readOnly={readOnly} />
  {!catalogReady ? <DockChrome id="catalogIntro"><small className="color-hint">Načítavam katalóg…</small></DockChrome> : <CatalogPanel selectionReadKey={selectionReadKey} labelCounts={labelCounts} links={assetLinks} valueTokens={valueTokens} activeColors={activeColors} activeQuality={activeQuality} header={catalogEditor} onNavigate={onNavigate} includeChildren={includeChildren} onIncludeChildrenChange={onIncludeChildrenChange} selectedIds={selectedIds} onSetCategory={onSetCategory} onSetKeyword={onSetKeyword} onCategoryChanged={onCategoryChanged} onKeywordChanged={onKeywordChanged} onCategoryError={onCategoryError} refreshToken={refreshToken} colorToken={colorToken} aiAvailable={aiAvailable} aiReady={aiReady} facePersons={facePersons} faceStatus={faceStatus} onRefreshFacePersons={reloadFacePersons} onNotice={onClusterNotice} readOnly={readOnly} />}</>,
  };
}

export type PropertiesSectionData = ReturnType<typeof useRightPanelModel>["sections"];

export default function RightPanel(props: Parameters<typeof useRightPanelModel>[0]) {
  const { sections, content } = useRightPanelModel(props);
  return <RightSectionContext.Provider value={{ properties: sections }}>{content}</RightSectionContext.Provider>;
}

function MetadataGroup({ group }: { group: { name: string; rows: MetadataRow[] } }): ReactElement {
  const {
    bucketKey, catalogText, colorNames, dragRow, dropAfter, dropMark, isCollapsed, moveRow, propertyEvents,
    quality, qualityText, qualityTitle, rowKey, selectRow, selectedRows, setCopyMenu, setDropMark,
  } = useRightSectionData("properties");
  return <DockSection id={propertySection(group.name)}><section className="property-group" {...propertyEvents}>
        <SectionHeading id={propertySection(group.name)} count={group.rows.length} metadata/>
        {!isCollapsed(propertySection(group.name)) && <div className="metadata-rows">
          {[...new Set(group.rows.map(row=>row.subgroup))].map(subgroup=><div className="metadata-subgroup" key={subgroup}>
            {subgroup && subgroup !== group.name && <div className="metadata-subgroup-title">{subgroup}</div>}
            {group.rows.filter(row=>row.subgroup===subgroup).map(row => <div key={rowKey(row)} className={`meta-row checkable ${selectedRows.includes(rowKey(row)) ? "selected" : ""} ${dropMark?.key===rowKey(row)?(dropMark.after?"drop-after":"drop-before"):""}`} onClick={e => selectRow(row,e)} onContextMenu={e => { e.preventDefault(); setCopyMenu(true); }}
              draggable onDragStart={e=>{dragRow.current=row;e.dataTransfer.effectAllowed="move";e.dataTransfer.setData("text/plain",row.name)}} onDragEnd={()=>{dragRow.current=null;setDropMark(null)}}
              onDragOver={e=>{const source=dragRow.current;if(!source||bucketKey(source)!==bucketKey(row))return;e.preventDefault();e.dataTransfer.dropEffect="move";const key=rowKey(row),after=dropAfter(e);setDropMark(mark=>mark&&mark.key===key&&mark.after===after?mark:{key,after})}}
              onDrop={e=>{if(!dragRow.current)return;e.preventDefault();moveRow(row,dropAfter(e))}}>
              <input type="checkbox" className="meta-check" tabIndex={-1} readOnly checked={selectedRows.includes(rowKey(row))} title="Zaškrtnuté riadky sa kopírujú tlačidlami dole. Poradie riadkov zmeníš ťahaním." /><span title={row.name}>{row.name}</span><b title={row.value}>{group.name === "Katalóg" ? catalogText(row) : row.value}</b>
            </div>)}
          </div>)}
        </div>}
        {group.name==="Katalóg"&&<div className="meta-row"><span>Farby</span><b>{colorNames||"—"}</b></div>}
        {group.name==="Katalóg"&&quality&&<div className="meta-row"><span>Kvalita</span><b title={qualityTitle}>{qualityText}</b></div>}
      </section></DockSection>;
}

function FacesSection(): ReactElement {
  const {
    assetFaceThumbs, assetFaces, chooseFacePerson, facePersons, isCollapsed, propertyEvents, readOnly,
    updateFace,
  } = useRightSectionData("properties");
  return <DockSection id="properties.faces" available={assetFaces.length > 0}>{assetFaces.length>0&&<section className="property-group face-properties" {...propertyEvents}><SectionHeading id="properties.faces" count={assetFaces.length} metadata/>{!isCollapsed("properties.faces") && <div className="face-property-list">{assetFaces.map(face=><div className="face-property-row" key={face.faceId}>{face.x==null||face.y==null||face.w==null||face.h==null?<span className="face-photo-level">Cel? fotka</span>:<img src={assetFaceThumbs[face.faceId]} alt=""/>}<div className="face-property-main"><strong>{face.name??"Neznáma tvár"}</strong>{face.state==="auto"&&<small>navrhnuté</small>}{face.state==="suggested"&&<small>návrh ACDSee</small>}</div><select disabled={readOnly} aria-label="Osoba" value={face.personId?.toString()??""} onChange={event=>void chooseFacePerson(face,event.target.value)}><option value="">Neznáma tvár</option>{facePersons.map(person=><option value={person.id} key={person.id}>{person.name}</option>)}<option value="__new__">Nová osoba…</option><option value="__clear__">Zrušiť meno</option><option value="__ignore__">Nie je osoba (ignorovať)</option></select>{(face.state==="auto"||face.state==="suggested")&&<span className="face-review-buttons"><button disabled={readOnly} title="Potvrdiť" onClick={()=>void updateFace(face.faceId,"confirm")}>✓</button><button disabled={readOnly} title="Odmietnuť" onClick={()=>void updateFace(face.faceId,"reject")}>✗</button></span>}</div>)}</div>}</section>}</DockSection>;
}

function IncomingSection({ children }: { children: ReactNode }) {
  const { isCollapsed } = useSectionCollapse();
  return <DockSection id="ai.incoming"><section className="catalog-section">
    <SectionHeading id="ai.incoming"/>
    <div hidden={isCollapsed("ai.incoming")}>{children}</div>
  </section></DockSection>;
}

type ContentKeyword = { name: string; labelId: number | null; path: string };

function ContentKeywordSuggestions({ assetId, readOnly, onAssign, onCreate }: {
  assetId: number;
  readOnly: boolean;
  onAssign: (ids: number[], keywordId: number, assign: boolean) => Promise<boolean>;
  onCreate: (ids: number[], parentId: number | null, name: string) => void;
}) {
  const [items, setItems] = useState<ContentKeyword[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [applying, setApplying] = useState(false);

  const load = async () => {
    setError("");
    setLoading(true);
    try {
      setItems(await invoke<ContentKeyword[]>("ai_content_keywords", { assetId }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  };

  const apply = async (item: ContentKeyword) => {
    setApplying(true);
    try {
      if (item.labelId === null) await onCreate([assetId], null, item.name);
      else await onAssign([assetId], item.labelId, true);
    } finally {
      setApplying(false);
    }
  };

  return <div className="content-keyword-suggestions">
    <button type="button" className="content-keyword-search" disabled={loading || applying} onClick={() => void load()}>
      {loading ? "Analyzujem obsah…" : "Navrhnúť podľa obsahu"}
    </button>
    {error && <small role="status">{error}</small>}
    {!loading && !error && items?.length === 0 && <small role="status">Žiadne ďalšie návrhy podľa obsahu.</small>}
    {!loading && !error && !!items?.length && <div className="keyword-ai-suggestions">
      {items.map(item => <span className="ai-suggestion-chip" key={item.name}>
        <button type="button" disabled={readOnly || applying}
          title={item.labelId === null ? `Vytvoriť a priradiť „${item.name}“` : `Priradiť: ${item.path}`}
          onClick={() => void apply(item)}>
          {item.path} <small className={item.labelId === null ? "new-keyword-label" : undefined}>
            {item.labelId === null ? "Nové" : "V katalógu"}
          </small>
        </button>
      </span>)}
    </div>}
  </div>;
}

function KeywordProperties({selectionReadKey,links,primaryId,selectedIds,onSetKeyword,onCreateAssignKeyword,readOnly,onError,suggestions=[],onAcceptSuggestion,onRejectSuggestion,contentSuggestions}:{selectionReadKey:string;contentSuggestions?:ReactNode;links:CatalogAssetLinks|null;primaryId:number|null;selectedIds:number[];onSetKeyword:(ids:number[],keywordId:number,assign:boolean)=>void;onCreateAssignKeyword:(ids:number[],parentId:number|null,name:string)=>void;readOnly:boolean;onError:(error:unknown)=>void;suggestions?:AiSuggestion[];onAcceptSuggestion?:(item:AiSuggestion)=>void;onRejectSuggestion?:(item:AiSuggestion)=>void}){
  const [text,setText]=useState("");const [options,setOptions]=useState<{id:number;name:string;path:string}[]>([]);const [offerCreate,setOfferCreate]=useState(false);const ids=selectedIds.length?selectedIds:primaryId==null?[]:[primaryId];
  // With several photos selected, a chip shows how many of them carry the keyword; clicking it assigns it to all.
  const [selectionCounts,setSelectionCounts]=useState<Record<number,number>>({});
  const selectionKey = ids.length > 1 ? `${selectionReadKey}:${(links?.keywordIds ?? []).join(",")}` : "";
  useEffect(()=>{if(!selectionKey){setSelectionCounts({});return;}let active=true;void invoke<{totalFiles:number;assigned:Record<number,number>}>("get_keyword_selection_counts",{assetIds:ids}).then(value=>{if(active)setSelectionCounts(value.assigned)}).catch(()=>{if(active)setSelectionCounts({})});return()=>{active=false}},[selectionKey]);
  useEffect(()=>{const query=text.trim();if(!query){setOptions([]);setOfferCreate(false);return;}let active=true;const timer=window.setTimeout(()=>{void invoke<{id:number;name:string;path:string}[]>("search_keyword_options",{query}).then(rows=>{if(active)setOptions(rows)}).catch(error=>{if(active){setOptions([]);onError(error)}})},140);return()=>{active=false;window.clearTimeout(timer)}},[text]);
  const exact=options.find(item=>item.name.toLowerCase()===text.trim().toLowerCase());const add=(id:number)=>{if(!ids.length||readOnly)return;onSetKeyword(ids,id,true);setText("");setOptions([]);setOfferCreate(false)};const create=()=>{if(!ids.length||readOnly||!text.trim())return;onCreateAssignKeyword(ids,null,text.trim());setText("");setOptions([]);setOfferCreate(false)};
  return <div className="property-keyword-editor" onClick={event=>event.stopPropagation()}><div className="property-keyword-chips">{(links?.keywordPaths??[]).map(keyword=>{const have=ids.length>1?(selectionCounts[keyword.id]??0):ids.length;const partial=ids.length>1&&have<ids.length;return <span className={`property-keyword-chip${partial?" partial":""}`} key={keyword.id} title={partial?`${keyword.path}\nMá ho ${have} z ${ids.length} vybraných fotiek. Kliknutím ho priradíš všetkým.`:keyword.path}>{partial?<button type="button" className="property-keyword-assign" disabled={readOnly} onClick={()=>onSetKeyword(ids,keyword.id,true)}>{keyword.path} <small>{have}/{ids.length}</small></button>:keyword.path}<button disabled={readOnly||!ids.length} aria-label={`Odobrať kľúčové slovo ${keyword.path}`} onClick={()=>onSetKeyword(ids,keyword.id,false)}>×</button></span>})}</div><div className="keyword-autocomplete"><input disabled={readOnly} value={text} placeholder="Pridať kľúčové slovo…" onChange={event=>{setText(event.target.value);setOfferCreate(false)}} onKeyDown={event=>{if(event.key==="Enter"){event.preventDefault();if(exact){add(exact.id);return;}const query=text.trim();if(!query)return;void invoke<{id:number;name:string;path:string}[]>("search_keyword_options",{query}).then(rows=>{setOptions(rows);const match=rows.find(item=>item.name.toLowerCase()===query.toLowerCase());if(match)add(match.id);else setOfferCreate(true)}).catch(onError)}else if(event.key==="Escape"){setText("");setOptions([]);setOfferCreate(false)}}}/>{text.trim()&&<div className="keyword-autocomplete-options">{options.map(item=><button type="button" key={item.id} onMouseDown={event=>event.preventDefault()} onClick={()=>add(item.id)}><strong>{item.name}</strong>{item.path!==item.name&&<small>{item.path}</small>}</button>)}{offerCreate&&!exact&&<button type="button" disabled={!ids.length} onMouseDown={event=>event.preventDefault()} onClick={create}>Vytvoriť kľúčové slovo „{text.trim()}“ a pridať</button>}</div>}</div>{suggestions.length>0&&<div className="keyword-ai-suggestions" title="Návrhy AI podľa podobných fotiek – kliknutím priradíš, × odmietne">{suggestions.map(item=><span className="ai-suggestion-chip" key={item.labelId} title={item.path}><button type="button" disabled={readOnly||!ids.length} onClick={()=>onAcceptSuggestion?.(item)}>{item.path.split("/").slice(-1)[0]} <small>V katalógu</small></button><button type="button" aria-label="Odmietnuť návrh" onClick={()=>onRejectSuggestion?.(item)}>×</button></span>)}</div>}{contentSuggestions}</div>
}

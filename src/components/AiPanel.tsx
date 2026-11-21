import { useEffect, useRef, useState, type ReactElement } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Asset, ValueCount, View } from "../types";
import { thumbUrl } from "./AssetGrid";
import { DockSection, SectionHeading, useSectionCollapse } from "./Dock";
import { RightSectionContext, useRightSectionData } from "./rightSectionContext";
import type { CategoryMutation, UnsortedCluster } from "./rightPanelData";

function useAiPanelModel({activeQuality="",thumbEpoch,primaryId,onNavigate,aiReady,aiAvailable,aiReason,catalogRefreshToken=0,onSetCategory,onCategoryChanged,onCategoryError,onRefresh,onClusterNotice,readOnly}:{activeQuality?:string;thumbEpoch:number;primaryId:number|null;onNavigate:(view:View)=>void;aiReady:boolean;aiAvailable:boolean;aiReason:string;catalogRefreshToken?:number;onSetCategory:(ids:number[],categoryId:number,assign:boolean)=>void|Promise<boolean>;onCategoryChanged:(result:CategoryMutation,deleted:boolean)=>void;onCategoryError:(error:unknown)=>void;onRefresh:()=>void;onClusterNotice:(message:string)=>void;readOnly:boolean}){
  const [suggestionKind,setSuggestionKind]=useState<"category"|"keyword">("category");
  const [suggestionGroups,setSuggestionGroups]=useState<{labelId:number;path:string;count:number;assetIds:number[]}[]>([]);
  const [suggestionLoading,setSuggestionLoading]=useState(false);
  const [duplicateLevel,setDuplicateLevel]=useState<"same"|"similar">("same");
  const [duplicateLoading,setDuplicateLoading]=useState(false);
  const [duplicateResult,setDuplicateResult]=useState<{groups:{assetIds:number[]}[]}|null>(null);
  const [photoDupLoading,setPhotoDupLoading]=useState<"same"|"similar"|null>(null);
  const [photoDupMessage,setPhotoDupMessage]=useState("");
  const [clusters,setClusters]=useState<{groups:UnsortedCluster[];unlabeled:number;clustered:number}|null>(null);
  const [clusterLoading,setClusterLoading]=useState(false);
  const [clusterSamples,setClusterSamples]=useState<Record<string,Asset[]>>({});
  const [clusterCategories,setClusterCategories]=useState<{id:number;path:string}[]>([]);
  const [clusterSelections,setClusterSelections]=useState<Record<string,string>>({});
  const [clusterCreate,setClusterCreate]=useState<{key:string;name:string}|null>(null);
  const suggestionsLoaded=useRef(false);
  const { isCollapsed } = useSectionCollapse();
  useEffect(()=>setPhotoDupMessage(""),[primaryId]);
  useEffect(()=>{if(!suggestionsLoaded.current)return;let active=true;void invoke<{labelId:number;path:string;count:number;assetIds:number[]}[]>("ai_suggestion_groups",{kind:suggestionKind}).then(g=>{if(active)setSuggestionGroups(g)}).catch(()=>{});return()=>{active=false}},[catalogRefreshToken]);
  const loadClusterCategories=async()=>{try{setClusterCategories(await invoke<{id:number;path:string}[]>("ai_category_paths"));}catch{setClusterCategories([])}};
  useEffect(()=>{if(aiReady)void loadClusterCategories();},[aiReady,catalogRefreshToken]);
  useEffect(()=>{if(!clusters?.groups.length){setClusterSamples({});return;}let active=true;void Promise.all(clusters.groups.map(async(group,index)=>{const page=await invoke<{items:Asset[]}>("get_assets",{filter:{kind:"ai",id:null,rootId:null,includeChildren:false,folderPath:null,ids:group.ids.slice(0,4),value:null,query:null,sort:"name",descending:false,offset:0,limit:4,includeCategories:false,includeKeywords:false}});return [`${index}:${group.ids[0]}`,page.items] as const;})).then(rows=>{if(active)setClusterSamples(Object.fromEntries(rows))}).catch(()=>{if(active)setClusterSamples({})});return()=>{active=false}},[clusters]);
  const runClusters=async()=>{if(!aiReady||clusterLoading)return;setClusterLoading(true);try{setClusters(await invoke<{groups:UnsortedCluster[];unlabeled:number;clustered:number}>("ai_unsorted_clusters",{minSize:null}));}catch(error){onClusterNotice(String(error));}finally{setClusterLoading(false)}};
  const removeCluster=(group:UnsortedCluster,assigned=0)=>setClusters(previous=>previous?{...previous,groups:previous.groups.filter(item=>item!==group),unlabeled:Math.max(0,previous.unlabeled-assigned),clustered:Math.max(0,previous.clustered-group.total)}:previous);
  const assignCluster=async(group:UnsortedCluster,categoryId:number)=>{if(readOnly)return;try{const result=await onSetCategory(group.ids.slice(0,500),categoryId,true);if(result!==false)removeCluster(group,Math.min(group.total,500));}catch(error){onCategoryError(error);}};
  const ignoreCluster=async(group:UnsortedCluster)=>{try{await invoke<number>("ai_cluster_ignore",{assetIds:group.ids.slice(0,500)});removeCluster(group);onClusterNotice(`Skupina ignorovaná: ${Math.min(group.total,500)} fotiek`);onRefresh();}catch(error){onCategoryError(error);}};
  const createClusterCategory=async(group:UnsortedCluster)=>{if(readOnly||!clusterCreate?.name.trim())return;try{const result=await invoke<CategoryMutation>("create_category",{parentId:null,name:clusterCreate.name.trim()});if(result.categoryId==null)throw new Error("Kategóriu sa nepodarilo vytvoriť");onCategoryChanged(result,false);void loadClusterCategories();const assigned=await onSetCategory(group.ids.slice(0,500),result.categoryId,true);if(assigned!==false){removeCluster(group,Math.min(group.total,500));setClusterCreate(null);}}catch(error){onCategoryError(error);}};
  const [qualityCounts,setQualityCounts]=useState<ValueCount[]>([]);
  useEffect(()=>{let active=true;void invoke<ValueCount[]>("get_quality_counts",{}).then(v=>{if(active)setQualityCounts(v)}).catch(()=>{});return()=>{active=false}},[catalogRefreshToken,aiReady]);
  return {
    sections: {
      activeQuality, aiAvailable, aiReady, aiReason, assignCluster, clusterCategories, clusterCreate,
      clusterLoading, clusterSamples, clusterSelections, clusters, createClusterCategory, duplicateLevel,
      duplicateLoading, duplicateResult, ignoreCluster, isCollapsed, onCategoryError, onNavigate,
      photoDupLoading, photoDupMessage, primaryId, qualityCounts, readOnly, runClusters, setClusterCreate,
      setClusterSelections, setDuplicateLevel, setDuplicateLoading, setDuplicateResult, setPhotoDupLoading,
      setPhotoDupMessage, setSuggestionGroups, setSuggestionKind, setSuggestionLoading, suggestionGroups,
      suggestionKind, suggestionLoading, suggestionsLoaded, thumbEpoch,
    },
    content: <>
    <DuplicatesSection/>
    <SuggestionsSection/>
    <UnsortedSection/>
  <QualitySection/></>,
  };
}

export type AiSectionData = ReturnType<typeof useAiPanelModel>["sections"];

export function AiPanel(props: Parameters<typeof useAiPanelModel>[0]) {
  const { sections, content } = useAiPanelModel(props);
  return <RightSectionContext.Provider value={{ ai: sections }}>{content}</RightSectionContext.Provider>;
}

function DuplicatesSection(): ReactElement {
  const {
    aiAvailable, aiReady, aiReason, duplicateLevel, duplicateLoading, duplicateResult, isCollapsed, onNavigate,
    photoDupLoading, photoDupMessage, primaryId, setDuplicateLevel, setDuplicateLoading, setDuplicateResult,
    setPhotoDupLoading, setPhotoDupMessage,
  } = useRightSectionData("ai");
  return <DockSection id="ai.aiDuplicates"><section className="catalog-section ai-review-section"><SectionHeading id="ai.aiDuplicates" title="DUPLICITY (AI)"/>{!isCollapsed("ai.aiDuplicates")&&<>{!aiReady?<small className="color-not-computed" title={aiReason}>{aiAvailable?"AI sa načítava…":"AI modul nie je nainštalovaný"}</small>:<><div className="dup-block"><small className="dup-caption">K vybranej fotke</small><div className="ai-review-controls">{(["same","similar"] as const).map(level=><button key={level} disabled={primaryId==null||primaryId<=0||photoDupLoading!==null} title={primaryId==null?"Najprv vyber fotku":primaryId<=0?"Fotka nie je v katalógu":""} onClick={()=>{const id=primaryId!;setPhotoDupLoading(level);setPhotoDupMessage("");void invoke<number[]>("ai_duplicates_of",{assetId:id,level}).then(ids=>{if(ids.length)onNavigate({kind:"ai",ids,title:level==="same"?"Rovnaké ako vybraná fotka":"Veľmi podobné vybranej fotke"});else setPhotoDupMessage(level==="same"?"Žiadna kópia ani RAW/JPG dvojica tejto fotky":"Žiadne veľmi podobné fotky")}).catch(e=>setPhotoDupMessage(String(e))).finally(()=>setPhotoDupLoading(null))}}>{photoDupLoading===level?"Hľadám…":level==="same"?"Rovnaké":"Veľmi podobné"}</button>)}</div>{photoDupMessage&&<small className="color-not-computed">{photoDupMessage}</small>}</div><div className="dup-block"><small className="dup-caption">V celom katalógu</small><div className="ai-review-controls"><button className={duplicateLevel==="same"?"active":""} onClick={()=>setDuplicateLevel("same")}>Rovnaké</button><button className={duplicateLevel==="similar"?"active":""} onClick={()=>setDuplicateLevel("similar")}>Veľmi podobné</button><button disabled={duplicateLoading} onClick={()=>{setDuplicateLoading(true);void invoke<{groups:{assetIds:number[]}[]}|null>("ai_duplicate_groups",{level:duplicateLevel}).then(setDuplicateResult).catch(()=>setDuplicateResult({groups:[]})).finally(()=>setDuplicateLoading(false))}}>{duplicateLoading?"Hľadám…":"Hľadať"}</button></div>{duplicateLoading?<small>Hľadám duplicity…</small>:duplicateResult?duplicateResult.groups.length?<><small>{duplicateResult.groups.length} skupín · {duplicateResult.groups.reduce((n,g)=>n+g.assetIds.length,0)} fotiek</small><button className="value-row" onClick={()=>{const ids:number[]=[],groups:number[]=[];duplicateResult.groups.forEach((g,i)=>g.assetIds.forEach(id=>{ids.push(id);groups.push(i+1)}));onNavigate({kind:"ai",ids,groups,title:`Duplicity (AI): ${duplicateResult.groups.length} skupín`})}}>Zobraziť</button></>:<small>AI nenašlo duplicity</small>:null}</div></>}</>}</section></DockSection>;
}

function SuggestionsSection(): ReactElement {
  const {
    aiAvailable, aiReady, aiReason, isCollapsed, onNavigate, setSuggestionGroups, setSuggestionKind,
    setSuggestionLoading, suggestionGroups, suggestionKind, suggestionLoading, suggestionsLoaded,
  } = useRightSectionData("ai");
  return <DockSection id="ai.aiSuggestions"><section className="catalog-section ai-review-section"><SectionHeading id="ai.aiSuggestions" title="NÁVRHY AI"/>{!isCollapsed("ai.aiSuggestions")&&<>{!aiReady?<small className="color-not-computed" title={aiReason}>{aiAvailable?"AI sa načítava…":"AI modul nie je nainštalovaný"}</small>:<><div className="ai-review-controls"><button className={suggestionKind==="category"?"active":""} onClick={()=>{setSuggestionKind("category");setSuggestionGroups([]);suggestionsLoaded.current=false}}>Kategórie</button><button className={suggestionKind==="keyword"?"active":""} onClick={()=>{setSuggestionKind("keyword");setSuggestionGroups([]);suggestionsLoaded.current=false}}>Kľúčové slová</button><button disabled={suggestionLoading} onClick={()=>{setSuggestionLoading(true);void invoke<{labelId:number;path:string;count:number;assetIds:number[]}[]>("ai_suggestion_groups",{kind:suggestionKind}).then(g=>{suggestionsLoaded.current=true;setSuggestionGroups(g)}).catch(()=>setSuggestionGroups([])).finally(()=>setSuggestionLoading(false))}}>{suggestionLoading?"Načítavam…":"Načítať návrhy"}</button></div>{suggestionGroups.map(group=><button className="value-row" key={group.labelId} title={group.path} onClick={()=>onNavigate({kind:"ai",ids:group.assetIds,title:`Návrh AI: ${group.path}`,suggestion:{kind:suggestionKind,labelId:group.labelId,path:group.path}})}><span>{group.path}</span><small>· {group.count}</small></button>)}</>}</>}</section></DockSection>;
}

function UnsortedSection(): ReactElement {
  const {
    aiReady, aiReason, assignCluster, clusterCategories, clusterCreate, clusterLoading, clusterSamples,
    clusterSelections, clusters, createClusterCategory, ignoreCluster, isCollapsed, onCategoryError,
    onNavigate, readOnly, runClusters, setClusterCreate, setClusterSelections, thumbEpoch,
  } = useRightSectionData("ai");
  return <DockSection id="ai.aiClusters"><section className="catalog-section ai-review-section"><SectionHeading id="ai.aiClusters" title="NEZARADENÉ SKUPINY"/>{!isCollapsed("ai.aiClusters")&&<><div className="cluster-controls"><button disabled={!aiReady||clusterLoading} title={!aiReady?aiReason:""} onClick={()=>void runClusters()}>{clusterLoading?"Hľadám…":"Nájsť skupiny"}</button>{clusters&&<button className="cluster-link" disabled={clusterLoading||readOnly} onClick={()=>{void invoke("ai_cluster_ignore_reset",{}).then(()=>runClusters()).catch(onCategoryError)}}>Obnoviť ignorované</button>}</div>{clusters&&<small className="cluster-summary">{clusters.clustered} z {clusters.unlabeled} nezaradených fotiek v {clusters.groups.length} skupinách</small>}{clusters&&!clusters.groups.length&&<small className="color-not-computed">Nenašiel som žiadne skupiny.</small>}{clusters?.groups.map((group,index)=>{const key=`${index}:${group.ids[0]}`;return <article className="cluster-card" key={key}><div className="cluster-thumbnails">{(clusterSamples[key]??[]).slice(0,4).map(asset=>asset.path&&<img key={asset.id} src={thumbUrl(asset,160,thumbEpoch)} alt="" loading="lazy" draggable={false}/>)}</div><strong>{group.total} fotiek</strong>{group.labelPath&&<small>Podobné na: {group.labelPath}</small>}{group.name&&<small>Návrh názvu: {group.name}</small>}{group.total>500&&<small className="cluster-muted">priradí sa prvých 500, zvyšok sa objaví pri ďalšom hľadaní</small>}<div className="cluster-actions"><button onClick={()=>onNavigate({kind:"ai",ids:group.ids,title:`Skupina: ${group.name??"bez názvu"}`})}>Zobraziť</button>{group.labelId!=null&&group.labelPath&&<button disabled={readOnly} title={`Priradiť do ${group.labelPath}`} onClick={()=>void assignCluster(group,group.labelId!)}>Priradiť</button>}<select aria-label="Do kategórie…" value={clusterSelections[key]??""} disabled={readOnly||clusterCategories.length===0} onChange={event=>{const value=event.target.value;setClusterSelections(previous=>({...previous,[key]:value}));if(value)void assignCluster(group,Number(value));}}><option value="">Do kategórie…</option>{clusterCategories.map(category=><option key={category.id} value={category.id}>{category.path}</option>)}</select><button disabled={readOnly} onClick={()=>setClusterCreate({key,name:group.name??""})}>Nová kategória…</button><button disabled={readOnly} onClick={()=>void ignoreCluster(group)}>Ignorovať</button></div>{clusterCreate?.key===key&&<form className="cluster-create" onSubmit={event=>{event.preventDefault();void createClusterCategory(group)}}><input autoFocus maxLength={100} value={clusterCreate.name} onChange={event=>setClusterCreate({...clusterCreate,name:event.target.value})}/><button disabled={readOnly||!clusterCreate.name.trim()}>OK</button><button type="button" onClick={()=>setClusterCreate(null)}>Zrušiť</button></form>}</article>})}</>}</section></DockSection>;
}

function QualitySection(): ReactElement {
  const {
    activeQuality, isCollapsed, onNavigate, qualityCounts,
  } = useRightSectionData("ai");
  return <DockSection id="ai.quality"><section className="catalog-section"><SectionHeading id="ai.quality" title="TECHNICKÁ KVALITA"/>{!isCollapsed("ai.quality")&&(qualityCounts.every(row=>row.count===0)&&<small className="quality-hint">Kvalita sa dopočíta počas AI indexovania</small>)}{!isCollapsed("ai.quality")&&(["blur","over","under"] as const).map((value,index)=>{const label=["Neostré","Prepálené","Tmavé"][index];const count=qualityCounts.find(row=>row.value===value)?.count??0;return <button key={value} className={`value-row quality-value-row${activeQuality===value?" active":""}`} title={`Kvalita: ${label}`} onClick={()=>onNavigate({kind:"quality",value,title:`Kvalita: ${label}`})}><span>{label}</span><small>{count.toLocaleString("sk-SK")}</small></button>})}</section></DockSection>;
}

import { useEffect, useMemo, useRef, useState,
  type ReactElement, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import type { CatalogAssetLinks, Node as CatalogNode, Root, ValueCount, View } from "../types";
import { labelColor, labelText } from "./AssetGrid";
import { readStored, writeStored } from "../storage";
import { expandedNodes, toggleSetItem, usePersistentSet } from "../uiState";
import { springOpenHandlers, useSpringOpen } from "../hooks/useSpringOpen";
import { CategoryIcon, CategoryIconPicker } from "../categoryIcons";
import { DockChrome, DockSection, SectionHandle, SectionHeading, useSectionCollapse, useSectionMenu } from "./Dock";
import { RightSectionContext, useRightSectionData } from "./rightSectionContext";
import { fetchFaceThumbs, type CategoryMutation, type FacePerson, type FaceReview, type FaceStatus,
  type FaceUnknownCluster, type KeywordMutation } from "./rightPanelData";
import "./CategoryManagement.css";

type CategoryIconTarget = { id: number; isRoot: boolean; iconKey: string | null };

function useCatalogPanelModel({ activeColors = "", header, onNavigate, includeChildren, onIncludeChildrenChange, primaryId, selectedIds, onSetCategory, onSetKeyword, onCategoryChanged, onKeywordChanged, onCategoryError, refreshToken, colorToken = 0,aiAvailable,aiReady,facePersons,faceStatus,onRefreshFacePersons,onNotice,readOnly }: { activeColors?: string; activeQuality?: string; header?: ReactNode; onNavigate: (view: View) => void; includeChildren: boolean; onIncludeChildrenChange: (value: boolean) => void; primaryId: number | null; selectedIds:number[]; onSetCategory:(ids:number[],categoryId:number,assign:boolean)=>void; onSetKeyword:(ids:number[],keywordId:number,assign:boolean)=>void; onCategoryChanged:(result:CategoryMutation,deleted:boolean)=>void; onKeywordChanged:(result:KeywordMutation,deleted:boolean)=>void; onCategoryError:(error:unknown)=>void;refreshToken: number; colorToken?: number;aiAvailable:boolean;aiReady:boolean;facePersons:FacePerson[];faceStatus:FaceStatus;onRefreshFacePersons:()=>Promise<void>;onNotice:(message:string)=>void;readOnly:boolean }) {
  const [roots,setRoots]=useState<Root[]>([]);
  const [openCategories, setOpenCategories] = usePersistentSet<string>("ddreams.ui.catalog.categories", "");
  const { openRoots, openCats } = useMemo(() => {
    const openRoots = new Set<number>();
    const openCats = new Set<number>();
    for (const key of openCategories) {
      const [kind, id] = key.split(":");
      if (kind === "root") openRoots.add(Number(id));
      if (kind === "category") openCats.add(Number(id));
    }
    return { openRoots, openCats };
  }, [openCategories]);
  const rootSpringTimer=useRef<number|null>(null);
  const [cats,setCats]=useState(new Map<number,CatalogNode[]>());
  const [catChildren,setCatChildren]=useState(new Map<number,CatalogNode[]>());
  const [keywords,setKeywords]=useState<CatalogNode[]>([]);
  const [keywordResults,setKeywordResults]=useState<CatalogNode[]>([]);
  const [categoryResults,setCategoryResults]=useState<CatalogNode[]>([]);
  const [openKeywords, setOpenKeywords] = usePersistentSet<number>("ddreams.ui.catalog.keywords", 0);
  const [keywordChildren,setKeywordChildren]=useState(new Map<number,CatalogNode[]>());
  const [keywordMenu,setKeywordMenu]=useState<{x:number;y:number;id:number;name:string;parentId:number|null}|null>(null);
  const [activeKeyword,setActiveKeyword]=useState<{id:number;name:string;parentId:number|null}|null>(null);
  const [keywordDialog,setKeywordDialog]=useState<{kind:"create";parentId:number|null;name:string}|{kind:"rename";id:number;name:string}|null>(null);
  const [keywordDeleteDialog,setKeywordDeleteDialog]=useState<{info:{id:number;name:string;assignedFiles:number;subkeywords:number};blocked:boolean}|null>(null);
  const [peopleQuery,setPeopleQuery]=useState("");
  const [personMenu,setPersonMenu]=useState<number|null>(null);
  const [reviewPerson,setReviewPerson]=useState<FacePerson|null>(null);
  const [reviewRows,setReviewRows]=useState<FaceReview[]>([]);
  const [reviewThumbs,setReviewThumbs]=useState<Record<number,string>>({});
  const [reviewSelected,setReviewSelected]=useState<Set<number>>(new Set());
  const [newPersonName,setNewPersonName]=useState<string|null>(null);
  const [newPersonValue,setNewPersonValue]=useState("");
  const [coverThumbs,setCoverThumbs]=useState<Record<number,string>>({});
  const [recognizingFaces,setRecognizingFaces]=useState(false);
  const [unknownFaceClusters,setUnknownFaceClusters]=useState<{available:boolean;groups:FaceUnknownCluster[];unknown:number;clustered:number}|null>(null);
  const [unknownFaceLoading,setUnknownFaceLoading]=useState(false);
  const [unknownFaceThumbs,setUnknownFaceThumbs]=useState<Record<number,string>>({});
  const [unknownFaceCreate,setUnknownFaceCreate]=useState<{group:FaceUnknownCluster;name:string}|null>(null);
  const [highlightKeyword,setHighlightKeyword]=useState<number|null>(null);
  const [keywordDropTarget,setKeywordDropTarget]=useState<number|null>(null);
  const [keywordTopDrop,setKeywordTopDrop]=useState(false);
  const [keywordQuery,setKeywordQuery]=useState("");
  const [ratings,setRatings]=useState<ValueCount[]>([]);
  const [labels,setLabels]=useState<ValueCount[]>([]);
  const [colorCounts,setColorCounts]=useState<{value:string;name:string;hex:string;count:number}[]>([]);

  const [special,setSpecial]=useState<ValueCount[]>([]);
  const [links,setLinks]=useState<CatalogAssetLinks|null>(null);
  const [autoGroups,setAutoGroups]=useState<{key:string;name:string;values:{value:string;label:string;count:number}[]}[]>([]);
  const [autoProgress,setAutoProgress]=useState<{done:number;total:number;running:boolean;error:string|null}|null>(null);
  useEffect(()=>{let cancelled=false;const load=()=>void invoke<[{key:string;name:string;values:{value:string;label:string;count:number}[]}[],{done:number;total:number;running:boolean;error:string|null}]>("auto_categories").then(([groups,progress])=>{if(!cancelled){setAutoGroups(groups);setAutoProgress(progress)}}).catch(error=>{if(!cancelled)setAutoProgress({done:0,total:0,running:false,error:String(error)})});load();const timer=window.setInterval(load,5000);return()=>{cancelled=true;window.clearInterval(timer)}},[]);
  const { isCollapsed, setCollapsed } = useSectionCollapse();
  const categoriesCollapsed = isCollapsed("catalog.categories");
  const keywordsCollapsed = isCollapsed("catalog.keywords");
  const [showAssignSquares,setShowAssignSquares]=useState(()=>readStored("ddreams.showAssignSquares",true));
  const [selectionCounts,setSelectionCounts]=useState<{totalFiles:number;assigned:Record<number,number>}>({totalFiles:0,assigned:{}});
  const [keywordSelectionCounts,setKeywordSelectionCounts]=useState<{totalFiles:number;assigned:Record<number,number>}>({totalFiles:0,assigned:{}});
  const [categoryMenu,setCategoryMenu]=useState<{x:number;y:number;id:number;name:string;parentId:number|null;icon:CategoryIconTarget;rootOnly:boolean}|null>(null);
  const [activeCategory,setActiveCategory]=useState<{id:number;name:string;parentId:number|null;rootId:number|null}|null>(null);
  const [categoryDialog,setCategoryDialog]=useState<
    {kind:"create";parentId:number|null;name:string;iconKey:string|null}
    | {kind:"rename";id:number;name:string}
    | ({kind:"icon";name:string} & CategoryIconTarget)
    | null
  >(null);
  const [deleteDialog,setDeleteDialog]=useState<{info:{id:number;name:string;assignedFiles:number;subcategories:number};blocked:boolean}|null>(null);
  const [highlightCategory,setHighlightCategory]=useState<number|null>(null);
  const [dropTarget,setDropTarget]=useState<number|null>(null);
  const [topDropActive,setTopDropActive]=useState(false);
  const linksRequest=useRef(0);
  const labelTimer=useRef<number|undefined>(undefined);

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

  // Colour counts change while the AI indexes; they refresh on their own slow tick instead of reloading the whole panel.
  useEffect(()=>{let active=true;void invoke<{value:string;name:string;hex:string;count:number}[]>("get_color_counts",{}).then(v=>{if(active)setColorCounts(v)}).catch(()=>{});return()=>{active=false}},[refreshToken,colorToken]);

  useEffect(()=>{let active=true;if(facePersons.length===0){setCoverThumbs({});return;}void fetchFaceThumbs(facePersons.flatMap(person=>person.cover==null?[]:[person.cover])).then(v=>{if(active)setCoverThumbs(v)}).catch(()=>{if(active)setCoverThumbs({})});return()=>{active=false}},[facePersons]);
  useEffect(()=>{
    let active=true;
    const load=<T,>(promise:Promise<T>, apply:(value:T)=>void)=>{void promise.then(value=>{if(active)apply(value)});};
    load(invoke<Root[]>("get_category_roots",{includeChildren,offset:0,limit:500}),setRoots);
    load(invoke<ValueCount[]>("get_catalog_values",{kind:"rating",offset:0,limit:20}),values=>{const counts=new Map(values.map(v=>[v.value,v.count]));setRatings(["unrated","1","2","3","4","5"].map(value=>({value,count:counts.get(value)??0})));});
    load(invoke<ValueCount[]>("get_catalog_values",{kind:"label",offset:0,limit:100}),setLabels);
    load(invoke<ValueCount[]>("get_catalog_values",{kind:"special",offset:0,limit:20}),setSpecial);
    load(invoke<CatalogNode[]>("get_catalog_nodes",{kind:"keyword",parentId:null,rootId:null,includeChildren,offset:0,limit:500}),setKeywords);
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

  const loadPersonReview=async(person:FacePerson)=>{setReviewPerson(person);setReviewSelected(new Set());try{const result=await invoke<{faces:FaceReview[]}>('ai_faces_review',{personId:person.id,limit:200});setReviewRows(result.faces??[]);setReviewThumbs(await fetchFaceThumbs((result.faces??[]).map(face=>face.faceId)))}catch(error){setReviewRows([]);setReviewThumbs({});onCategoryError(error)}};
  const applyReview=async(action:'confirm'|'reject',ids:number[])=>{if(readOnly||!ids.length)return;try{const count=await invoke<number>('ai_faces_set',{faceIds:ids,action,personId:null});if(reviewPerson)await loadPersonReview(reviewPerson);await onRefreshFacePersons();onNotice(action==='confirm'?`Potvrdené: ${count} tvárí`:`Odmietnuté: ${count} tvárí`)}catch(error){onCategoryError(error)}};
  const openPerson=async(person:FacePerson,includeAuto=true)=>{try{const ids=await invoke<number[]>('ai_faces_person_assets',{personId:person.id,includeAuto});onNavigate({kind:'people',ids,title:`Osoba: ${person.name}`})}catch(error){onCategoryError(error)}};
  const createPerson=async()=>{const name=newPersonValue.trim();if(readOnly||!name)return;try{await invoke<number>('ai_faces_person_create',{name});setNewPersonName(null);setNewPersonValue('');await onRefreshFacePersons();onNotice(`Osoba vytvorená: ${name}`)}catch(error){onCategoryError(error)}};
  const runFaceRecognition=async()=>{if(recognizingFaces||readOnly)return;setRecognizingFaces(true);try{const changed=await invoke<number>('ai_faces_recognize_all',{});await onRefreshFacePersons();onNotice(`Rozpoznanie aktualizovalo ${changed} tvárí`)}catch(error){onCategoryError(error)}finally{setRecognizingFaces(false)}};
  const runUnknownFaceClusters=async()=>{if(unknownFaceLoading||!faceStatus.available)return;setUnknownFaceLoading(true);try{const result=await invoke<{available:boolean;groups:FaceUnknownCluster[];unknown:number;clustered:number}>('ai_faces_clusters',{minSize:null});setUnknownFaceClusters(result);const thumbs=await fetchFaceThumbs(result.groups.flatMap(group=>group.faceIds.slice(0,8)));setUnknownFaceThumbs(thumbs)}catch(error){onCategoryError(error)}finally{setUnknownFaceLoading(false)}};
  const removeUnknownFaceCluster=(group:FaceUnknownCluster)=>setUnknownFaceClusters(previous=>previous?{...previous,groups:previous.groups.filter(item=>item!==group),clustered:Math.max(0,previous.clustered-group.total)}:previous);
  const assignUnknownFaceCluster=async(group:FaceUnknownCluster,personId:number,name:string)=>{if(readOnly)return;try{const changed=await invoke<number>('ai_faces_set',{faceIds:group.faceIds,action:'assign',personId});removeUnknownFaceCluster(group);setUnknownFaceCreate(null);onNotice(`${changed} tvárí priradených: ${name}`);void invoke<number>('ai_faces_recognize_all',{}).then(()=>onRefreshFacePersons()).catch(onCategoryError)}catch(error){onCategoryError(error)}};
  const ignoreUnknownFaceCluster=async(group:FaceUnknownCluster)=>{if(readOnly)return;try{await invoke<number>('ai_faces_set',{faceIds:group.faceIds,action:'ignore',personId:null});removeUnknownFaceCluster(group);onNotice('Skupina ignorovaná')}catch(error){onCategoryError(error)}};
  const createPersonForUnknownCluster=async()=>{if(readOnly||!unknownFaceCreate?.name.trim())return;try{const name=unknownFaceCreate.name.trim();const personId=await invoke<number>('ai_faces_person_create',{name});await assignUnknownFaceCluster(unknownFaceCreate.group,personId,name);await onRefreshFacePersons()}catch(error){onCategoryError(error)}};
  const branchRequests = useRef(new Map<string, "loading" | "loaded" | "failed">());
  useEffect(() => {
    const requests = new Map<string, "loading" | "loaded" | "failed">();
    branchRequests.current = requests;
    return () => { requests.clear(); };
  }, [includeChildren, refreshToken]);

  useEffect(() => {
    const requests = branchRequests.current;
    const load = (key: string, kind: "category" | "keyword", parentId: number | null, rootId: number | null,
      id: number, setChildren: typeof setCats) => {
      if (requests.has(key)) return;
      requests.set(key, "loading");
      void invoke<CatalogNode[]>("get_catalog_nodes", {kind, parentId, rootId, includeChildren, offset: 0, limit: 500})
        .then(nodes => {
          if (branchRequests.current !== requests || !requests.has(key)) return;
          requests.set(key, "loaded");
          setChildren(previous => new Map(previous).set(id, nodes));
        }).catch(error => {
          if (branchRequests.current !== requests || !requests.has(key)) return;
          requests.set(key, "failed");
          onCategoryError(error);
        });
    };
    if (!categoriesCollapsed) {
      for (const root of roots) {
        if (!root.hasChildren || !openRoots.has(root.id)) continue;
        load("root:" + root.id, "category", null, root.id, root.id, setCats);
        for (const node of expandedNodes(cats.get(root.id) ?? [], node => node.hasChildren && openCats.has(node.id),
          node => catChildren.get(node.id) ?? [])) {
          load("category:" + node.id, "category", node.id, node.rootId, node.id, setCatChildren);
        }
      }
    }
    if (!keywordsCollapsed) {
      for (const node of expandedNodes(keywords, node => node.hasChildren && openKeywords.has(node.id),
        node => keywordChildren.get(node.id) ?? [])) {
        load("keyword:" + node.id, "keyword", node.id, null, node.id, setKeywordChildren);
      }
    }
  }, [roots, cats, catChildren, keywords, keywordChildren, openRoots, openCats, openKeywords,
    categoriesCollapsed, keywordsCollapsed, includeChildren, refreshToken, onCategoryError]);

  const retryBranch = (key: string) => {
    if (branchRequests.current.get(key) === "failed") branchRequests.current.delete(key);
  };
  const toggleRoot = (root: Root) => {
    if (!root.hasChildren) return;
    retryBranch("root:" + root.id);
    setOpenCategories(previous => toggleSetItem(previous, "root:" + root.id));
  };
  const toggleCat = (node: CatalogNode) => {
    if (!node.hasChildren) return;
    retryBranch("category:" + node.id);
    setOpenCategories(previous => toggleSetItem(previous, "category:" + node.id));
  };
  const toggleKeyword = (node: CatalogNode) => {
    if (!node.hasChildren) return;
    retryBranch("keyword:" + node.id);
    setOpenKeywords(previous => toggleSetItem(previous, node.id));
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
  const finishCategoryMutation=(result:CategoryMutation,deleted=false)=>{if(result.categoryId!=null){setHighlightCategory(result.categoryId);setActiveCategory(prev=>prev?.id===result.categoryId?{...prev,parentId:result.parentId,rootId:result.rootId}:prev);setOpenCategories(previous => new Set([
      ...previous, ...(result.rootId == null ? [] : ["root:" + result.rootId]),
      ...result.ancestorIds.map(id => "category:" + id),
    ]));}if(deleted&&activeCategory&&result.deletedCategoryIds.includes(activeCategory.id))setActiveCategory(null);setCategoryMenu(null);setCategoryDialog(null);setDeleteDialog(null);onCategoryChanged(result,deleted);};
  const runCategoryCommand=async(command:string,args:Record<string,unknown>,deleted=false)=>{try{const result=await invoke<CategoryMutation>(command,args);if(command==="rename_category"&&result.categoryId!=null)setActiveCategory(prev=>prev?.id===result.categoryId?{...prev,name:String(args.name).trim()}:prev);finishCategoryMutation(result,deleted);}catch(error){setCategoryMenu(null);onCategoryError(error);}};
  const openCreateCategory = (parentId: number | null) => {
    setCategoryMenu(null);
    setCategoryDialog({kind:"create", parentId, name:"", iconKey:null});
  };
  const saveCategoryDialog = () => {
    if (!categoryDialog || readOnly) return;
    if (categoryDialog.kind === "create") {
      const { parentId, name, iconKey } = categoryDialog;
      void runCategoryCommand("create_category", { parentId, name, iconKey });
    } else if (categoryDialog.kind === "icon") {
      const { id, isRoot, iconKey } = categoryDialog;
      void runCategoryCommand("set_category_icon", { id, isRoot, iconKey });
    } else {
      void runCategoryCommand("rename_category", { id:categoryDialog.id, name:categoryDialog.name });
    }
  };
  const requestDeleteCategory=async(id:number)=>{setCategoryMenu(null);try{const info=await invoke<{id:number;name:string;assignedFiles:number;subcategories:number}>("get_category_delete_info",{id});setDeleteDialog({info,blocked:info.assignedFiles>0});}catch(error){onCategoryError(error);}};
  const confirmDeleteCategory=()=>{if(deleteDialog)void runCategoryCommand("delete_category",{id:deleteDialog.info.id},true);};
  const categoryDragStart=(event:React.DragEvent,id:number)=>{event.dataTransfer.setData("application/x-ddreams-category-id",String(id));event.dataTransfer.effectAllowed="move";};
  const categoryDragOver=(event:React.DragEvent,id:number)=>{if(!event.dataTransfer.types.includes("application/x-ddreams-category-id")&&!event.dataTransfer.types.includes("application/x-ddreams-asset-ids"))return;event.preventDefault();event.stopPropagation();setDropTarget(id);};
  const categoryDrop=(event:React.DragEvent,id:number)=>{event.preventDefault();event.stopPropagation();setDropTarget(null);setTopDropActive(false);const moving=event.dataTransfer.getData("application/x-ddreams-category-id");if(moving){const source=Number(moving);if(Number.isFinite(source)&&source!==id)void runCategoryCommand("move_category",{id:source,newParentId:id});return;}try{const ids:number[]=JSON.parse(event.dataTransfer.getData("application/x-ddreams-asset-ids"));if(ids.length)onSetCategory(ids,id,true);}catch{}};
  const categoryDragOverTop=(event:React.DragEvent)=>{if(!event.dataTransfer.types.includes("application/x-ddreams-category-id"))return;event.preventDefault();event.stopPropagation();setTopDropActive(true);};
  const categoryDropTop=(event:React.DragEvent)=>{event.preventDefault();event.stopPropagation();setTopDropActive(false);setDropTarget(null);const moving=event.dataTransfer.getData("application/x-ddreams-category-id");if(moving){const id=Number(moving);if(Number.isFinite(id))void runCategoryCommand("move_category",{id,newParentId:null});}};
  const categoryContext = (event: React.MouseEvent, id: number, name: string, parentId: number | null,
    rootId: number | null, iconKey: string | null, root?: Root) => {
    event.preventDefault();
    event.stopPropagation();
    const rootOnly = root != null && root.mergedCategoryId == null;
    if (!rootOnly) setActiveCategory({ id, name, parentId, rootId });
    const icon = { id: root?.id ?? id, isRoot: root != null, iconKey };
    setCategoryMenu({ x:event.clientX, y:event.clientY, id, name, parentId, icon, rootOnly });
  };
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
  const categoryTools=<><button className="category-add" title="Nová kategória" disabled={readOnly} onClick={()=>openCreateCategory(null)}>+</button><button className="collapse-all" onClick={()=>setOpenCategories(new Set())}>Zbaliť všetko</button><label className="include-children"><input type="checkbox" checked={includeChildren} onChange={e=>onIncludeChildrenChange(e.target.checked)}/>Vrátane podkategórií</label><label className="include-children" title="Zobraziť štvorčeky na priraďovanie vybraných súborov do kategórií"><input type="checkbox" checked={showAssignSquares} onChange={e=>setShowAssignSquares(e.target.checked)}/>Štvorčeky</label></>;
  const keywordTools=<><button className="category-add" title="Nové kľúčové slovo" onClick={()=>setKeywordDialog({kind:"create",parentId:null,name:""})}>+</button><button className="collapse-all" onClick={()=>setOpenKeywords(new Set())}>Zbaliť všetko</button></>;
  return {
    sections: {
      activeColors, aiAvailable, applyReview, assignCategory, assignKeyword, assignUnknownFaceCluster,
      assignedCategory, assignedCount, autoGroups, autoProgress, cancelLabelClick, catChildren,
      categoriesCollapsed, categoryAncestor, categoryContext, categoryDragOver, categoryDragOverTop,
      categoryDragStart, categoryDrop, categoryDropTop, categoryResults, categoryTools, cats, colorCounts,
      coverThumbs, createPerson, createPersonForUnknownCluster, delayLabelClick, dropTarget, facePersons,
      faceStatus, filteredKeywords, highlightCategory, highlightKeyword, ignoreUnknownFaceCluster, isCollapsed,
      keywordAssignedCount, keywordChildren, keywordContext, keywordDragOver, keywordDragOverTop,
      keywordDragStart, keywordDrop, keywordDropTarget, keywordDropTop, keywordQuery, keywordSelectionCounts,
      keywordTools, keywordTopDrop, keywordsCollapsed, labels, links, loadPersonReview, navigateCategory,
      navigateKeyword, navigateRoot, newPersonName, newPersonValue, onCategoryError, onNavigate, onNotice,
      onRefreshFacePersons, openCats, openKeywords, openPerson, openRoots, peopleQuery, personMenu, ratings,
      readOnly, recognizingFaces, reviewPerson, reviewRows, reviewSelected, reviewThumbs, rootRelated,
      rootSpringTimer, roots, runFaceRecognition, runUnknownFaceClusters, selectedIds, selectionCounts,
      setActiveCategory, setActiveKeyword, setCollapsed, setDropTarget, setKeywordDropTarget,
      setKeywordTopDrop, setNewPersonName, setNewPersonValue, setPeopleQuery, setPersonMenu, setReviewPerson,
      setReviewRows, setReviewSelected, setTopDropActive, setUnknownFaceCreate, showAssignSquares, special,
      toggleCat, toggleKeyword, toggleRoot, topDropActive, unknownFaceClusters, unknownFaceCreate,
      unknownFaceLoading, unknownFaceThumbs,
    },
    content: <><EditSelectionSection>{header}</EditSelectionSection><DockChrome id="catalogIntro">
    <div className="catalog-zone-title filter-zone">Filtrovanie – klik zobrazí fotky</div>
    <input className="keyword-filter" value={keywordQuery} onChange={e=>setKeywordQuery(e.target.value)} placeholder="Filtrovať kategórie a keywordy…" />
    </DockChrome>
    <MatchesSection/>
    <CategoriesSection/>
    <SpecialSection/>
    <PeopleSection/>
    <UnknownFacesSection/>
    <RatingsSection/>
    <LabelsSection/>
    <ColorsSection/>
    
     <KeywordsSection/>
     <AutoCategoriesSection/>
     {createPortal(<>{keywordMenu&&<div className="category-context-menu" style={{left:Math.min(keywordMenu.x,window.innerWidth-250),top:Math.min(keywordMenu.y,window.innerHeight-280)}} onMouseLeave={()=>setKeywordMenu(null)}>
       <button onClick={()=>{setKeywordMenu(null);setKeywordDialog({kind:"create",parentId:null,name:""})}}>Nové kľúčové slovo…</button>
       <button onClick={()=>{setKeywordMenu(null);setKeywordDialog({kind:"create",parentId:keywordMenu.id,name:""})}}>Nové podriadené slovo…</button>
       <button onClick={()=>{setKeywordDialog({kind:"rename",id:keywordMenu.id,name:keywordMenu.name});setKeywordMenu(null)}}>Premenovať (F2)</button>
       <button disabled={keywordMenu.parentId==null} onClick={()=>{void runKeywordCommand("move_keyword",{id:keywordMenu.id,newParentId:null});setKeywordMenu(null)}}>Presunúť na najvyššiu úroveň</button>
       <button onClick={()=>void requestDeleteKeyword(keywordMenu.id)}>Zmazať</button><hr />
       <button disabled={!aiReady} onClick={()=>{
         const {id,name}=keywordMenu;
         setKeywordMenu(null);
         onNotice(`Hľadám fotky pre „${name}“…`);
         void invoke<{ids:number[];examples:number}>("ai_keyword_matches",{keywordId:id}).then(result=>{
           if (!result.ids.length) {
             onNotice(result.examples ? "Nenašli sa ďalšie dosť podobné fotky." : "Tomuto slovu zatiaľ chýba zaindexovaná vzorová fotka.");
             return;
           }
           onNavigate({kind:"ai",ids:result.ids,title:`Podobné fotky · ${name}`,
             suggestion:{kind:"keyword",labelId:id,path:name}});
           onNotice(`Nájdené fotky: ${result.ids.length} · použité vzory: ${result.examples}`);
         }).catch(onCategoryError);
       }}>Nájsť ďalšie fotky pre toto slovo</button>
       <button disabled={!selectedIds.length||keywordSelectionCounts.totalFiles===0||keywordAssignedCount(keywordMenu.id)===keywordSelectionCounts.totalFiles} onClick={()=>{onSetKeyword(selectedIds,keywordMenu.id,true);setKeywordMenu(null)}}>Pridať vybraným súborom</button>
       <button disabled={!selectedIds.length||keywordAssignedCount(keywordMenu.id)===0} onClick={()=>{onSetKeyword(selectedIds,keywordMenu.id,false);setKeywordMenu(null)}}>Odobrať vybraným súborom</button>
     </div>}
     {keywordDialog&&<div className="category-editor-backdrop" onMouseDown={e=>{if(e.target===e.currentTarget)setKeywordDialog(null)}}><form className="category-editor-dialog" onSubmit={e=>{e.preventDefault();saveKeywordDialog()}}><strong>{keywordDialog.kind==="create"?(keywordDialog.parentId==null?"Nové kľúčové slovo":"Nové podriadené slovo"):"Premenovať kľúčové slovo"}</strong><label>Názov<input autoFocus maxLength={100} value={keywordDialog.name} onChange={e=>setKeywordDialog({...keywordDialog,name:e.target.value})} onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();setKeywordDialog(null)}}}/></label><div><button type="submit">OK</button><button type="button" onClick={()=>setKeywordDialog(null)}>Zrušiť</button></div></form></div>}
     {keywordDeleteDialog&&<div className="category-editor-backdrop"><div className="category-editor-dialog"><strong>Zmazať kľúčové slovo</strong>{keywordDeleteDialog.blocked?<><p>Kľúčové slovo {keywordDeleteDialog.info.name} nie je možné zmazať, obsahuje {keywordDeleteDialog.info.assignedFiles} {slovakFileCount(keywordDeleteDialog.info.assignedFiles)} (vrátane podriadených slov). Najprv z neho súbory odoberte.</p><div><button onClick={()=>{onIncludeChildrenChange(true);onNavigate({kind:"keyword",id:keywordDeleteDialog.info.id,title:`Kľúčové slovo: ${keywordDeleteDialog.info.name}`});setKeywordDeleteDialog(null)}}>Zobraziť súbory</button><button onClick={()=>setKeywordDeleteDialog(null)}>Zavrieť</button></div></>:<><p>{keywordDeleteDialog.info.subkeywords>0?`Zmazať kľúčové slovo ${keywordDeleteDialog.info.name} aj s ${keywordDeleteDialog.info.subkeywords} podriadenými slovami?`:`Zmazať kľúčové slovo ${keywordDeleteDialog.info.name}?`}</p><div><button onClick={confirmDeleteKeyword}>Zmazať</button><button onClick={()=>setKeywordDeleteDialog(null)}>Zrušiť</button></div></>}</div></div>}
     {categoryMenu&&<div className="category-context-menu" style={{left:Math.min(categoryMenu.x,window.innerWidth-250),top:Math.min(categoryMenu.y,window.innerHeight-250)}} onMouseLeave={()=>setCategoryMenu(null)}>
       <button disabled={readOnly} onClick={()=>{setCategoryDialog({kind:"icon",name:categoryMenu.name,...categoryMenu.icon});setCategoryMenu(null)}}>Zmeniť ikonu…</button>
       {!categoryMenu.rootOnly&&<>
       <button disabled={readOnly} onClick={()=>openCreateCategory(null)}>Nová kategória…</button>
       <button disabled={readOnly} onClick={()=>openCreateCategory(categoryMenu.id)}>Nová podkategória…</button>
       <button onClick={()=>{setCategoryDialog({kind:"rename",id:categoryMenu.id,name:categoryMenu.name});setCategoryMenu(null)}}>Premenovať (F2)</button>
       <button disabled={categoryMenu.parentId==null} onClick={()=>{void runCategoryCommand("move_category",{id:categoryMenu.id,newParentId:null});setCategoryMenu(null)}}>Presunúť na najvyššiu úroveň</button>
       <button onClick={()=>void requestDeleteCategory(categoryMenu.id)}>Zmazať</button>
       <hr />
       <button disabled={!selectedIds.length||selectionCounts.totalFiles===0||assignedCount(categoryMenu.id)===selectionCounts.totalFiles} onClick={()=>{onSetCategory(selectedIds,categoryMenu.id,true);setCategoryMenu(null)}}>Priradiť vybrané súbory</button><button disabled={!selectedIds.length||assignedCount(categoryMenu.id)===0} onClick={()=>{onSetCategory(selectedIds,categoryMenu.id,false);setCategoryMenu(null)}}>Odobrať vybrané súbory</button>
       </>}
     </div>}
     {categoryDialog&&<div className="category-editor-backdrop" onMouseDown={e=>{if(e.target===e.currentTarget)setCategoryDialog(null)}}>
       <form className="category-editor-dialog" onSubmit={e=>{e.preventDefault();saveCategoryDialog()}}
         onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();setCategoryDialog(null)}}}>
         <strong>{categoryDialog.kind==="icon"?"Zmeniť ikonu – "+categoryDialog.name
           :categoryDialog.kind==="create"?(categoryDialog.parentId==null?"Nová kategória":"Nová podkategória"):"Premenovať kategóriu"}</strong>
         {categoryDialog.kind!=="icon"&&<label>Názov<input autoFocus maxLength={100} value={categoryDialog.name}
           onChange={e=>setCategoryDialog({...categoryDialog,name:e.target.value})}/></label>}
         {categoryDialog.kind!=="rename"&&<CategoryIconPicker value={categoryDialog.iconKey} disabled={readOnly}
           onChange={iconKey=>setCategoryDialog({...categoryDialog,iconKey})}/>}
         <div><button type="submit" disabled={readOnly} autoFocus={categoryDialog.kind==="icon"}>OK</button>
           <button type="button" onClick={()=>setCategoryDialog(null)}>Zrušiť</button></div>
       </form>
     </div>}
     {deleteDialog&&<div className="category-editor-backdrop"><div className="category-editor-dialog"><strong>Zmazať kategóriu</strong>{deleteDialog.blocked?<><p>Kategóriu {deleteDialog.info.name} nie je možné zmazať, obsahuje {deleteDialog.info.assignedFiles} {deleteDialog.info.assignedFiles===1?"súbor":deleteDialog.info.assignedFiles<=4?"súbory":"súborov"} (vrátane podkategórií). Najprv z nej súbory odoberte.</p><div><button onClick={()=>{onIncludeChildrenChange(true);onNavigate({kind:"category",id:deleteDialog.info.id,title:`Kategória: ${deleteDialog.info.name}`});setDeleteDialog(null)}}>Zobraziť súbory</button><button onClick={()=>setDeleteDialog(null)}>Zavrieť</button></div></>:<><p>{deleteDialog.info.subcategories>0?`Zmazať kategóriu ${deleteDialog.info.name} aj s ${deleteDialog.info.subcategories} ${deleteDialog.info.subcategories===1?"podkategóriou":"podkategóriami"}?`:`Zmazať kategóriu ${deleteDialog.info.name}?`}</p><div><button onClick={confirmDeleteCategory}>Zmazať</button><button onClick={()=>setDeleteDialog(null)}>Zrušiť</button></div></>}</div></div>}
  </>, document.body)}</>,
  };
}

export type CatalogSectionData = ReturnType<typeof useCatalogPanelModel>["sections"];

export function CatalogPanel(props: Parameters<typeof useCatalogPanelModel>[0]) {
  const { sections, content } = useCatalogPanelModel(props);
  return <RightSectionContext.Provider value={{ catalog: sections }}>{content}</RightSectionContext.Provider>;
}

function MatchesSection(): ReactElement {
  const {
    assignedCategory, categoryAncestor, categoryResults, isCollapsed, keywordQuery, navigateCategory,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.matches" available={!!keywordQuery.trim()}>{keywordQuery.trim()&&<section className="catalog-section"><SectionHeading id="catalog.matches" title="Zhody kategórií"/>{!isCollapsed("catalog.matches")&&categoryResults.map(node=><button className={`tree-row tree-label keyword-result ${assignedCategory(node.id)?"catalog-assigned":categoryAncestor(node.id)?"catalog-ancestor":""}`} key={node.id} onClick={()=>navigateCategory(node)}><span>{assignedCategory(node.id)?"✓":categoryAncestor(node.id)?"•":""}</span>{node.name}<small>{node.count}</small></button>)}</section>}</DockSection>;
}

function CategoriesSection(): ReactElement {
  const {
    assignCategory, assignedCategory, assignedCount, cancelLabelClick, catChildren, categoriesCollapsed,
    categoryContext, categoryDragOver, categoryDragOverTop, categoryDragStart, categoryDrop, categoryDropTop,
    categoryTools, cats, delayLabelClick, dropTarget, highlightCategory, links, navigateCategory, navigateRoot,
    openCats, openRoots, rootRelated, rootSpringTimer, roots, selectedIds, selectionCounts, setActiveCategory,
    setDropTarget, setTopDropActive, showAssignSquares, toggleCat, toggleRoot, topDropActive,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.categories"><section className="catalog-section" onDragOver={categoryDragOverTop} onDragLeave={()=>setTopDropActive(false)} onDrop={categoryDropTop}><SectionHeading id="catalog.categories" title="Kategórie" tools={categoryTools}/>{topDropActive&&<div className="category-top-drop">Presunúť na najvyššiu úroveň</div>}{!categoriesCollapsed&&roots.map(root=>{
      const direct=root.mergedCategoryId!=null&&assignedCategory(root.mergedCategoryId);
      const related=rootRelated(root.id)&&!direct;
      const rootSpring=springOpenHandlers(rootSpringTimer,openRoots.has(root.id),root.hasChildren,()=>{if(!openRoots.has(root.id))void toggleRoot(root)});
      return <div key={root.id}><div className={`tree-row ${root.mergedCategoryId!=null&&dropTarget===root.mergedCategoryId?"category-drop-target":""}`} onDragEnter={rootSpring.onDragEnter} onDragOver={e=>{rootSpring.onDragOver(e);if(root.mergedCategoryId!=null)categoryDragOver(e,root.mergedCategoryId)}} onDragLeave={e=>{rootSpring.onDragLeave(e);setDropTarget(null)}} onDrop={e=>{rootSpring.onDrop();if(root.mergedCategoryId!=null)categoryDrop(e,root.mergedCategoryId)}}>{showAssignSquares&&root.mergedCategoryId!=null&&<AssignSquare disabled={!selectedIds.length||selectionCounts.totalFiles===0} count={assignedCount(root.mergedCategoryId)} total={selectionCounts.totalFiles} onClick={()=>assignCategory(root.mergedCategoryId!)}/>}<TreeArrow visible={root.hasChildren} open={openRoots.has(root.id)} onClick={()=>void toggleRoot(root)}/><button draggable={root.mergedCategoryId!=null} className={`tree-label ${highlightCategory===root.mergedCategoryId?"category-new-highlight":""} ${direct?"catalog-assigned":related?"catalog-ancestor":""}`} onDragStart={root.mergedCategoryId==null?undefined:e=>categoryDragStart(e,root.mergedCategoryId!)} onContextMenu={e=>categoryContext(e,root.mergedCategoryId??root.id,root.name,null,root.id,root.iconKey,root)} onClick={e=>{if(root.mergedCategoryId!=null)setActiveCategory({id:root.mergedCategoryId,name:root.name,parentId:null,rootId:root.id});if(e.detail<=1)delayLabelClick(()=>navigateRoot(root));}} onDoubleClick={()=>{cancelLabelClick();void toggleRoot(root);}}><CategoryIcon iconKey={root.iconKey} root /><span className="tree-text">{direct?"✓ ":related?"• ":""}{root.name}</span><small>{root.count||""}</small></button></div>{openRoots.has(root.id)&&(cats.get(root.id)??[]).map(cat=><CategoryNode key={cat.id} node={cat} inheritedIconKey={root.iconKey} depth={1} open={openCats} childrenMap={catChildren} onToggle={toggleCat} onNavigate={navigateCategory} onLabelClick={delayLabelClick} onLabelDoubleClick={cancelLabelClick} assignedIds={new Set(links?.categoryIds??[])} ancestorIds={new Set(links?.categoryAncestorIds??[])} showSquare={showAssignSquares} count={assignedCount} total={selectionCounts.totalFiles} disabled={!selectedIds.length} onAssign={assignCategory} onActivate={setActiveCategory} onContextMenu={categoryContext} onDragStart={categoryDragStart} highlightId={highlightCategory} dropTarget={dropTarget} onDragOver={categoryDragOver} onDragLeave={()=>setDropTarget(null)} onDrop={categoryDrop}/>)}</div>;
    })}</section></DockSection>;
}

function SpecialSection(): ReactElement {
  const {
    isCollapsed, onNavigate, special,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.special"><section className="catalog-section"><SectionHeading id="catalog.special" title="Špeciálne položky"/>{!isCollapsed("catalog.special")&&[["all_photos","Všetky fotky"],["uncategorized","Bez kategórie"],["unkeyworded","Bez keywordov"],["quarantine","Karanténa"],["tagged","Označené"],["delete_suggested","Návrh na vymazanie"]].map(([value,label])=><button className="value-row" key={value} onClick={()=>onNavigate({kind:"special",value,title:label})}><span>{label}</span><small>{special.find(s=>s.value===value)?.count??0}</small></button>)}</section></DockSection>;
}

function PeopleSection(): ReactElement {
  const {
    applyReview, coverThumbs, createPerson, facePersons, faceStatus, isCollapsed, loadPersonReview,
    newPersonName, newPersonValue, onCategoryError, onNavigate, onNotice, onRefreshFacePersons, openPerson,
    peopleQuery, personMenu, readOnly, recognizingFaces, reviewPerson, reviewRows, reviewSelected,
    reviewThumbs, runFaceRecognition, setNewPersonName, setNewPersonValue, setPeopleQuery, setPersonMenu,
    setReviewPerson, setReviewRows, setReviewSelected,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.people"><section className="catalog-section people-section"><SectionHeading id="catalog.people" title="Osoby"/>{!isCollapsed("catalog.people")&&<><div className="people-status">{faceStatus.available?`Tváre: ${faceStatus.faces} · ${faceStatus.named} pomenovaných · neskenovaných ${faceStatus.pending}`:"Model tvárí nie je nainštalovaný"}</div>{faceStatus.available&&<><input className="keyword-filter" value={peopleQuery} onChange={event=>setPeopleQuery(event.target.value)} placeholder="Filtrovať osoby…"/><div className="people-toolbar"><button disabled={readOnly} onClick={()=>{setNewPersonName("");setNewPersonValue("")}}>Nová osoba…</button><button disabled={readOnly||recognizingFaces} onClick={()=>void runFaceRecognition()}>{recognizingFaces?"Prepočítavam…":"Prepočítať rozpoznanie"}</button></div>{newPersonName!==null&&<form className="person-create-form" onSubmit={event=>{event.preventDefault();void createPerson()}}><input autoFocus maxLength={100} value={newPersonValue} placeholder="Meno osoby" onChange={event=>setNewPersonValue(event.target.value)} onKeyDown={event=>{if(event.key==='Escape')setNewPersonName(null)}}/><button disabled={readOnly||!newPersonValue.trim()}>OK</button><button type="button" onClick={()=>setNewPersonName(null)}>Zrušiť</button></form>}{reviewPerson?<div className="face-review-panel"><div className="face-review-title"><strong>Kontrola: {reviewPerson.name}</strong><button onClick={()=>{setReviewPerson(null);setReviewRows([]);setReviewSelected(new Set())}}>Zavrieť</button></div>{reviewRows.length===0?<small className="people-muted">Žiadne nepotvrdené tváre.</small>:<><div className="face-review-grid">{reviewRows.map(face=><label className={`face-review-tile ${reviewSelected.has(face.faceId)?'selected':''}`} key={face.faceId} onClick={event=>{if((event.target as HTMLElement).closest('input'))return;onNavigate({kind:'people',ids:[face.assetId],title:`Osoba: ${reviewPerson.name}`})}}><input type="checkbox" checked={reviewSelected.has(face.faceId)} onChange={event=>setReviewSelected(current=>{const next=new Set(current);if(event.target.checked)next.add(face.faceId);else next.delete(face.faceId);return next})}/><img src={reviewThumbs[face.faceId]} alt=""/><small>{face.score==null?'—':`${(face.score*100).toFixed(1)}%`}</small></label>)}</div><div className="face-review-actions"><button disabled={readOnly||!reviewSelected.size} onClick={()=>void applyReview('confirm',[...reviewSelected])}>Potvrdiť vybrané</button><button disabled={readOnly||!reviewSelected.size} onClick={()=>void applyReview('reject',[...reviewSelected])}>Nie je to on/ona (vybrané)</button><button disabled={readOnly} onClick={()=>void applyReview('confirm',reviewRows.map(face=>face.faceId))}>Potvrdiť všetky zobrazené</button></div></>}</div>:<div className="people-list">{facePersons.filter(person=>person.name.toLocaleLowerCase('sk').includes(peopleQuery.trim().toLocaleLowerCase('sk'))).map(person=><div className="face-person-row" key={person.id}><button className="face-person-open" title={`Zobraziť fotky osoby ${person.name}`} onClick={()=>void openPerson(person)}>{person.cover!=null&&coverThumbs[person.cover]?<img src={coverThumbs[person.cover]} alt=""/>:<span className="face-person-placeholder">●</span>}<span className="face-person-name">{person.name}</span><small>{person.confirmed+person.auto}{person.auto>0&&<em>(+{person.auto} na kontrolu)</em>}</small></button><button className="face-person-menu-button" aria-label={`Možnosti osoby ${person.name}`} onClick={()=>setPersonMenu(personMenu===person.id?null:person.id)}>⋯</button>{personMenu===person.id&&<div className="face-person-menu"><button disabled={readOnly} onClick={()=>{setPersonMenu(null);const name=window.prompt('Premenovať osobu',person.name);if(name?.trim())void invoke<boolean>('ai_faces_person_rename',{personId:person.id,name:name.trim()}).then(()=>{void onRefreshFacePersons();onNotice(`Premenované: ${name.trim()}`)}).catch(onCategoryError)}}>Premenovať…</button>{person.auto>0&&<button onClick={()=>{setPersonMenu(null);void loadPersonReview(person)}}>Skontrolovať nepotvrdené</button>}<button onClick={()=>{setPersonMenu(null);void openPerson(person,false)}}>Zobraziť len potvrdené</button></div>}</div>)}{facePersons.length===0&&<small className="people-muted">Zatiaľ nie sú známe osoby.</small>}</div>}</>}</>}</section></DockSection>;
}

function UnknownFacesSection(): ReactElement {
  const {
    assignUnknownFaceCluster, createPersonForUnknownCluster, facePersons, faceStatus, ignoreUnknownFaceCluster,
    isCollapsed, onNavigate, readOnly, runUnknownFaceClusters, setUnknownFaceCreate, unknownFaceClusters,
    unknownFaceCreate, unknownFaceLoading, unknownFaceThumbs,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.unknownFaces"><section className="catalog-section unknown-face-clusters"><SectionHeading id="catalog.unknownFaces"/>{!isCollapsed("catalog.unknownFaces")&&<><button disabled={!faceStatus.available||unknownFaceLoading} title={!faceStatus.available?"Model tvárí nie je nainštalovaný":""} onClick={()=>void runUnknownFaceClusters()}>{unknownFaceLoading?"Hľadám…":"Nájsť skupiny neznámych tvárí"}</button>{unknownFaceClusters&&<small className="cluster-summary">{unknownFaceClusters.clustered} z {unknownFaceClusters.unknown} neznámych tvárí v {unknownFaceClusters.groups.length} skupinách</small>}{unknownFaceClusters&&!unknownFaceClusters.groups.length&&<small className="people-muted">Nenašiel som žiadne skupiny.</small>}{unknownFaceClusters?.groups.map(group=><article className="cluster-card" key={group.faceIds[0]}><div className="unknown-face-thumbs">{group.faceIds.slice(0,8).map(id=>unknownFaceThumbs[id]&&<img key={id} src={unknownFaceThumbs[id]} alt=""/>)}</div><strong>{group.total} tvárí na {group.assets} fotkách</strong>{group.guess&&<small>Možno: {group.guess.name} ({(group.guess.score*100).toFixed(0)}%)</small>}<div className="cluster-actions">{group.guess&&<button disabled={readOnly} onClick={()=>void assignUnknownFaceCluster(group,group.guess!.personId,group.guess!.name)}>Je to {group.guess.name}</button>}<select aria-label="Priradiť osobe" value="" disabled={readOnly||facePersons.length===0} onChange={event=>{const person=facePersons.find(item=>item.id===Number(event.target.value));if(person)void assignUnknownFaceCluster(group,person.id,person.name)}}><option value="">Priradiť osobe…</option>{facePersons.map(person=><option key={person.id} value={person.id}>{person.name}</option>)}</select><button disabled={readOnly} onClick={()=>setUnknownFaceCreate({group,name:group.guess?.name??""})}>Nová osoba…</button><button onClick={()=>onNavigate({kind:"people",ids:group.assetIds,title:`Neznáma osoba (${group.total} tvárí)`})}>Zobraziť fotky</button><button disabled={readOnly} onClick={()=>void ignoreUnknownFaceCluster(group)}>Ignorovať</button></div>{unknownFaceCreate?.group===group&&<form className="cluster-create" onSubmit={event=>{event.preventDefault();void createPersonForUnknownCluster()}}><input autoFocus maxLength={100} value={unknownFaceCreate.name} onChange={event=>setUnknownFaceCreate({...unknownFaceCreate,name:event.target.value})}/><button disabled={readOnly||!unknownFaceCreate.name.trim()}>OK</button><button type="button" onClick={()=>setUnknownFaceCreate(null)}>Zrušiť</button></form>}</article>)}</>}</section></DockSection>;
}

function RatingsSection(): ReactElement {
  const {
    isCollapsed, onNavigate, ratings,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.ratings"><section className="catalog-section"><SectionHeading id="catalog.ratings" title="Hodnotenie"/>{!isCollapsed("catalog.ratings")&&ratings.map(r=><button className="value-row" key={r.value} onClick={()=>onNavigate({kind:"rating",value:r.value,title:r.value==="unrated"?"Bez hodnotenia":`Hodnotenie ${r.value} ★`})}><span>{r.value==="unrated"?"Bez hodnotenia":"★".repeat(Number(r.value))}</span><small>{r.count}</small></button>)}</section></DockSection>;
}

function LabelsSection(): ReactElement {
  const {
    isCollapsed, labels, onNavigate,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.labels"><section className="catalog-section"><SectionHeading id="catalog.labels" title="Farebné štítky"/>{!isCollapsed("catalog.labels")&&labels.filter(l=>l.value).map(l=><button className="value-row" key={l.value} onClick={()=>onNavigate({kind:"label",value:l.value,title:`Farebný štítok: ${labelText(l.value)}`})}><span><i className="label-dot" style={{background:labelColor(l.value)}}/>{labelText(l.value)}</span><small>{l.count}</small></button>)}</section></DockSection>;
}

function ColorsSection(): ReactElement {
  const {
    activeColors, aiAvailable, colorCounts, isCollapsed, onNavigate,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.colors"><section className="catalog-section"><SectionHeading id="catalog.colors" title="FARBY"/>{!isCollapsed("catalog.colors")&&aiAvailable&&<small className="color-hint">Ctrl+klik = viac farieb naraz</small>}{!isCollapsed("catalog.colors")&&(aiAvailable?colorCounts.map(color=><button className={`value-row${activeColors.split(",").includes(color.value)?" active":""}`} key={color.value} title="Ctrl+klik pridá ďalšiu farbu – zobrazia sa fotky, ktoré obsahujú všetky vybrané" onClick={e=>{const current=activeColors?activeColors.split(","):[];const next=(e.ctrlKey||e.metaKey)?(current.includes(color.value)?current.filter(v=>v!==color.value):[...current,color.value]):[color.value];if(!next.length)return;const names=next.map(v=>colorCounts.find(c=>c.value===v)?.name??v);onNavigate({kind:"color",value:next.join(","),title:next.length>1?`Farby: ${names.join(" + ")}`:`Farba: ${names[0]}`})}}><span><i className="label-dot" style={{background:color.hex}}/>{color.name}</span><small>{color.count.toLocaleString("sk-SK")}</small></button>):<small className="color-not-computed">AI modul nie je nainštalovaný</small>)}</section></DockSection>;
}

function KeywordsSection(): ReactElement {
  const {
    assignKeyword, cancelLabelClick, delayLabelClick, filteredKeywords, highlightKeyword, keywordAssignedCount,
    keywordChildren, keywordContext, keywordDragOver, keywordDragOverTop, keywordDragStart, keywordDrop,
    keywordDropTarget, keywordDropTop, keywordSelectionCounts, keywordTools, keywordTopDrop, keywordsCollapsed,
    links, navigateKeyword, openKeywords, selectedIds, setActiveKeyword, setKeywordDropTarget,
    setKeywordTopDrop, showAssignSquares, toggleKeyword,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.keywords"><section className="catalog-section keyword-section" onDragOver={keywordDragOverTop} onDragLeave={()=>setKeywordTopDrop(false)} onDrop={keywordDropTop}><SectionHeading id="catalog.keywords" title="Kľúčové slová" tools={keywordTools}/>{keywordTopDrop&&<div className="category-top-drop">Presunúť na najvyššiu úroveň</div>}{!keywordsCollapsed&&filteredKeywords.map(node=><KeywordNode key={node.id} node={node} depth={0} open={openKeywords} childrenMap={keywordChildren} onToggle={toggleKeyword} onNavigate={navigateKeyword} onLabelClick={delayLabelClick} onLabelDoubleClick={cancelLabelClick} assignedIds={new Set(links?.keywordIds??[])} ancestorIds={new Set(links?.keywordAncestorIds??[])} showSquare={showAssignSquares} count={keywordAssignedCount} total={keywordSelectionCounts.totalFiles} disabled={!selectedIds.length} onAssign={assignKeyword} onActivate={setActiveKeyword} onContextMenu={keywordContext} onDragStart={keywordDragStart} highlightId={highlightKeyword} dropTarget={keywordDropTarget} onDragOver={keywordDragOver} onDragLeave={()=>setKeywordDropTarget(null)} onDrop={keywordDrop}/>)}</section></DockSection>;
}

function AutoCategoriesSection(): ReactElement {
  const {
    autoGroups, autoProgress, isCollapsed, onNavigate, setCollapsed,
  } = useRightSectionData("catalog");
  return <DockSection id="catalog.autoCategories"><section className="catalog-section"><SectionHeading id="catalog.autoCategories" title="Auto kategórie"/>{!isCollapsed("catalog.autoCategories")&&<>{autoProgress?.running&&<small className="color-hint">Indexujem EXIF: {autoProgress.done.toLocaleString("sk-SK")} / {autoProgress.total.toLocaleString("sk-SK")}</small>}{autoProgress?.error&&<small className="settings-error">{autoProgress.error}</small>}{autoGroups.map(group=><div key={group.key}><div className="tree-row" style={{paddingLeft:8}}><button className="tree-toggle" aria-expanded={!isCollapsed("catalog.autoGroup:" + group.key)} onClick={()=>setCollapsed("catalog.autoGroup:" + group.key)}>{!isCollapsed("catalog.autoGroup:" + group.key)?"▾":"▸"}</button><button className="tree-label" onClick={()=>setCollapsed("catalog.autoGroup:" + group.key)}>{group.name}</button></div>{!isCollapsed("catalog.autoGroup:" + group.key)&&group.values.map(item=>{const {value,label}=item;return <button className="value-row" style={{paddingLeft:30}} key={value} onClick={()=>onNavigate({kind:"auto",value:`${group.key}\t${value}`,title:`${group.name}: ${label}`})}><span>{label}</span><small>{item.count.toLocaleString("sk-SK")}</small></button>})}</div>)}</>}</section></DockSection>;
}
function TreeArrow({visible,open,onClick}:{visible:boolean;open:boolean;onClick:()=>void}){return visible?<button className="tree-toggle" aria-label={open?"Zbaliť":"Rozbaliť"} onClick={onClick}>{open?"▾":"▸"}</button>:<span className="tree-toggle-placeholder"/>;}
function slovakFileCount(value:number){return value===1?"súbor":value>=2&&value<=4?"súbory":"súborov";}
function AssignSquare({disabled,count,total,onClick,noun="kategórie"}:{disabled:boolean;count:number;total:number;onClick:()=>void;noun?:string}){const checked=total>0&&count===total,partial=count>0&&!checked;return <button type="button" role="checkbox" aria-checked={partial?"mixed":checked} disabled={disabled} className={`category-assign-box ${checked?"checked":""} ${partial?"partial":""}`} title={partial?`${count} z ${total} súborov priradených`:checked?`Odobrať z ${noun}`:`Priradiť do ${noun}`} onClick={e=>{e.stopPropagation();onClick()}}>{checked?"✓":partial?"–":""}</button>}
function CategoryNode({node,inheritedIconKey,depth,open,childrenMap,onToggle,onNavigate,onLabelClick,onLabelDoubleClick,assignedIds,ancestorIds,showSquare,count,total,disabled,onAssign,onActivate,onContextMenu,onDragStart,highlightId,dropTarget,onDragOver,onDragLeave,onDrop}:{node:CatalogNode;inheritedIconKey:string|null;depth:number;open:Set<number>;childrenMap:Map<number,CatalogNode[]>;onToggle:(node:CatalogNode)=>void;onNavigate:(node:CatalogNode)=>void;onLabelClick:(action:()=>void)=>void;onLabelDoubleClick:()=>void;assignedIds:Set<number>;ancestorIds:Set<number>;showSquare:boolean;count:(id:number)=>number;total:number;disabled:boolean;onAssign:(id:number)=>void;onActivate:(value:{id:number;name:string;parentId:number|null;rootId:number|null})=>void;onContextMenu:(event:React.MouseEvent,id:number,name:string,parentId:number|null,rootId:number|null,iconKey:string|null)=>void;onDragStart:(event:React.DragEvent,id:number)=>void;highlightId:number|null;dropTarget:number|null;onDragOver:(event:React.DragEvent,id:number)=>void;onDragLeave:()=>void;onDrop:(event:React.DragEvent,id:number)=>void}){const iconKey=node.iconKey??inheritedIconKey;const assigned=assignedIds.has(node.id),ancestor=!assigned&&ancestorIds.has(node.id);const spring=useSpringOpen(open.has(node.id),node.hasChildren,()=>onToggle(node));return <div><div className={`tree-row ${dropTarget===node.id?"category-drop-target":""}`} style={{paddingLeft:8+depth*14}} onDragEnter={spring.onDragEnter} onDragOver={e=>{spring.onDragOver(e);onDragOver(e,node.id)}} onDragLeave={e=>{spring.onDragLeave(e);onDragLeave()}} onDrop={e=>{spring.onDrop();onDrop(e,node.id)}}>{showSquare&&<AssignSquare disabled={disabled||total===0} count={count(node.id)} total={total} onClick={()=>onAssign(node.id)}/>}<TreeArrow visible={node.hasChildren} open={open.has(node.id)} onClick={()=>onToggle(node)}/><button draggable className={`tree-label ${highlightId===node.id?"category-new-highlight":""} ${assigned?"catalog-assigned":ancestor?"catalog-ancestor":""}`} onDragStart={e=>onDragStart(e,node.id)} onContextMenu={e=>onContextMenu(e,node.id,node.name,node.parentId,node.rootId,node.iconKey??null)} onClick={e=>{onActivate({id:node.id,name:node.name,parentId:node.parentId,rootId:node.rootId});if(e.detail<=1)onLabelClick(()=>onNavigate(node));}} onDoubleClick={()=>{onLabelDoubleClick();onToggle(node);}}><CategoryIcon iconKey={iconKey} />{(assigned||ancestor)&&<span className="category-assignment-mark">{assigned?"✓":"•"}</span>}<span className="tree-text">{node.name}</span><small>{node.count||""}</small></button></div>{open.has(node.id)&&(childrenMap.get(node.id)??[]).map(child=><CategoryNode key={child.id} node={child} inheritedIconKey={iconKey} depth={depth+1} open={open} childrenMap={childrenMap} onToggle={onToggle} onNavigate={onNavigate} onLabelClick={onLabelClick} onLabelDoubleClick={onLabelDoubleClick} assignedIds={assignedIds} ancestorIds={ancestorIds} showSquare={showSquare} count={count} total={total} disabled={disabled} onAssign={onAssign} onActivate={onActivate} onContextMenu={onContextMenu} onDragStart={onDragStart} highlightId={highlightId} dropTarget={dropTarget} onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}/>)}</div>}
function EditSelectionSection({ children }: { children: ReactNode }) {
  const id = "catalog.edit";
  const { isCollapsed, setCollapsed } = useSectionCollapse();
  const menu = useSectionMenu(id);
  return <DockSection id={id}><details className="catalog-edit-disclosure" open={!isCollapsed(id)}
    onToggle={event => { if (event.target === event.currentTarget) setCollapsed(id, !event.currentTarget.open); }}>
    <summary {...menu}>Upraviť vybrané fotky <SectionHandle id={id}/></summary>{children}
  </details></DockSection>;
}

function KeywordNode({node,depth,open,childrenMap,onToggle,onNavigate,onLabelClick,onLabelDoubleClick,assignedIds,ancestorIds,showSquare,count,total,disabled,onAssign,onActivate,onContextMenu,onDragStart,highlightId,dropTarget,onDragOver,onDragLeave,onDrop}:{node:CatalogNode;depth:number;open:Set<number>;childrenMap:Map<number,CatalogNode[]>;onToggle:(node:CatalogNode)=>void;onNavigate:(node:CatalogNode)=>void;onLabelClick:(action:()=>void)=>void;onLabelDoubleClick:()=>void;assignedIds:Set<number>;ancestorIds:Set<number>;showSquare:boolean;count:(id:number)=>number;total:number;disabled:boolean;onAssign:(id:number)=>void;onActivate:(value:{id:number;name:string;parentId:number|null})=>void;onContextMenu:(event:React.MouseEvent,id:number,name:string,parentId:number|null)=>void;onDragStart:(event:React.DragEvent,id:number)=>void;highlightId:number|null;dropTarget:number|null;onDragOver:(event:React.DragEvent,id:number)=>void;onDragLeave:()=>void;onDrop:(event:React.DragEvent,id:number)=>void}){
  const assigned=assignedIds.has(node.id),ancestor=!assigned&&ancestorIds.has(node.id);const spring=useSpringOpen(open.has(node.id),node.hasChildren,()=>onToggle(node));return <div><div className={`tree-row ${dropTarget===node.id?"category-drop-target":""}`} style={{paddingLeft:8+depth*14}} onDragEnter={spring.onDragEnter} onDragOver={event=>{spring.onDragOver(event);onDragOver(event,node.id)}} onDragLeave={event=>{spring.onDragLeave(event);onDragLeave()}} onDrop={event=>{spring.onDrop();onDrop(event,node.id)}}>{showSquare&&<AssignSquare disabled={disabled||total===0} count={count(node.id)} total={total} onClick={()=>onAssign(node.id)} noun="kľúčového slova"/>}<TreeArrow visible={node.hasChildren} open={open.has(node.id)} onClick={()=>onToggle(node)}/><button draggable className={`tree-label ${highlightId===node.id?"category-new-highlight":""} ${assigned?"catalog-assigned":ancestor?"catalog-ancestor":""}`} onDragStart={event=>onDragStart(event,node.id)} onContextMenu={event=>onContextMenu(event,node.id,node.name,node.parentId)} onClick={event=>{onActivate({id:node.id,name:node.name,parentId:node.parentId});if(event.detail<=1)onLabelClick(()=>onNavigate(node));}} onDoubleClick={()=>{onLabelDoubleClick();onToggle(node);}}><span className="keyword-icon">{assigned?"✓":ancestor?"•":"⌑"}</span><span className="tree-text">{node.name}</span><small>{node.count||""}</small></button></div>{open.has(node.id)&&(childrenMap.get(node.id)??[]).map(child=><KeywordNode key={child.id} node={child} depth={depth+1} open={open} childrenMap={childrenMap} onToggle={onToggle} onNavigate={onNavigate} onLabelClick={onLabelClick} onLabelDoubleClick={onLabelDoubleClick} assignedIds={assignedIds} ancestorIds={ancestorIds} showSquare={showSquare} count={count} total={total} disabled={disabled} onAssign={onAssign} onActivate={onActivate} onContextMenu={onContextMenu} onDragStart={onDragStart} highlightId={highlightId} dropTarget={dropTarget} onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}/>)}</div>
}

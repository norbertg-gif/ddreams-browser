import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type WheelEvent as ReactWheelEvent } from "react";
import Filmstrip from "./components/Filmstrip";
import { launchEditor, useEditors, type Editor } from "./editors";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createPortal } from "react-dom";
import AssetGrid, { formatBytes, formatFor, isImage, labelText, thumbUrl, type AssetGridHandle, type AssetValuePatch } from "./components/AssetGrid";
import MapView from "./components/MapView";
import LeftPanel from "./components/LeftPanel";
import RightPanel from "./components/RightPanel";
import ThumbnailInfoDialog from "./components/ThumbnailInfoDialog";
import SettingsDialog from "./components/SettingsDialog";
import QuarantineReview, { type QuarantineOutcome, type QuarantineSettings, type Review } from "./components/QuarantineReview";
import IncomingTools, { type DeletionSuggestionChange } from "./components/IncomingTools";
import HelpDialog from "./components/HelpDialog";
import BackupReminder from "./components/BackupReminder";
import { useTransientDetails } from "./uiState";
import { FOLDER_RULES_EVENT, type FolderRuleMode } from "./folderRules";
import FileOperationDialog from "./components/FileOperationDialog";
import FileNameDialog from "./components/FileNameDialog";
import { DockLayoutMenu, DockSections, DockSide, useDockDrag, useDockHosts, useDockLayout, useDockPresence } from "./components/Dock";
import type { Asset, Folder, FormatTable, Page, View, ViewFilters, ValueCount } from "./types";
import { readStored, writeStored } from "./storage";
import { buildAssetFilter, effectiveViewFilters, readViewSort, sortKeyForView, updateViewSort, type SavedViewSorts } from "./assetFilter";
import { DEFAULT_THUMBNAIL_INFO, normalizeThumbnailInfo, type ThumbnailInfoConfig } from "./thumbnailInfo";
import "./App.css";
import "./manage.css";

const DEFAULT_PATH = String.raw`F:\My_docs\My Pictures\Fotky`;
type AppInfo = { root: string; catalog: string; backups: string; cache: string; exiftool: string; formats: string; metadataView: string; settings: string; portable: boolean; readonly: boolean };
type RecentChange = { id:number; ts:string; summary:string };
type CategoryMutation = { changeId:number|null; summary:string; categoryId:number|null; parentId:number|null; rootId:number|null; ancestorIds:number[]; deletedCategoryIds:number[]; deletedRootIds:number[] };
type KeywordMutation = { changeId:number|null; summary:string; keywordId:number|null; parentId:number|null; ancestorIds:number[]; deletedKeywordIds:number[] };
type ExclusionImpact = { files:number; withCategories:number; withKeywords:number; withRatingLabelTagged:number; withColorOverrides:number };
type RemoveCatalogInfo={assets:number;folders:number;offline:boolean;missing:boolean};
const folderParentPath=(input:string)=>{const clean=input.replace(/[\\/]+$/g,"");if(clean.startsWith("\\\\")){const parts=clean.slice(2).split(/[\\/]+/);return parts.length<=2?"":"\\\\"+parts.slice(0,-1).join("\\")}const parts=clean.split(/[\\/]+/);if(parts.length<=1)return "";const parent=parts.slice(0,-1).join("\\");return /^[A-Za-z]:$/.test(parent)?parent+"\\":parent};
type RemoveCatalogResult={summary:string;removedAssets:number;removedFolders:number;assetIds:number[]};
type AiStatus = { available:boolean; reason:string|null; state:string; modelState:string; waiting:number; indexed:number; queued:number; done:number; failed:number; offline:number; total:number; ratePerS:number; etaS:number|null; lastError:string|null };
type AiSearchPage = { ids:number[]; searched:number };
type SeriesPreset = "strict" | "normal" | "loose";
type AiSeriesResponse = { groups:{ids:number[];best:number}[]; analyzed:number; truncated:boolean };

function slovakItems(count:number){return `${count.toLocaleString("sk-SK")} ${count===1?"položka":count>=2&&count<=4?"položky":"položiek"}`;}
function dateText(value: string | null | undefined) { return value ? value.replace("T", " ").slice(0, 19) : "—"; }

export default function App() {
  const seriesMenu = useTransientDetails();
  const filtersMenu = useTransientDetails();
  const displayMenu = useTransientDetails();
  const [helpOpen, setHelpOpen] = useState(false);
  useEffect(() => {
    const handleHelp = (event: globalThis.KeyboardEvent) => {
      if (event.key === "F1" || (helpOpen && event.key === "Escape")) {
        event.preventDefault();
        event.stopImmediatePropagation();
        if (event.key === "F1") setHelpOpen(true); else setHelpOpen(false);
      }
    };
    window.addEventListener("keydown", handleHelp, true);
    return () => window.removeEventListener("keydown", handleHelp, true);
  }, [helpOpen]);
  const editors = useEditors();
  const openEditor = (editor: Editor, assetIds: number[]) => { if (assetIds.length) void launchEditor(editor, assetIds).catch(error => window.alert(String(error))); };
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  useEffect(() => { void invoke<AppInfo>("get_app_info").then(setAppInfo).catch(() => {}); }, []);
  const catalogTooltip = appInfo ? `Katalóg: ${appInfo.catalog}\nZálohy: ${appInfo.backups}\nCache: ${appInfo.cache}\nExifTool: ${appInfo.exiftool}\nFormáty: ${appInfo.formats}\nMetadata: ${appInfo.metadataView}\nNastavenia: ${appInfo.settings}\nRežim: ${appInfo.portable ? "prenosný" : "vývojový"}` : "Cesty katalógu sa načítavajú";
  const [view, setView] = useState<View>({ kind: "folder", id: -1, title: "Načítavam katalóg…" });
  const [query, setQuery] = useState("");
  const [queryDebounced, setQueryDebounced] = useState("");
  const [savedSorts, setSavedSorts] = useState(() => readStored<SavedViewSorts>("ddreams.sorts", {}));
  const [zoom, setZoom] = useState(168);
  const [centerMode,setCenterMode]=useState<"grid"|"map">(()=>{const saved=readStored<string>("ddreams.centerMode","grid");return saved==="map"?"map":"grid"});
  useEffect(()=>writeStored("ddreams.centerMode",centerMode),[centerMode]);
  const [dockLayout, setDockLayout] = useDockLayout();
  const dockHosts = useDockHosts();
  const [dockPresence, setDockPresence] = useDockPresence();
  const { dragging: dockDragging, controls: dockControls } = useDockDrag();
  const [overlayMode, setOverlayMode] = useState<0|2>(()=>readStored<number>("ddreams.overlay",2)===0?0:2);
  const [hoverMode, setHoverMode] = useState<"off"|"hover"|"shift">(()=>readStored("ddreams.hover", "shift"));
  const [thumbnailInfo, setThumbnailInfo] = useState<ThumbnailInfoConfig>(()=>normalizeThumbnailInfo(readStored("ddreams.thumbnailInfo",DEFAULT_THUMBNAIL_INFO)));
  const [thumbnailInfoOpen, setThumbnailInfoOpen] = useState(false);
  const [filters, setFilters] = useState<ViewFilters>({});
  const [showOtherFiles,setShowOtherFiles]=useState(()=>readStored("ddreams.showOtherFiles",false));
  const [seriesPreset,setSeriesPreset]=useState<SeriesPreset>(()=>{const saved=readStored<string>("ddreams.seriesPreset","normal");return saved==="strict"||saved==="loose"?saved:"normal"});
  const [seriesBusy,setSeriesBusy]=useState(false);
  useEffect(()=>writeStored("ddreams.seriesPreset",seriesPreset),[seriesPreset]);
  useEffect(()=>writeStored("ddreams.showOtherFiles",showOtherFiles),[showOtherFiles]);
  const [uiScale,setUiScale]=useState(()=>readStored("ddreams.uiScale",1));
  useEffect(()=>writeStored("ddreams.uiScale",uiScale),[uiScale]);
  // What the backend gets: by default only photos/RAW/video ("media"); "all" from Filtre shows everything.
  const queryFilters = useMemo(() => effectiveViewFilters(filters, showOtherFiles), [filters, showOtherFiles]);
  const [settingsOpen,setSettingsOpen]=useState(false);
  const [incomingFolder, setIncomingFolder] = useState<string | null>(null);
  useEffect(() => {
    void invoke<string>("get_incoming_folder").then(setIncomingFolder).catch(error => {
      console.warn("Priečinok s novými fotkami", error);
      setIncomingFolder("");
    });
  }, []);
  const [deleteConfirm,setDeleteConfirm]=useState<{ids:number[];name:string|null;permanent:boolean;quarantine?:boolean;info:{count:number;names:string[];sidecars:number;permanentCount:number;noBinDrives:string[]}|null}|null>(null);
  const [quarantineSettings, setQuarantineSettings] = useState<QuarantineSettings | null>(null);
  const [quarantineReview, setQuarantineReview] = useState<Review | null>(null);
  const [reviewInGrid, setReviewInGrid] = useState(false);
  const [quarantineBusy, setQuarantineBusy] = useState(false);
  const quarantineRunning = useRef(false);
  const [quarantineConfirm, setQuarantineConfirm] = useState<{ ids: number[]; action: "move" | "restore"; suggestionsOnly?: boolean } | null>(null);
  const [quarantineFailures, setQuarantineFailures] = useState<string[]>([]);
  const [excludeConfirm,setExcludeConfirm]=useState<{path:string;impact:ExclusionImpact}|null>(null);
  const [removeCatalogConfirm,setRemoveCatalogConfirm]=useState<{path:string;info:RemoveCatalogInfo}|null>(null);
  const [includeChildren, setIncludeChildren] = useState<boolean>(()=>readStored("ddreams.includeChildren",true));
  const [availableLabels,setAvailableLabels]=useState<ValueCount[]>([]);
  const [history, setHistory] = useState<View[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [epoch, setEpoch] = useState(1);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [storedPrimaryId, setPrimaryId] = useState<number | null>(null);
  const [storedPrimaryAsset, setPrimaryAsset] = useState<Asset | null>(null);
  const primaryId = storedPrimaryId != null && selectedIds.has(storedPrimaryId) ? storedPrimaryId : null;
  const primaryAsset = primaryId != null && storedPrimaryAsset?.id === primaryId ? storedPrimaryAsset : null;
  const [total, setTotal] = useState(0);
  const [totalSize, setTotalSize] = useState(0);
  const [toast,setToast]=useState("");
  const [recentChanges,setRecentChanges]=useState<RecentChange[]>([]);
  const [historyOpen,setHistoryOpen]=useState(false);
  const historyRef=useRef<HTMLDivElement>(null);
  const [aiStatus,setAiStatus]=useState<AiStatus>({available:false,reason:"AI modul nie je nainštalovaný",state:"off",modelState:"unloaded",waiting:0,indexed:0,queued:0,done:0,failed:0,offline:0,total:0,ratePerS:0,etaS:null,lastError:null});
  const [aiEnabled,setAiEnabled]=useState(()=>readStored("ddreams.aiEnabled",true));
  const [aiLoadModelAtStart,setAiLoadModelAtStart]=useState(()=>readStored("ddreams.aiLoadModelAtStart",true));
  const [aiOpen,setAiOpen]=useState(false);
  
  const [aiQuery,setAiQuery]=useState("");
  const [aiSearchBusy,setAiSearchBusy]=useState(false);
  const aiRef=useRef<HTMLDivElement>(null);
  const [aiSearchTarget,setAiSearchTarget]=useState<HTMLElement|null>(null);
  const aiReady=aiStatus.available&&aiEnabled&&(aiStatus.state==="ready"||aiStatus.state==="indexing"||aiStatus.state==="paused");
  const [refreshTick,setRefreshTick]=useState(0);
  const [deletionSuggestions, setDeletionSuggestions] = useState<Set<number>>(() => new Set());
  useEffect(() => {
    let active = true;
    void invoke<number[]>("get_deletion_suggestions").then(ids => {
      if (active) setDeletionSuggestions(new Set(ids));
    }).catch(error => console.warn("Návrhy na vymazanie", error));
    return () => { active = false; };
  }, [refreshTick]);
  const [treeRefreshTick,setTreeRefreshTick]=useState(0);
  const [treeRefreshPaths,setTreeRefreshPaths]=useState<string[]>([]);
  const [fileOp,setFileOp]=useState<{ids:number[];mode:"move"|"copy"}|null>(null);
  const [nameOp,setNameOp]=useState<{kind:"rename";assetId:number;name:string;extension:string}|{kind:"folder";parentPath:string}|null>(null);
  const [fileConflict,setFileConflict]=useState<{ids:number[];mode:"move"|"copy";path:string;count:number}|null>(null);
  const [moveWarning, setMoveWarning] = useState<{
    ids: number[]; path: string; conflict: "skip" | "keep_both" | "cancel"; count: number;
  } | null>(null);
  const [cutIds,setCutIds]=useState<number[]>([]);
  const [fileClipboard,setFileClipboard]=useState<{ids:number[];mode:"move"|"copy"}|null>(null);
  const [folderClipboard,setFolderClipboard]=useState<{path:string;mode:"move"|"copy"}|null>(null);
  const [folderRenameOp,setFolderRenameOp]=useState<{path:string;name:string}|null>(null);
  const [folderDeleteConfirm,setFolderDeleteConfirm]=useState<{path:string;info:{files:number;folders:number;catalogued:number;bytes:number;recycleAvailable:boolean}}|null>(null);
  const [operationProgress,setOperationProgress]=useState("");
  const [viewer, setViewer] = useState<{ asset: Asset; index: number } | null>(null);
  const currentViewer = useRef(viewer);
  const viewerValuePatches = useRef(new Map<number, AssetValuePatch>());
  currentViewer.current = viewer;
  // Returning from the full-screen viewer: show and focus the last viewed photo in the grid.
  const lastViewerIndex = useRef<number | null>(null);
  useEffect(() => {
    if (viewer) { lastViewerIndex.current = viewer.index; return; }
    const index = lastViewerIndex.current;
    lastViewerIndex.current = null;
    if (index != null) requestAnimationFrame(() => grid.current?.reveal(index));
  }, [viewer]);
  const grid = useRef<AssetGridHandle>(null);
  const valueWrites = useRef(Promise.resolve());
  // A photo that just received a catalogue ID is still the same selection.
  const valueAliases = useRef(new Map<number, number>());
  const [valueTokens, setValueTokens] = useState({ rating: 0, label: 0, special: 0 });
  const [primaryValueEdit, setPrimaryValueEdit] = useState<{ patch: AssetValuePatch; refreshTick: number }>();
  const valueContext = useRef({ primaryId, refreshTick, historyOpen });
  valueContext.current = { primaryId, refreshTick, historyOpen };
  const [slideshow, setSlideshow] = useState(false);
  const [slideshowSeconds, setSlideshowSeconds] = useState(5);
  const [viewerFit,setViewerFit]=useState(true);
  const [viewerZoom,setViewerZoom]=useState(1);
  const [viewerPan,setViewerPan]=useState({x:0,y:0});
  const [viewerLock,setViewerLock]=useState(false);
  const [viewerDimensions,setViewerDimensions]=useState<{width:number;height:number}|null>(null);
  const [viewerPreviewSource,setViewerPreviewSource]=useState<string|null>(null);
  const [viewerBroken,setViewerBroken]=useState(false);
  const [formats,setFormats]=useState<FormatTable>({version:0,formats:[]});
  const [viewerCanvasSize,setViewerCanvasSize]=useState({width:0,height:0});
  const viewerCanvasRef=useRef<HTMLDivElement>(null);
  const viewerOverlayRef = useRef<HTMLDivElement>(null);
  const viewerOpen = viewer !== null;
  useEffect(() => {
    if (!viewerOpen) return;
    const priorFocus = document.activeElement;
    viewerOverlayRef.current?.focus({ preventScroll: true });
    return () => {
      viewerValuePatches.current.clear();
      if (priorFocus instanceof HTMLElement && priorFocus.isConnected) priorFocus.focus({ preventScroll: true });
    };
  }, [viewerOpen]);

  useEffect(() => { const timer = window.setTimeout(() => setQueryDebounced(query), 180); return () => window.clearTimeout(timer); }, [query]);
  useEffect(() => { setZoom(readStored("ddreams.zoom",168)); const path=readStored("ddreams.lastFolder",DEFAULT_PATH); void invoke<Folder | null>("find_folder_path", { path }).then(folder => { const v=folder?{kind:"folder" as const,id:folder.id,ids:folder.folderIds,title:folder.path??folder.name,folderPath:folder.path??undefined}:{kind:"folder" as const,id:-1,title:DEFAULT_PATH,folderPath:DEFAULT_PATH};viewResolved.current=performance.now();void invoke("startup_mark",{label:"initial folder resolved"}).catch(()=>{});setView(v);setHistory([v]);setHistoryIndex(0); }).catch(() => { viewResolved.current=performance.now(); const v={kind:"folder" as const,id:-1,title:DEFAULT_PATH,folderPath:DEFAULT_PATH};setView(v);setHistory([v]);setHistoryIndex(0); }); }, []);
  useEffect(()=>{void invoke<FormatTable>("get_formats").then(setFormats).catch(error=>console.error("Format table failed to load",error));},[]);
  useEffect(() => {
    let active = true;
    void invoke<ValueCount[]>("get_catalog_values", { kind: "label", offset: 0, limit: 500 })
      .then(labels => { if (active) setAvailableLabels(labels); })
      .catch(() => { if (active) setAvailableLabels([]); });
    return () => { active = false; };
  }, [refreshTick, valueTokens.label]);
  useEffect(() => writeStored("ddreams.zoom",zoom),[zoom]);
  useEffect(() => writeStored("ddreams.overlay",overlayMode),[overlayMode]);
  useEffect(() => writeStored("ddreams.hover",hoverMode),[hoverMode]);
  useEffect(() => writeStored("ddreams.thumbnailInfo",thumbnailInfo),[thumbnailInfo]);
  useEffect(() => writeStored("ddreams.includeChildren",includeChildren),[includeChildren]);
  useEffect(()=>writeStored("ddreams.aiEnabled",aiEnabled),[aiEnabled]);
  useEffect(()=>writeStored("ddreams.aiLoadModelAtStart",aiLoadModelAtStart),[aiLoadModelAtStart]);
  const selectedList=useMemo(()=>[...selectedIds],[selectedIds]);
  const selectedReadIds = useMemo(() => selectedList.map(id => valueAliases.current.get(id) ?? id), [selectedList]);
  const primaryReadId = primaryId == null ? null : valueAliases.current.get(primaryId) ?? primaryId;
  const viewerReadId = viewer ? valueAliases.current.get(viewer.asset.id) ?? viewer.asset.id : null;
  const [aiTick,setAiTick]=useState(0);
  useEffect(()=>{if(aiStatus.state!=="indexing")return;const timer=window.setInterval(()=>setAiTick(v=>v+1),20000);return()=>window.clearInterval(timer)},[aiStatus.state]);
  useEffect(()=>{if(view.kind!=="ai"||!view.title.startsWith("AI:"))return;const base=view.title.replace(/ · prehľadané [\d\s\u00a0]+ z [\d\s\u00a0]+$/u,"");const suffix=aiStatus.indexed<aiStatus.total?` · prehľadané ${aiStatus.indexed.toLocaleString("sk-SK")} z ${aiStatus.total.toLocaleString("sk-SK")}`:"";const title=base+suffix;if(title!==view.title)setView(current=>current===view?{...current,title}:current)},[aiStatus.indexed,aiStatus.total,view]);
  useEffect(()=>setAiSearchTarget(document.querySelector<HTMLElement>(".toolbar-left")),[]);
  useEffect(()=>{let active=true;let unlisten:(()=>void)|undefined;void listen<AiStatus>("ai-status",event=>{if(active)setAiStatus(event.payload)}).then(fn=>{if(active)unlisten=fn;else fn()});void invoke<AiStatus>("ai_status").then(status=>{if(!active)return;setAiStatus(status);}).catch(()=>{});return()=>{active=false;unlisten?.()}},[]);
  const sortKey = sortKeyForView(view);
  const {sort, descending} = readViewSort(savedSorts, sortKey);
  const editPendingView = view.kind === "special" && view.value === "edit_pending";
  const changeSort = (nextSort: string, nextDescending: boolean) => {
    const saved = updateViewSort(savedSorts, sortKey, nextSort, nextDescending);
    setSavedSorts(saved);
    writeStored("ddreams.sorts", saved);
  };
  const assetFilter = useMemo(() => buildAssetFilter(view, queryDebounced, sort, descending, queryFilters, includeChildren),
    [view, queryDebounced, sort, descending, queryFilters, includeChildren]);
  const currentAssetFilter = useRef(assetFilter);
  currentAssetFilter.current = assetFilter;
  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (!ready || !appInfo) return;
    let active = true;
    const showError = (error: unknown) => {
      if (active) setToast(String(error));
    };
    void invoke<QuarantineSettings>("get_quarantine_settings").then(settings => {
      if (active) setQuarantineSettings(settings);
    }).catch(showError);
    if (!appInfo.readonly) {
      void invoke<Review>("preview_quarantine_review").then(review => {
        if (active && (review.moves.ids.length || review.deletes.ids.length || review.staleEdits.ids.length)) setQuarantineReview(review);
      }).catch(showError);
    }
    return () => { active = false; };
  }, [ready, appInfo]);
  const viewResolved = useRef(0);
  const markReady = useCallback((why: string) => { setReady(was => { if (!was) void invoke("startup_mark", { label: `ready (${why})` }).catch(() => {}); return true; }); }, []);
  const counts = useCallback((n: number, size: number) => { setTotal(n); setTotalSize(size); if (viewResolved.current) markReady("first page"); }, [markReady]);
  useEffect(() => { void invoke("startup_mark", { label: "ui mounted" }).catch(() => {}); const timer = window.setTimeout(() => markReady("timeout 20 s"), 20000); return () => window.clearTimeout(timer); }, [markReady]);
  useEffect(()=>{if(!ready||!aiEnabled)return;const timer=window.setTimeout(()=>{void invoke<AiStatus>("ai_status").then(status=>{if(status.available&&status.state==="off")void invoke<AiStatus>("ai_start",{lazyModel:!aiLoadModelAtStart}).then(setAiStatus).catch(error=>setAiStatus(s=>({...s,state:"error",lastError:String(error)})));}).catch(()=>{});},4000);return()=>window.clearTimeout(timer);},[ready]);
  const navigate = useCallback((next: View) => { setView(next);setHistory(h=>{const base=h.slice(0,historyIndex+1);base.push(next);setHistoryIndex(base.length-1);return base;}); setQuery(""); setQueryDebounced(""); setFilters({});setSelectedIds(new Set()); setPrimaryId(null); setPrimaryAsset(null); setViewer(null);if(next.kind==="folder")writeStored("ddreams.lastFolder",next.folderPath??next.title); }, [historyIndex]);
  const openMapNeighborhood=useCallback(async(ids:number[])=>{if(!ids.length)return;const next:View={kind:"people",ids,title:"Okolie na mape"};navigate(next);try{const page=await invoke<Page>("get_assets",{filter:{...buildAssetFilter(next, "", "name", false, effectiveViewFilters({}, showOtherFiles), false), offset:0, limit:1}});const asset=page.items[0];if(asset){setViewer({asset,index:0});setPrimaryId(asset.id);setPrimaryAsset(asset);setSelectedIds(new Set([asset.id]));setViewerDimensions(null)}}catch(error){setToast(String(error))}},[navigate,showOtherFiles]);
  const travel = useCallback((index:number)=>{const next=history[index];if(!next)return;setHistoryIndex(index);setView(next);setQuery("");setFilters({});setSelectedIds(new Set());setPrimaryId(null);setPrimaryAsset(null);setViewer(null);},[history]);
  const select = useCallback((asset: Asset, _index: number, event: ReactMouseEvent | ReactKeyboardEvent) => {
    if (event.ctrlKey || event.metaKey) {
      const next = new Set(selectedIds);
      if (next.has(asset.id)) {
        next.delete(asset.id);
        if (primaryId === asset.id || !next.size) { setPrimaryId(null); setPrimaryAsset(null); }
      } else {
        next.add(asset.id); setPrimaryId(asset.id); setPrimaryAsset(asset);
      }
      setSelectedIds(next);
    } else if (!event.shiftKey) {
      setSelectedIds(new Set([asset.id])); setPrimaryId(asset.id); setPrimaryAsset(asset);
    }
  }, [selectedIds, primaryId]);
  const selectRange = useCallback((ids: number[], asset: Asset, _index: number, add = false) => { setSelectedIds(current => new Set(add ? [...current, ...ids] : ids)); setPrimaryId(asset.id); setPrimaryAsset(asset); }, []);
  const selectAll = useCallback(async (ids: number[]) => { setSelectedIds(new Set(ids)); if (!ids.length) {setPrimaryId(null);setPrimaryAsset(null);return;} const wanted=new Set(ids);const page = await invoke<Page>("get_assets", { filter: {...assetFilter, offset:0, limit:500} }); const asset=page.items.find(a=>wanted.has(a.id));if(asset){setPrimaryId(asset.id);setPrimaryAsset(asset);} }, [assetFilter]);
  const epochCounter = useRef(epoch);
  const updateViewport = useCallback(() => {
    const next = ++epochCounter.current;
    setEpoch(next);
    void invoke("set_thumbnail_epoch", { epoch: next });
  }, []);
  const viewportView = useRef<View | null>(null);
  useEffect(() => {
    if (viewportView.current === view) return;
    viewportView.current = view;
    updateViewport();
  }, [view, updateViewport]);
  const watchedFolderId = view.kind === "folder" ? view.id : null;
  const watchedFolder = useRef<{ id: number; path?: string } | null>(null);
  useLayoutEffect(() => {
    watchedFolder.current = watchedFolderId == null ? null : { id: watchedFolderId, path: view.folderPath };
    return () => { watchedFolder.current = null; };
  }, [watchedFolderId, view.folderPath]);
  useEffect(() => {
    let pending = false;
    const check = async () => {
      const folder = watchedFolder.current;
      if (!folder || pending || document.visibilityState !== "visible") return;
      pending = true;
      try {
        const changed = await invoke<boolean>("check_folder_changed", { folderId: folder.id });
        const current = watchedFolder.current;
        if (changed && current?.id === folder.id && current.path === folder.path) {
          await grid.current?.reloadListing();
        }
      } catch (error) {
        console.warn("Kontrola zmien priečinka zlyhala", error);
      } finally {
        pending = false;
      }
    };
    const interval = window.setInterval(check, 3000);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
    };
  }, []);
  const up = useCallback(async()=>{if(view.kind!=="folder")return;const p=view.folderPath??view.title;const parts=p.split(/[\\/]/).filter(Boolean);if(parts.length<=1)return;parts.pop();let parent=parts.join("\\");if(/^[A-Za-z]:$/.test(parent))parent+="\\";const f=await invoke<Folder|null>("find_folder_path",{path:parent}).catch(()=>null);if(f)navigate({kind:"folder",id:f.id,ids:f.folderIds,title:f.path??f.name,folderPath:f.path??undefined});},[view,navigate]);
  const refresh = useCallback(()=>{if(view.kind==="folder"&&view.id!=null)void invoke("refresh_folder_listing",{folderId:view.id});setRefreshTick(v=>v+1);setTreeRefreshPaths([]);setTreeRefreshTick(v=>v+1);updateViewport();},[view,updateViewport]);
  const loadRecentChanges=useCallback(()=>{void invoke<RecentChange[]>("get_recent_changes",{limit:20}).then(setRecentChanges).catch(()=>setRecentChanges([]));},[]);
  useEffect(()=>{loadRecentChanges()},[loadRecentChanges]);
  const refreshAfterWrite=useCallback((refreshTree=false,affectedPaths:string[]=[] )=>{if(view.kind==="folder"&&view.id!=null)void invoke("refresh_folder_listing",{folderId:view.id});setRefreshTick(v=>v+1);if(refreshTree){setTreeRefreshPaths(affectedPaths);setTreeRefreshTick(v=>v+1);}updateViewport();loadRecentChanges();},[view,updateViewport,loadRecentChanges]);
  const showDeletionChange = useCallback((change: DeletionSuggestionChange) => {
    setToast(change.summary);
    if (change.changeId != null) refreshAfterWrite();
    window.setTimeout(() => setToast(""), 6000);
  }, [refreshAfterWrite]);
  useEffect(() => {
    const subscription = listen<DeletionSuggestionChange>("deletion-suggestions-changed", event => {
      showDeletionChange(event.payload);
    });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [showDeletionChange]);
  const runDeletionAction = async (command: "analyze_incoming_series" | "keep_suggested_assets", assetIds?: number[]) => {
    if (appInfo?.readonly) return;
    try {
      const args = command === "keep_suggested_assets" ? {assetIds:assetIds ?? null} : undefined;
      showDeletionChange(await invoke<DeletionSuggestionChange>(command, args));
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 4000);
    }
  };
  const setAssetValues = useCallback((ids: number[], changes: Record<string, unknown>) => {
    if (appInfo?.readonly) { setToast("Katalóg je iba na čítanie"); window.setTimeout(() => setToast(""), 2200); return; }
    if (!ids.length) return;
    const ratingEdited = "rating" in changes, labelEdited = "label" in changes;
    const taggedEdited = "tagged" in changes, pendingEdited = "editPending" in changes;
    const write = async () => {
      try {
        const result = await invoke<{
          changeId: number | null;
          summary: string;
          remappedIds: { oldId: number; newId: number }[];
          assets: { id: number; rating: number | null; label: string | null; tagged: boolean; editPendingSince: number | null }[];
        }>("set_asset_values", { assetIds: ids, changes });
        const patches = new Map<number, AssetValuePatch>(result.assets.map(asset => [asset.id, {
          id: asset.id, rating: asset.rating, label: asset.label, tagged: Number(asset.tagged),
          ...(pendingEdited ? { editPendingSince: asset.editPendingSince } : {}),
        }]));
        const remap = new Map(result.remappedIds.map(({ oldId, newId }) => [oldId, newId]));
        for (const [oldId, newId] of remap) {
          const patch = patches.get(newId);
          if (patch) patches.set(oldId, patch);
        }
        if (remap.size) {
          for (const [oldId, newId] of remap) valueAliases.current.set(newId, oldId);
          setSelectedIds(previous => [...previous].some(id => remap.has(id))
            ? new Set([...previous].map(id => remap.get(id) ?? id)) : previous);
          setPrimaryId(id => id == null ? id : remap.get(id) ?? id);
        }
        const patchAsset = (asset: Asset) => {
          const patch = patches.get(asset.id);
          return patch ? { ...asset, ...patch } : asset;
        };
        if (currentViewer.current) {
          for (const [id, patch] of patches) {
            viewerValuePatches.current.set(id, { ...viewerValuePatches.current.get(id), ...patch });
          }
        }
        setPrimaryAsset(previous => previous && patchAsset(previous));
        setViewer(previous => {
          if (!previous) return previous;
          const asset = patchAsset(previous.asset);
          return asset === previous.asset ? previous : { ...previous, asset };
        });
        const context = valueContext.current;
        const patch = context.primaryId == null ? undefined : patches.get(context.primaryId);
        if (patch) {
          setPrimaryValueEdit(previous => ({
            patch: {
              ...(previous?.refreshTick === context.refreshTick && previous.patch.id === patch.id ? previous.patch : {}),
              ...patch,
            },
            refreshTick: context.refreshTick,
          }));
        }
        if (result.changeId != null) {
          const filter = currentAssetFilter.current;
          const reloadListing =
            (ratingEdited && (filter.kind === "rating" || !!filter.ratingMode || filter.sort === "rating")) ||
            (labelEdited && (filter.kind === "label" || !!filter.labelFilter || filter.sort === "label")) ||
            (taggedEdited && (filter.taggedOnly || (filter.kind === "special" && filter.value === "tagged"))) ||
            (pendingEdited && filter.kind === "special" && filter.value === "edit_pending");
          grid.current?.applyValues(patches, reloadListing);
          setValueTokens(tokens => ({
            rating: tokens.rating + Number(ratingEdited || !!remap.size),
            label: tokens.label + Number(labelEdited || !!remap.size),
            special: tokens.special + Number(taggedEdited || pendingEdited || !!remap.size),
          }));
          if (context.historyOpen) loadRecentChanges();
        } else {
          grid.current?.applyValues(patches, false);
        }
        setToast(result.summary);
        window.setTimeout(() => setToast(""), 6000);
      } catch (error) {
        const message = String(error);
        setToast(message.toLowerCase().includes("read-only") ? "Katalóg je iba na čítanie" : message);
        window.setTimeout(() => setToast(""), 3000);
      }
    };
    valueWrites.current = valueWrites.current.then(write);
    return valueWrites.current;
  }, [appInfo?.readonly, loadRecentChanges]);
  const setAssetColors=useCallback(async(ids:number[],color:string,state:boolean|null)=>{if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");return;}try{const count=await invoke<number>("set_asset_colors",{assetIds:ids,color,state});if(count){setToast(`Farba ${color} opravená`);refreshAfterWrite();window.setTimeout(()=>setRefreshTick(v=>v+1),2400);window.setTimeout(()=>setToast(""),6000)}}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),3500)}},[appInfo,refreshAfterWrite]);
  const setAssetCategories=useCallback(async(ids:number[],categoryId:number,assign:boolean):Promise<boolean>=>{
    if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");window.setTimeout(()=>setToast(""),2200);return false;}
    if(!ids.length)return false;
    try{
      const result=await invoke<{changeId:number|null;summary:string;changedCount:number;remappedIds:{oldId:number;newId:number}[]}>("set_asset_categories",{assetIds:ids,categoryId,assign});
      const remap=new Map(result.remappedIds.map(item=>[item.oldId,item.newId]));
      setSelectedIds(previous=>new Set([...previous].map(id=>remap.get(id)??id)));
      const nextPrimary=primaryId==null?null:remap.get(primaryId)??primaryId;if(nextPrimary!=null){setPrimaryId(nextPrimary);setPrimaryAsset(previous=>previous?{...previous,id:nextPrimary}:previous);}
      setToast(result.summary);refreshAfterWrite();window.setTimeout(()=>setToast(""),6000);return true;
    }catch(error){const message=String(error);setToast(message.toLowerCase().includes("read-only")?"Katalóg je iba na čítanie":message);window.setTimeout(()=>setToast(""),3000);return false;}
  },[appInfo,primaryId,refreshAfterWrite]);
  const keywordStructureChanged=useCallback((result:KeywordMutation,deleted:boolean)=>{setToast(result.summary);refreshAfterWrite();if(deleted&&view.kind==="keyword"&&view.id!=null&&result.deletedKeywordIds.includes(view.id))navigate({kind:"special",value:"all_photos",title:"Všetky fotky"});window.setTimeout(()=>setToast(""),6000);},[refreshAfterWrite,view,navigate]);
  const setAssetKeywords=useCallback(async(ids:number[],keywordId:number,assign:boolean):Promise<boolean>=>{
    if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");window.setTimeout(()=>setToast(""),2200);return false;}if(!ids.length)return false;
    try{const result=await invoke<{changeId:number|null;summary:string;changedCount:number;remappedIds:{oldId:number;newId:number}[]}>("set_asset_keywords",{assetIds:ids,keywordId,assign});const remap=new Map(result.remappedIds.map(item=>[item.oldId,item.newId]));setSelectedIds(previous=>new Set([...previous].map(id=>remap.get(id)??id)));const nextPrimary=primaryId==null?null:remap.get(primaryId)??primaryId;if(nextPrimary!=null){setPrimaryId(nextPrimary);setPrimaryAsset(previous=>previous?{...previous,id:nextPrimary}:previous);}setToast(result.summary);refreshAfterWrite();window.setTimeout(()=>setToast(""),6000);return result.changedCount>0;}catch(error){const message=String(error);setToast(message.toLowerCase().includes("read-only")||appInfo?.readonly?"Katalóg je iba na čítanie":message);window.setTimeout(()=>setToast(""),3000);return false;}
  },[appInfo,primaryId,refreshAfterWrite]);
  const createAndAssignKeyword=useCallback(async(ids:number[],parentId:number|null,name:string)=>{
    if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");window.setTimeout(()=>setToast(""),2200);return;}if(!ids.length)return;
    try{const result=await invoke<{changeId:number|null;summary:string;changedCount:number;remappedIds:{oldId:number;newId:number}[];keywordId:number|null;parentId:number|null;ancestorIds:number[]}>("create_and_assign_keyword",{assetIds:ids,parentId,name});const remap=new Map(result.remappedIds.map(item=>[item.oldId,item.newId]));setSelectedIds(previous=>new Set([...previous].map(id=>remap.get(id)??id)));const nextPrimary=primaryId==null?null:remap.get(primaryId)??primaryId;if(nextPrimary!=null){setPrimaryId(nextPrimary);setPrimaryAsset(previous=>previous?{...previous,id:nextPrimary}:previous);}if(result.keywordId!=null)keywordStructureChanged({changeId:result.changeId,summary:result.summary,keywordId:result.keywordId,parentId:result.parentId,ancestorIds:result.ancestorIds,deletedKeywordIds:[]},false);}catch(error){const message=String(error);setToast(message.toLowerCase().includes("read-only")||appInfo?.readonly?"Katalóg je iba na čítanie":message);window.setTimeout(()=>setToast(""),3500);}
  },[appInfo,primaryId,keywordStructureChanged]);
  const categoryStructureChanged=useCallback((result:CategoryMutation,deleted:boolean)=>{
    setToast(result.summary);refreshAfterWrite();
    if(deleted&&view.kind==="category"&&((view.id!=null&&result.deletedCategoryIds.includes(view.id))||(view.rootId!=null&&result.deletedRootIds.includes(view.rootId))))navigate({kind:"special",value:"all_photos",title:"Všetky fotky"});
    window.setTimeout(()=>setToast(""),6000);
  },[refreshAfterWrite,view,navigate]);
  const categoryWriteError=useCallback((error:unknown)=>{const message=String(error);setToast(appInfo?.readonly||message.toLowerCase().includes("read-only")?"Katalóg je iba na čítanie":message);window.setTimeout(()=>setToast(""),4000);},[appInfo]);
  const [applyingSuggestion, setApplyingSuggestion] = useState(false);
  const currentView = useRef(view);
  currentView.current = view;
  const applySuggestion = async (ids: number[]) => {
    const suggestion = view.suggestion;
    if (!suggestion || !ids.length || applyingSuggestion) return;
    setApplyingSuggestion(true);
    try {
      const saved = suggestion.kind === "category"
        ? await setAssetCategories(ids, suggestion.labelId, true)
        : await setAssetKeywords(ids, suggestion.labelId, true);
      if (saved && currentView.current === view) {
        const applied = new Set(ids);
        navigate({...view, ids: (view.ids ?? []).filter(id => !applied.has(id))});
      }
    } finally {
      setApplyingSuggestion(false);
    }
  };
  const selectTaggedInView=useCallback(async()=>{
    const filter = {...assetFilter, taggedOnly:true};
    const first=await invoke<Page>("get_assets",{filter:{...filter,offset:0,limit:1}});const ids:number[]=[];
    for(let offset=0;offset<first.total;offset+=500){const page=await invoke<Page>("get_assets",{filter:{...filter,offset,limit:500}});ids.push(...page.items.filter(asset=>!asset.isDirectory).map(asset=>asset.id));if(page.items.length<500)break;}
    setSelectedIds(new Set(ids));if(ids.length){const page=await invoke<Page>("get_assets",{filter:{...filter,offset:0,limit:500}});const asset=page.items.find(item=>item.id===ids[0]);if(asset){setPrimaryId(asset.id);setPrimaryAsset(asset);}}
  },[assetFilter]);
  const copyCurrentResult=useCallback(async()=>{let lines:string[]=[];for(let offset=0;offset<total;offset+=500){const p=await invoke<Page>("get_assets",{filter:{...assetFilter,offset,limit:500}});for(const a of p.items)if(!a.isDirectory)lines.push([a.name,a.size??"",a.ext,a.fileModified??"",a.width&&a.height?`${a.width} × ${a.height}`:"",a.caption??"",a.rating??""].join("\t"));if(p.items.length<500)break;}await navigator.clipboard.writeText(lines.join("\n"));setToast(`Skopírované: ${slovakItems(lines.length)}`);window.setTimeout(()=>setToast(""),1600);},[assetFilter,total]);
  const showViewerAsset = useCallback((asset: Asset, index: number) => {
    const patch = viewerValuePatches.current.get(asset.id);
    const shown = patch ? { ...asset, ...patch } : asset;
    const next = { asset: shown, index };
    currentViewer.current = next;
    setViewer(next);
    setViewerDimensions(null);
    setPrimaryId(shown.id);
    setPrimaryAsset(shown);
    setSelectedIds(new Set([shown.id]));
    if (!viewerLock) {
      setViewerFit(true);
      setViewerZoom(1);
      setViewerPan({ x: 0, y: 0 });
    }
  }, [viewerLock]);

  const advanceAfterRemoval = useCallback(async (removedIds: number[]) => {
    const shown = currentViewer.current;
    if (!shown || !removedIds.includes(shown.asset.id)) return;
    const filter = currentAssetFilter.current;
    const stillShowing = () => currentViewer.current?.asset.id === shown.asset.id && currentAssetFilter.current === filter;
    const canShow = (asset: Asset) => !removedIds.includes(asset.id) && !asset.isDirectory
      && !asset.offline && !asset.missing && !!asset.path && isImage(asset, formats);
    const pageSize = 60;
    try {
      let total = 0;
      for (let offset = shown.index; ; offset += pageSize) {
        const page = await invoke<Page>("get_assets", { filter: { ...filter, offset, limit: pageSize } });
        if (!stillShowing()) return;
        total = page.total;
        const index = page.items.findIndex(canShow);
        if (index !== -1) { showViewerAsset(page.items[index], offset + index); return; }
        if (offset + pageSize >= total) break;
      }
      for (let end = Math.min(shown.index, total); end > 0;) {
        const offset = Math.max(0, end - pageSize);
        const page = await invoke<Page>("get_assets", { filter: { ...filter, offset, limit: end - offset } });
        if (!stillShowing()) return;
        for (let index = page.items.length - 1; index >= 0; index--) {
          if (canShow(page.items[index])) { showViewerAsset(page.items[index], offset + index); return; }
        }
        end = offset;
      }
    } finally {
      if (stillShowing()) {
        currentViewer.current = null;
        setViewer(null);
      }
    }
  }, [formats, showViewerAsset]);

  const requestQuarantineMove = (ids: number[]) => {
    if (!appInfo?.readonly && !quarantineBusy) setQuarantineConfirm({ ids, action: "move" });
  };
  const folderActionRef=useRef<((action:"rename"|"delete",path:string)=>Promise<void>)|null>(null);
  const runQuarantine = async (command: string, args: Record<string, unknown>) => {
    if (quarantineRunning.current) return;
    quarantineRunning.current = true;
    setQuarantineBusy(true);
    setOperationProgress("Spracúvam karanténu…");
    try {
      const result = await invoke<QuarantineOutcome>(command, args);
      const summary = [
        result.moved && `Presunuté do karantény: ${result.moved}`,
        result.restored && `Vrátené: ${result.restored}`,
        result.deleted && `Odstránené: ${result.deleted}`,
        result.failures.length && `Nedokončené položky: ${result.failures.length}`,
      ].filter(Boolean).join(" · ");
      setToast(summary || "Karanténa bez zmien");
      if (result.affectedFolderPaths.length) {
        setSelectedIds(new Set());
        setPrimaryId(null);
        setPrimaryAsset(null);
        refreshAfterWrite(true, result.affectedFolderPaths);
        if ((command === "move_to_quarantine" || command === "delete_from_quarantine") && Array.isArray(args.assetIds)) {
          const failures = new Set(result.failures.flatMap(error => {
            const match = /^Položka (-?\d+):/.exec(error);
            return match ? [Number(match[1])] : [];
          }));
          const successfulIds = (args.assetIds as number[]).filter(id => !failures.has(id));
          const count = command === "move_to_quarantine" ? result.moved : result.deleted;
          if (count === successfulIds.length) await advanceAfterRemoval(successfulIds);
          else if (args.assetIds.length === 1 && count === 1) await advanceAfterRemoval(args.assetIds as number[]);
        }
      }
      setQuarantineFailures(result.failures);
      window.setTimeout(() => setToast(""), 8000);
    } catch (error) {
      setToast(String(error));
      setQuarantineFailures([String(error)]);
    } finally {
      if (command === "execute_quarantine_review") {
        setQuarantineReview(null);
        setReviewInGrid(false);
      }
      setOperationProgress("");
      quarantineRunning.current = false;
      setQuarantineBusy(false);
    }
  };

  const requestQuarantineDelete = async (ids: number[]) => {
    if (!ids.length || quarantineBusy) return;
    try {
      const info = await invoke<NonNullable<NonNullable<typeof deleteConfirm>["info"]>>("get_quarantine_delete_info", { assetIds: ids });
      setDeleteConfirm({ ids, name: null, permanent: false, quarantine: true, info });
    } catch (error) { setToast(String(error)); }
  };

  const inspectQuarantineReview = (ids: number[], title: string) => {
    setCenterMode("grid");
    setViewer(null);
    navigate({ kind: "quarantine_review", ids, title });
    setReviewInGrid(true);
  };
  const beginFileOperation=useCallback((ids:number[],mode:"move"|"copy"|"rename"|"delete"|"delete-permanent",tileAsset?:Asset)=>{if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");return;}{const target=tileAsset??(ids.length===1&&primaryAsset?.id===ids[0]?primaryAsset:null);if(target?.isDirectory&&target.path&&(mode==="delete"||mode==="rename")){void folderActionRef.current?.(mode,target.path);return;}if(target?.isDirectory&&mode==="delete-permanent"){setToast("Priečinky sa dajú odstrániť len do Koša");window.setTimeout(()=>setToast(""),3000);return;}}if(mode==="delete"||mode==="delete-permanent"){if(!ids.length)return;const permanent=mode==="delete-permanent";void invoke<{count:number;names:string[];sidecars:number;permanentCount:number;noBinDrives:string[]}>("get_delete_info",{assetIds:ids,permanent}).then(info=>setDeleteConfirm({ids,name:ids.length===1?(tileAsset??primaryAsset)?.name??null:null,permanent,info})).catch(error=>{setToast(String(error));window.setTimeout(()=>setToast(""),4000)});return;}if(mode==="rename"){const asset=tileAsset??primaryAsset;if(!asset)return;const old=asset.name;const base=asset.ext&&old.toLowerCase().endsWith(`.${asset.ext.toLowerCase()}`)?old.slice(0,-asset.ext.length-1):old;setNameOp({kind:"rename",assetId:ids[0],name:base,extension:asset.ext});return;}setFileOp({ids,mode});},[appInfo,primaryAsset]);
  const runFileOperation=useCallback(async(ids:number[],mode:"move"|"copy",path:string,conflict:"skip"|"keep_both"|"cancel",confirmed=false)=>{
    try {
      if (mode === "move" && !confirmed) {
        const count = await invoke<number>("get_uncategorized_photos_for_move", {
          assetIds: ids, targetFolderPath: path, conflict,
        });
        if (count) {
          setFileConflict(null);
          setMoveWarning({ ids, path, conflict, count });
          return;
        }
      }
      const result=await invoke<any>(mode==="move"?"move_files":"copy_files",{assetIds:ids,targetFolderPath:path,conflict});setFileConflict(null);setOperationProgress("");setToast(result.summary);setCutIds([]);if(mode==="move")setFileClipboard(null);if(result.changedCount>0)refreshAfterWrite(true,result.affectedFolderPaths??[view.folderPath,path].filter(Boolean));if(mode==="move"&&result.changedCount>0&&view.kind==="folder"&&path.toLowerCase()!==(view.folderPath??"").toLowerCase()){setSelectedIds(prev=>new Set([...prev].filter(id=>!ids.includes(id))));if(primaryId!=null&&ids.includes(primaryId)){setPrimaryId(null);setPrimaryAsset(null)}}window.setTimeout(()=>setToast(""),result.changedCount?6000:2400);}catch(error){setOperationProgress("");setToast(String(error));window.setTimeout(()=>setToast(""),4000)}finally{setOperationProgress("")}},[refreshAfterWrite,view,primaryId]);
  const chooseOperationTarget=useCallback(async(path:string)=>{if(!fileOp)return;try{const count=await invoke<number>("get_file_operation_conflicts",{assetIds:fileOp.ids,targetFolderPath:path});const pending={...fileOp,path,count};setFileOp(null);if(count)setFileConflict(pending);else void runFileOperation(fileOp.ids,fileOp.mode,path,"cancel");}catch(error){setToast(String(error));setFileOp(null)}},[fileOp,runFileOperation]);
  const dropFiles=useCallback(async(ids:number[],path:string,copy:boolean)=>{if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");return;}const mode=copy?"copy":"move";try{const count=await invoke<number>("get_file_operation_conflicts",{assetIds:ids,targetFolderPath:path});if(count)setFileConflict({ids,mode,path,count});else void runFileOperation(ids,mode,path,"cancel");}catch(error){setToast(String(error));}},[appInfo,runFileOperation]);
  const folderAction=useCallback(async(action:"reveal"|"copyPath"|"rename"|"delete"|"cut"|"copy"|"paste",path:string)=>{if(action==="copyPath"){await navigator.clipboard.writeText(path);setToast("Cesta skopírovaná");window.setTimeout(()=>setToast(""),1800);return;}if(action==="reveal"){try{await invoke("reveal_in_explorer",{path})}catch(error){setToast(String(error))}return;}if(action==="cut"||action==="copy"){setFolderClipboard({path,mode:action==="cut"?"move":"copy"});setFileClipboard(null);setCutIds([]);return;}if(action==="delete"){try{const info=await invoke<{files:number;folders:number;catalogued:number;bytes:number;recycleAvailable:boolean}>("delete_folder_info",{path});if(!info.recycleAvailable){setToast("Na tomto disku nie je dostupný Kôš");return;}setFolderDeleteConfirm({path,info})}catch(error){setToast(String(error))}return;}if(action==="rename"){const name=path.split(/[\\/]/).filter(Boolean).pop()??path;setFolderRenameOp({path,name});return;}if(action==="paste"&&fileClipboard){void dropFiles(fileClipboard.ids,path,fileClipboard.mode==="copy");return;}if(action==="paste"&&folderClipboard){try{const result=await invoke<any>(folderClipboard.mode==="move"?"move_folder":"copy_folder",{path:folderClipboard.path,target:path});setToast(result.summary);if(folderClipboard.mode==="move"){const newPath=(result.affectedFolderPaths as string[]|undefined)?.[1]; if(newPath)window.dispatchEvent(new CustomEvent("ddreams-folder-path-changed",{detail:{oldPath:folderClipboard.path,newPath}}));setFolderClipboard(null);}refreshAfterWrite(true,result.affectedFolderPaths??[path,folderClipboard.path]);}catch(error){setToast(String(error))}}},[folderClipboard,fileClipboard,dropFiles,refreshAfterWrite]);
  folderActionRef.current=folderAction;
  const submitFolderRename=useCallback(async(name:string)=>{if(!folderRenameOp)return;const {path}=folderRenameOp;setFolderRenameOp(null);try{const result=await invoke<any>("rename_folder",{path,newName:name});setToast(result.summary);const newPath=result.newPath as string|undefined;if(newPath)window.dispatchEvent(new CustomEvent("ddreams-folder-path-changed",{detail:{oldPath:path,newPath}}));refreshAfterWrite(true,newPath?[path,newPath]:[path]);if(newPath&&view.kind==="folder"&&view.folderPath){const norm=(v:string)=>v.replace(/[\\/]+$/g,"").toLowerCase();const cur=norm(view.folderPath),old=norm(path);if(cur===old||cur.startsWith(old+"\\")){const moved=newPath+view.folderPath.slice(path.replace(/[\\/]+$/g,"").length);const folder=await invoke<Folder|null>("find_folder_path",{path:moved}).catch(()=>null);if(folder)navigate({kind:"folder",id:folder.id,ids:folder.folderIds,title:folder.path??folder.name,folderPath:folder.path??undefined});}}window.setTimeout(()=>setToast(""),6000)}catch(error){setToast(String(error))}},[folderRenameOp,refreshAfterWrite,view,navigate]);
  const confirmFolderDelete=useCallback(async()=>{if(!folderDeleteConfirm)return;const {path}=folderDeleteConfirm;setFolderDeleteConfirm(null);try{const result=await invoke<any>("delete_folder",{path});setToast(result.summary);refreshAfterWrite(true,result.affectedFolderPaths??[path]);const current=view.folderPath??"";const norm=(v:string)=>v.replace(/[\\/]+$/g,"").toLowerCase();if(current&&(norm(current)===norm(path)||norm(current).startsWith(norm(path)+"\\"))){const parent=folderParentPath(path);const folder=parent?await invoke<Folder|null>("find_folder_path",{path:parent}).catch(()=>null):null;if(folder)navigate({kind:"folder",id:folder.id,ids:folder.folderIds,title:folder.path??folder.name,folderPath:folder.path??undefined});else navigate({kind:"special",value:"all_photos",title:"Všetky fotky"})}window.setTimeout(()=>setToast(""),6000)}catch(error){setToast(String(error));}},[folderDeleteConfirm,refreshAfterWrite,view.folderPath,navigate]);
  const runDelete = useCallback(async (ids: number[], permanent: boolean, confirmedNoBinDrives: string[]) => {
    setDeleteConfirm(null);
    try {
      const result = await invoke<{ summary: string; affectedIds: number[]; affectedFolderPaths: string[] }>(
        "delete_files", { assetIds: ids, permanent, confirmedNoBinDrives },
      );
      setToast(result.summary);
      const removed = new Set(result.affectedIds);
      setSelectedIds(prev => new Set([...prev].filter(id => !removed.has(id))));
      setPrimaryId(id => id != null && removed.has(id) ? null : id);
      setPrimaryAsset(asset => asset && removed.has(asset.id) ? null : asset);
      refreshAfterWrite(false, result.affectedFolderPaths);
      await advanceAfterRemoval(result.affectedIds);
      window.setTimeout(() => setToast(""), 8000);
    } catch (error) { setToast(String(error)); window.setTimeout(() => setToast(""), 6000); }
  }, [refreshAfterWrite, advanceAfterRemoval]);
  const createFolder=useCallback((parentPath:string)=>{if(appInfo?.readonly){setToast("Katalóg je iba na čítanie");return;}setFileOp(null);setNameOp({kind:"folder",parentPath});},[appInfo]);
  const submitName=useCallback(async(name:string)=>{if(!nameOp)return;const result=await invoke<any>(nameOp.kind==="rename"?"rename_file":"create_folder",nameOp.kind==="rename"?{assetId:nameOp.assetId,newBaseName:name}:{parentPath:nameOp.parentPath,name});setNameOp(null);setToast(result.summary);setOperationProgress("");const paths=result.affectedFolderPaths??(nameOp.kind==="folder"?[nameOp.parentPath]:primaryAsset?.path?[primaryAsset.path.substring(0,primaryAsset.path.lastIndexOf("\\"))]:[]);refreshAfterWrite(true,paths);window.setTimeout(()=>setToast(""),6000);},[nameOp,primaryAsset,refreshAfterWrite]);
  useEffect(()=>{let unlisten:(()=>void)|undefined;void listen<{operation:string;done:number;total:number}>("file-operation-progress",event=>{if(event.payload.done>=event.payload.total)setOperationProgress("");else setOperationProgress(`${event.payload.operation} ${event.payload.done} / ${event.payload.total}…`)}).then(fn=>{unlisten=fn});return()=>unlisten?.()},[]);
  useEffect(()=>{if(!historyOpen)return;const outside=(event:PointerEvent)=>{if(event.target instanceof Node&&!historyRef.current?.contains(event.target))setHistoryOpen(false)};const escape=(event:globalThis.KeyboardEvent)=>{if(event.key==="Escape"){event.stopPropagation();setHistoryOpen(false)}};document.addEventListener("pointerdown",outside);window.addEventListener("keydown",escape);return()=>{document.removeEventListener("pointerdown",outside);window.removeEventListener("keydown",escape)}},[historyOpen]);
  useEffect(()=>{if(!aiOpen)return;const outside=(event:PointerEvent)=>{if(event.target instanceof Node&&!aiRef.current?.contains(event.target))setAiOpen(false)};const escape=(event:globalThis.KeyboardEvent)=>{if(event.key==="Escape"){event.stopPropagation();setAiOpen(false)}};document.addEventListener("pointerdown",outside);window.addEventListener("keydown",escape);return()=>{document.removeEventListener("pointerdown",outside);window.removeEventListener("keydown",escape)}},[aiOpen]);
  useEffect(()=>{const key=(e:globalThis.KeyboardEvent)=>{
    if(e.key==="Shift")window.__ddreamsShift=true;
    if (e.defaultPrevented || document.querySelector(".modal-backdrop,[role=dialog],[role=alertdialog]")) return;
    const target=e.target instanceof HTMLElement?e.target:null;const active=document.activeElement instanceof HTMLElement?document.activeElement:null;
    const editable=(el:HTMLElement|null)=>!!el&&(["INPUT","SELECT","TEXTAREA"].includes(el.tagName)||el.isContentEditable);
    if(editable(target)||editable(active))return;
    if(e.ctrlKey&&e.altKey&&!e.shiftKey&&e.code==="KeyX"){if(document.querySelector(".modal-backdrop,[role=dialog],[role=alertdialog]"))return;e.preventDefault();if(editors[0])openEditor(editors[0],viewer?(!viewer.asset.offline&&!viewer.asset.isDirectory?[viewer.asset.id]:[]):[...selectedIds]);return;}
    const ids=[...selectedIds];const targets=viewer?[viewer.asset.id]:ids.length?ids:primaryId==null?[]:[primaryId];
    if(!e.ctrlKey&&!e.metaKey&&!e.altKey&&!e.shiftKey&&e.key.toLowerCase()==="u"){
      if (e.repeat || !targets.length) return;
      e.preventDefault();void setAssetValues(targets,{editPending:"toggle"});return;
    }
    if(e.ctrlKey&&!e.altKey&&!e.shiftKey&&e.key>="1"&&e.key<="5"){e.preventDefault();if(!e.repeat)void setAssetValues(targets,{rating:Number(e.key)});return;}
    if(e.ctrlKey&&!e.altKey&&!e.shiftKey&&e.key==="0"){e.preventDefault();if(!e.repeat)void setAssetValues(targets,{rating:null});return;}
    if(e.altKey&&!e.ctrlKey&&!e.shiftKey&&e.key>="1"&&e.key<="6"){e.preventDefault();const labels=["Red","Yellow","Green","Blue","Purple","Gray"];if(!e.repeat)void setAssetValues(targets,{label:labels[Number(e.key)-1]});return;}
    if(e.altKey&&!e.ctrlKey&&!e.shiftKey&&e.key==="0"){e.preventDefault();if(!e.repeat)void setAssetValues(targets,{label:null});return;}
    if(!e.ctrlKey&&!e.altKey&&!e.shiftKey&&e.key.toLowerCase()==="x"){if(document.querySelector(".modal-backdrop,[role='dialog'],[role='alertdialog']")||!targets.length||e.repeat)return;e.preventDefault();void invoke<[number,string|null][]>("get_asset_labels",{assetIds:targets}).then(rows=>{if(rows.length!==targets.length)return;const allGray=rows.every(([,label])=>label?.toLowerCase()==="gray");void setAssetValues(targets,{label:allGray?null:"Gray"})});return;}
    if(!e.ctrlKey&&!e.altKey&&!e.shiftKey&&e.key==="\\"){e.preventDefault();if(!e.repeat)void setAssetValues(targets,{tagged:"toggle"});return;}
    const inGrid=!!target?.closest(".asset-scroll,.tile-context-menu")||!!active?.closest(".asset-scroll,.tile-context-menu");const inViewer=!!viewer;if(!inGrid&&!inViewer)return;
    if(e.ctrlKey&&e.key.toLowerCase()==="x"){e.preventDefault();setFileClipboard({ids:targets,mode:"move"});setFolderClipboard(null);setCutIds(targets);return;}
    if(e.ctrlKey&&e.key.toLowerCase()==="c"&&!e.shiftKey){e.preventDefault();setFileClipboard({ids:targets,mode:"copy"});setFolderClipboard(null);setCutIds([]);return;}
    if(e.ctrlKey&&e.key.toLowerCase()==="v"){e.preventDefault();if(view.kind==="folder"&&view.folderPath&&fileClipboard)void dropFiles(fileClipboard.ids,view.folderPath,fileClipboard.mode==="copy");else if(view.kind==="folder"&&view.folderPath&&folderClipboard)void folderAction("paste",view.folderPath);return;}
    if(e.key==="F2"&&primaryId!=null){e.preventDefault();beginFileOperation(targets,"rename");return;}
    if(e.key==="Delete"&&!e.ctrlKey&&!e.altKey&&targets.length){e.preventDefault();if(!e.repeat)beginFileOperation(targets,e.shiftKey?"delete-permanent":"delete",viewer?.asset);return;}
    if(e.ctrlKey&&e.key==="\\"){e.preventDefault();void selectTaggedInView();return;}
    if(e.ctrlKey&&e.key.toLowerCase()==="g"){e.preventDefault();void copyCurrentResult()}
    else if(e.ctrlKey&&e.shiftKey&&e.key.toLowerCase()==="o"&&primaryId!=null){e.preventDefault();void invoke("asset_action",{assetId:primaryId,action:"open"})}
    else if(e.altKey&&e.key==="ArrowLeft"){e.preventDefault();travel(historyIndex-1)}
    else if(e.altKey&&e.key==="ArrowRight"){e.preventDefault();travel(historyIndex+1)}
    else if(e.key==="Backspace"){e.preventDefault();void up()}
    else if(e.key==="F5"){e.preventDefault();refresh()}
    else if(e.key==="]"){setOverlayMode(v=>v===0?2:0)}
  };
  const keyup=(e:globalThis.KeyboardEvent)=>{if(e.key==="Shift")window.__ddreamsShift=false};const zoomer=(e:Event)=>setZoom(v=>Math.max(80,Math.min(400,v+(e as CustomEvent<number>).detail)));const mouse=(e:globalThis.MouseEvent)=>{if(e.button===3){e.preventDefault();travel(historyIndex-1)}else if(e.button===4){e.preventDefault();travel(historyIndex+1)}};window.addEventListener("keydown",key);window.addEventListener("keyup",keyup);window.addEventListener("ddreams-zoom",zoomer);window.addEventListener("mouseup",mouse);return()=>{window.removeEventListener("keydown",key);window.removeEventListener("keyup",keyup);window.removeEventListener("ddreams-zoom",zoomer);window.removeEventListener("mouseup",mouse)}},[travel,historyIndex,up,refresh,copyCurrentResult,primaryId,viewer,selectedIds,setAssetValues,selectTaggedInView,view,fileClipboard,folderClipboard,folderAction,dropFiles,beginFileOperation,editors]);

  const moveViewer = useCallback(async (delta: number) => {
    if (!viewer) return;
    let index = viewer.index + delta;
    while (index >= 0 && index < total) {
      const page = await invoke<Page>("get_assets", { filter: {...assetFilter, offset:index, limit:1} });
      const asset=page.items[0];
      if (!asset) break;
      if (!asset.isDirectory && isImage(asset, formats)) { showViewerAsset(asset, index); return; }
      index += delta;
    }
    setSlideshow(false);
  }, [viewer,total,assetFilter,showViewerAsset,formats]);
  useEffect(() => { if (!viewer) setSlideshow(false); }, [viewer]);
  useEffect(() => {
    if (!slideshow || !viewer) return;
    if (viewer.index >= total - 1) { setSlideshow(false); return; }
    const timer = window.setTimeout(() => void moveViewer(1).catch(() => { setSlideshow(false); setToast("Prezentáciu sa nepodarilo pokračovať."); }), slideshowSeconds * 1000);
    return () => window.clearTimeout(timer);
  }, [slideshow, viewer, total, slideshowSeconds, moveViewer]);
  useEffect(() => {
    if (!viewer) return;
    const key = (event: globalThis.KeyboardEvent) => { if (event.defaultPrevented || document.querySelector(".modal-backdrop,[role=dialog],[role=alertdialog]")) return; if (event.key === "Escape"||event.key==="Enter") setViewer(null); else if(!viewerFit&&event.key.startsWith("Arrow")&&(event.ctrlKey||event.key==="ArrowUp"||event.key==="ArrowDown")){event.preventDefault();const step=event.shiftKey?12:48;setViewerPan(p=>clampViewerPan({x:p.x+(event.key==="ArrowRight"?-step:event.key==="ArrowLeft"?step:0),y:p.y+(event.key==="ArrowDown"?-step:event.key==="ArrowUp"?step:0)},viewerZoom))}else if(event.key==="ArrowRight"&&!event.altKey)void moveViewer(1);else if(event.key==="ArrowLeft"&&!event.altKey)void moveViewer(-1);else if(event.code==="Space"){event.preventDefault();void moveViewer(1)}else if(event.key==="Backspace"){event.preventDefault();void moveViewer(-1)}else if(event.key==="+"||event.key==="="){setViewerFit(false);setViewerZoom(z=>Math.min(8,z*1.2))}else if(event.key==="-"){setViewerFit(false);setViewerZoom(z=>Math.max(.1,z/1.2))} };
    window.addEventListener("keydown",key); return () => window.removeEventListener("keydown",key);
  }, [viewer,moveViewer,viewerFit,viewerZoom,viewerDimensions]);
  useEffect(()=>{if(!viewer||!viewerCanvasRef.current)return;const el=viewerCanvasRef.current;const update=()=>setViewerCanvasSize({width:el.clientWidth,height:el.clientHeight});update();const observer=new ResizeObserver(update);observer.observe(el);return()=>observer.disconnect()},[viewer]);
  const viewerFitPercent=viewerDimensions&&viewerCanvasSize.width&&viewerCanvasSize.height?Math.round(Math.min((viewerCanvasSize.width-120)/viewerDimensions.width,(viewerCanvasSize.height-24)/viewerDimensions.height)*100):0;
  // The zoomed photo is centred in the viewer; panning is limited so it never leaves a gap it could fill.
  const clampViewerPan=(pan:{x:number;y:number},zoom:number)=>{const el=viewerCanvasRef.current,d=viewerDimensions;if(!el||!d)return pan;const mx=Math.max(0,(d.width*zoom-el.clientWidth)/2),my=Math.max(0,(d.height*zoom-el.clientHeight)/2);return{x:Math.max(-mx,Math.min(mx,pan.x)),y:Math.max(-my,Math.min(my,pan.y))};};
  const wheelViewer=(e:ReactWheelEvent<HTMLDivElement>)=>{e.preventDefault();const r=e.currentTarget.getBoundingClientRect();const cx=(e.clientX-r.left-r.width/2)/uiScale,cy=(e.clientY-r.top-r.height/2)/uiScale;const from=viewerFit?(Math.min(1,viewerFitPercent/100)||1):viewerZoom,pan=viewerFit?{x:0,y:0}:viewerPan;const to=Math.max(.05,Math.min(8,from*(e.deltaY<0?1.12:.89))),k=to/from;setViewerFit(false);setViewerZoom(to);setViewerPan(clampViewerPan({x:cx-(cx-pan.x)*k,y:cy-(cy-pan.y)*k},to));};
  useEffect(()=>{if(!viewer)return;let active=true;setViewerPreviewSource(null);setViewerBroken(false);void invoke<string>("get_preview_source",{assetId:viewer.asset.id,epoch}).then(source=>{if(active)setViewerPreviewSource(source)}).catch(()=>{});for(const d of [-1,1]){const i=viewer.index+d;if(i>=0&&i<total)void invoke<Page>("get_assets",{filter:{...assetFilter,offset:i,limit:1}}).then(p=>{const a=p.items[0];if(a&&!a.isDirectory&&a.path){const im=new Image();im.src=thumbUrl(a,0,epoch)}})}return()=>{active=false}},[viewerReadId,viewer?.index,assetFilter,total,epoch]);

  const crumbs = useMemo(() => view.kind === "folder" ? (view.folderPath ?? view.title).split(/[\\/]/).filter(Boolean) : ["Katalóg", view.title], [view]);
  const aiChipText=aiStatus.modelState==="error"?"Chyba AI modelu":aiStatus.state==="loading"?"AI sa načítava…":aiStatus.state==="indexing"?`AI indexuje ${aiStatus.indexed.toLocaleString("sk-SK")} / ${aiStatus.total.toLocaleString("sk-SK")}${aiStatus.etaS!=null?` · ~${Math.max(1,Math.round(aiStatus.etaS/3600))} h`:""}`:aiStatus.state==="ready"?"AI pripravené":aiStatus.state==="paused"?"AI pozastavené":aiStatus.state==="error"?"AI chyba":"AI vypnuté";
  const aiSearch=async()=>{const text=aiQuery.trim();if(!text||!aiReady||aiSearchBusy)return;setAiSearchBusy(true);try{const result=await invoke<AiSearchPage>("ai_search_text",{query:text,limit:300});if(result.ids.length){const suffix=result.searched<aiStatus.total?` · prehľadané ${result.searched.toLocaleString("sk-SK")} z ${aiStatus.total.toLocaleString("sk-SK")}`:"";navigate({kind:"ai",ids:result.ids,title:`AI: „${text}“${suffix}`});}else{setToast(`AI nenašlo nič (prehľadané ${result.searched.toLocaleString("sk-SK")} z ${aiStatus.total.toLocaleString("sk-SK")})`);window.setTimeout(()=>setToast(""),3500)}}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),3500)}finally{setAiSearchBusy(false)}};
  const findSimilar=async(asset:Asset)=>{try{const result=await invoke<AiSearchPage>("ai_similar",{assetId:asset.id,limit:300});if(result.ids.length)navigate({kind:"ai",ids:result.ids,title:`Podobné ako ${asset.name}`});else{setToast("AI nenašlo podobné fotky");window.setTimeout(()=>setToast(""),2500)}}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),3500)}};
  const findSeries=async()=>{if(!aiReady||seriesBusy)return;setSeriesBusy(true);try{const filter = {...assetFilter,offset:0,limit:500};const result=await invoke<AiSeriesResponse>("ai_series",{filter,preset:seriesPreset});const ids:number[]=[],groups:number[]=[],bestIds:number[]=[];result.groups.forEach((group,index)=>{for(const id of group.ids){ids.push(id);groups.push(index)}bestIds.push(group.best)});navigate({kind:"series",ids,groups,bestIds,seriesInfo:{count:result.groups.length,analyzed:result.analyzed,truncated:result.truncated},title:`Série: ${view.title}`});const notices=[];if(!result.groups.length)notices.push("V tomto zobrazení som nenašiel žiadne série");if(result.truncated)notices.push("Analyzovaných bolo prvých 20 000 fotiek");if(notices.length){setToast(notices.join(" · "));window.setTimeout(()=>setToast(""),5000)}}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),4000)}finally{setSeriesBusy(false)}};
  const applyFolderRule=async(path:string,mode:FolderRuleMode|null)=>{try{const removed=await invoke<number>("set_folder_rule",{path,mode});window.dispatchEvent(new Event(FOLDER_RULES_EVENT));const text=mode==="include"?`Zahrnuté do katalógu: ${path}`:mode==="exclude"?(removed>0?`Vylúčené z katalógu: ${removed} ${removed===1?"súbor":removed<5?"súbory":"súborov"}`:`Vylúčené z katalógu: ${path}`):`Pravidlo zrušené: ${path}`;setToast(text);refreshAfterWrite(true,[path]);window.setTimeout(()=>setToast(""),removed>0?8000:3500)}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),4000)}};
  const changeFolderRule=async(path:string,mode:FolderRuleMode|null)=>{if(mode==="exclude"){try{const impact=await invoke<ExclusionImpact>("get_exclusion_impact",{path});if(impact.files>0){setExcludeConfirm({path,impact});return;}}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),4000);return;}}void applyFolderRule(path,mode)};
  const requestRemoveCatalog=async(path:string)=>{try{const info=await invoke<RemoveCatalogInfo>("remove_folder_from_catalog_info",{folderPath:path});if(!info.offline&&!info.missing){setToast("Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie");window.setTimeout(()=>setToast(""),4000);return}setRemoveCatalogConfirm({path,info})}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),4000)}};
  const removeCatalogSource=async()=>{if(!removeCatalogConfirm)return;const path=removeCatalogConfirm.path;setRemoveCatalogConfirm(null);try{const result=await invoke<RemoveCatalogResult>("remove_folder_from_catalog",{folderPath:path});setToast(result.summary);refreshAfterWrite(true,[path]);window.dispatchEvent(new Event("ddreams-catalog-changed"));if(view.kind==="folder"&&view.folderPath){const norm=(v:string)=>v.replace(/[\\/]+$/g,"").toLowerCase(),root=norm(path),current=norm(view.folderPath);if(current===root||current.startsWith(root+"\\")){let candidate=folderParentPath(path);let moved=false;while(candidate){if(await invoke<boolean>("folder_exists",{path:candidate}).catch(()=>false)){const folder=await invoke<Folder|null>("find_folder_path",{path:candidate}).catch(()=>null);if(folder){navigate({kind:"folder",id:folder.id,ids:folder.folderIds?.length?folder.folderIds:[folder.id],title:folder.path??folder.name,folderPath:folder.path??undefined});moved=true;break}}candidate=folderParentPath(candidate)}if(!moved){const roots=await invoke<Folder[]>("get_folder_children",{parentId:null,parentIds:null,offset:0,limit:500}).catch(()=>[]);const first=roots.find(folder=>folder.path&&!folder.virtualNode);if(first)navigate({kind:"folder",id:first.id,ids:first.folderIds?.length?first.folderIds:[first.id],title:first.path??first.name,folderPath:first.path??undefined});else navigate({kind:"special",value:"all_photos",title:"Všetky fotky"})}}}window.setTimeout(()=>setToast(""),5000)}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),4000)}};
  const toggleAi=async()=>{const enabled=!aiEnabled;setAiEnabled(enabled);try{setAiStatus(await (enabled?invoke<AiStatus>("ai_start",{lazyModel:!aiLoadModelAtStart}):invoke<AiStatus>("ai_stop")))}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),3500)}};
  const updateAiStatus=async(command:"ai_pause"|"ai_resume"|"ai_reindex"|"ai_load_model")=>{try{setAiStatus(await invoke<AiStatus>(command))}catch(error){setToast(String(error));window.setTimeout(()=>setToast(""),3500)}};
  const openCrumb = async (index: number) => {
    if (view.kind !== "folder") return;
    const parts = (view.folderPath ?? view.title).split(/[\\/]/).filter(Boolean);
    let path = parts.slice(0, index + 1).join("\\");
    if (index === 0 && /^[A-Za-z]:$/.test(path)) path += "\\";
    const folder = await invoke<Folder | null>("find_folder_path", { path }).catch(() => null);
    if (folder) navigate({ kind: "folder", id: folder.id, ids: folder.folderIds, title: folder.path ?? folder.name, folderPath: folder.path ?? undefined });
  };
  return <main className="app-shell" style={{zoom:uiScale}}>
    <header className="topbar"><div className="top-left"><div className="brand-mark">D</div><div className="brand-title">DDreams <b>browser</b></div><nav className="top-crumbs">{crumbs.map((crumb,i)=><span key={`${crumb}-${i}`} className="crumb-item">{i>0&&<i>›</i>}<button onClick={()=>void openCrumb(i)}>{crumb}</button></span>)}{Object.values(filters).some(Boolean)&&<span className="active-filters">{filters.ratingMode&&`Hodnotenie ${filters.ratingMode} ${filters.ratingValue??""}`} {filters.labelFilter&&`Farebný štítok: ${labelText(filters.labelFilter)}`} {filters.taggedOnly&&"Označené"} {filters.fileGroup&&`Typ: ${filters.fileGroup==="all"?"všetky súbory":filters.fileGroup}`}<button onClick={()=>setFilters({})}>×</button></span>}</nav><span className="count-pill">{slovakItems(total)}</span></div><label className="search-box"><span>⌕</span><input value={query} onChange={e=>setQuery(e.target.value)} placeholder="Filtrovať v zobrazení…"/><kbd>Ctrl+F</kbd></label><div className="top-right"><DockLayoutMenu layout={dockLayout} setLayout={setDockLayout}/><div className="ai-anchor" ref={aiRef}><button className={`ai-chip ai-${aiStatus.state}`} title={!aiStatus.available?aiStatus.reason??"AI modul nie je nainštalovaný":aiStatus.lastError??"AI index a vyhľadávanie"} onClick={()=>setAiOpen(v=>!v)}>{aiChipText}</button>{aiOpen&&<div className="ai-popover"><b>AI katalóg</b><div><span>Zaindexované</span><strong>{aiStatus.indexed.toLocaleString("sk-SK")}</strong></div><div><span>Vo fronte</span><strong>{aiStatus.queued.toLocaleString("sk-SK")}</strong></div><div><span>Čaká na spracovanie</span><strong>{aiStatus.waiting.toLocaleString("sk-SK")}</strong></div><div><span>AI model</span><strong>{({unloaded:"nenačítaný",loading:"načítava sa",ready:"pripravený",error:"chyba"} as Record<string,string>)[aiStatus.modelState]??aiStatus.modelState}</strong></div>{aiStatus.modelState==="unloaded"&&<button disabled={!aiStatus.available||!aiEnabled} onClick={()=>void updateAiStatus("ai_load_model")}>Spracovať čakajúce fotografie <small>vyžaduje AI model</small></button>}<div><span>Offline</span><strong>{aiStatus.offline.toLocaleString("sk-SK")}</strong></div><div><span>Chyby</span><strong>{aiStatus.failed.toLocaleString("sk-SK")}</strong></div>{aiStatus.lastError&&<small>{aiStatus.lastError}</small>}<button disabled={!aiStatus.available||!aiEnabled||aiStatus.state==="loading"} onClick={()=>void updateAiStatus(aiStatus.state==="paused"?"ai_resume":"ai_pause")}>{aiStatus.state==="paused"?"Pokračovať":"Pozastaviť"}</button><button disabled={!aiStatus.available||!aiEnabled} onClick={()=>void updateAiStatus("ai_reindex")}>Aktualizovať index</button><button onClick={()=>void toggleAi()}>{aiEnabled?"Vypnúť AI":"Zapnúť AI"}</button></div>}</div><div className="history-anchor" ref={historyRef}><button className="catalog-label" title={catalogTooltip} onClick={()=>{setHistoryOpen(v=>!v);loadRecentChanges()}}><span className="status-light"/>{appInfo?.readonly ? "Katalóg iba na čítanie" : "Katalóg"}</button>{historyOpen&&<div className="change-history"><div className="change-history-head"><b>História zmien</b></div>{recentChanges.length?recentChanges.map(change=><div key={change.id}><time>{new Date(change.ts).toLocaleString("sk-SK")}</time><span>{change.summary}</span></div>):<small>Žiadne zmeny</small>}</div>}</div><button className="settings-button" title="Pomocník (F1)" onClick={()=>setHelpOpen(true)}>?</button><button className="settings-button" title="Nastavenia" onClick={()=>setSettingsOpen(true)}>⚙</button></div></header>
    <div className="workspace">
      <LeftPanel hosts={dockHosts} onNavigate={navigate} activeFolderId={view.kind === "folder" ? view.id ?? null : null} activeFolderIds={view.kind === "folder" ? view.ids : []} activeView={view} calendarRefreshToken={refreshTick} refreshToken={treeRefreshTick} refreshPaths={treeRefreshPaths} onFileDrop={dropFiles} onCreateFolder={createFolder} onFolderRule={(path,mode)=>void changeFolderRule(path,mode)} onFolderAction={(action,path)=>void folderAction(action,path)} clipboardPath={folderClipboard?.mode==="move"?folderClipboard.path:null} clipboardAvailable={!!folderClipboard||!!fileClipboard} onToast={message=>{setToast(message);window.setTimeout(()=>setToast(""),3000)}}/>
      <DockSections hosts={dockHosts} setLayout={setDockLayout} controls={dockControls} collapsed={dockLayout.collapsed} setPresence={setDockPresence}><RightPanel primaryReadId={primaryReadId} selectedReadIds={selectedReadIds} labelCounts={availableLabels} valueTokens={valueTokens} primaryValues={primaryValueEdit?.refreshTick === refreshTick && primaryValueEdit.patch.id === primaryId ? primaryValueEdit.patch : undefined} incomingTools={<IncomingTools folder={incomingFolder} ids={selectedList}
        aiReady={aiReady} aiReason={!aiEnabled?"AI je vypnutá":aiStatus.reason??aiStatus.lastError??"AI sa pripravuje"}
        readOnly={!!appInfo?.readonly} indexRevision={aiStatus.state==="indexing"?0:aiStatus.indexed}
        refreshToken={refreshTick} onMove={path=>void dropFiles(selectedList,path,false)}
        onShowPhotos={(ids,title,groups)=>{setCenterMode("grid");navigate({kind:"ai",ids,title,groups});}}
        onMoveGroup={(ids,path)=>void dropFiles(ids,path,false)}
        onChooseFolder={()=>beginFileOperation(selectedList,"move")} onAnalyze={()=>runDeletionAction("analyze_incoming_series")}/>} thumbEpoch={epoch} catalogReady={ready} activeColors={view.kind==="color"?(view.value??""):""} activeQuality={view.kind==="quality"?(view.value??""):""} primaryId={primaryId} selectedIds={selectedList} includeChildren={includeChildren} onIncludeChildrenChange={setIncludeChildren} onNavigate={navigate} onSetValues={setAssetValues} onSetColors={setAssetColors} aiAvailable={aiStatus.available} aiReady={aiReady} aiReason={aiStatus.reason??"AI modul nie je nainštalovaný"} onSetCategory={setAssetCategories} onCategoryChanged={categoryStructureChanged} onRefresh={refreshAfterWrite} onClusterNotice={message=>{setToast(message);window.setTimeout(()=>setToast(""),3500)}} onSetKeyword={setAssetKeywords} onCreateAssignKeyword={createAndAssignKeyword} onKeywordChanged={keywordStructureChanged} onCategoryError={categoryWriteError} readOnly={!!appInfo?.readonly} refreshToken={refreshTick} colorToken={aiTick} catalogRefreshToken={refreshTick}/></DockSections>
      <DockSide side="left" layout={dockLayout} setLayout={setDockLayout} hosts={dockHosts} hideNav={!!viewer} dragging={dockDragging} controls={dockControls} sectionPresence={dockPresence}/>
      <section className="center-pane">
        <div className="toolbar"><div className="toolbar-left"><button className="nav-btn" disabled={historyIndex<=0} onClick={()=>travel(historyIndex-1)}>←</button><button className="nav-btn" disabled={historyIndex>=history.length-1} onClick={()=>travel(historyIndex+1)}>→</button><button className="nav-btn" disabled={view.kind!=="folder"} onClick={()=>void up()}>↑</button><button className="nav-btn" onClick={refresh}>↻</button><div className="type-pills">{([["","Všetko"],["images","Fotografie"],["raw","RAW"],["video","Video"]] as const).map(([v,l])=><button key={v} className={`pill ${(filters.fileGroup??"")===v?"active":""}`} onClick={()=>setFilters(f=>({...f,fileGroup:(v||undefined) as ViewFilters["fileGroup"]}))}>{l}</button>)}</div></div><div className="toolbar-right">{(view.kind==="ai"||view.kind==="series"||view.kind==="people")?<select disabled value="relevance" title={view.kind==="people"?"Fotky osoby sú zoradené podľa dátumu":"Výsledky AI sú zoradené podľa zhody"}><option value="relevance">{view.kind==="people"?"Podľa dátumu":"Podľa zhody"}</option></select>:<><select disabled={editPendingView} value={editPendingView?"editPending":sort} onChange={e=>changeSort(e.target.value, descending)}>{editPendingView&&<option value="editPending">Najdlhšie čakajúce</option>}{[["name","Názov"],["dateTaken","Dátum nasnímania"],["modified","Dátum zmeny"],["size","Veľkosť"],["type","Typ súboru"],["dimensions","Rozmery"],["rating","Hodnotenie"],["label","Farebný štítok"],["tagged","Označené"],["caption","Popis"]].map(([v,l])=><option value={v} key={v}>{l}</option>)}{aiReady&&<option value="similarity">Podľa podobnosti</option>}</select><button className="direction-button" disabled={editPendingView} title={!editPendingView&&descending?"Zostupne":"Vzostupne"} onClick={()=>changeSort(sort, !descending)}>{!editPendingView&&descending?"↓":"↑"}</button></>}{view.kind!=="series"&&<details {...seriesMenu} className="toolbar-menu" title={!aiReady?(aiStatus.reason??"AI nie je pripravená"):undefined}><summary>Série</summary><div className="toolbar-pop"><select value={seriesPreset} disabled={!aiReady||seriesBusy} onChange={e=>setSeriesPreset(e.target.value as SeriesPreset)}><option value="strict">Prísne</option><option value="normal">Stredné</option><option value="loose">Voľné</option></select><button disabled={!aiReady||seriesBusy} title={!aiReady?(aiStatus.reason??"AI nie je pripravená"):undefined} onClick={()=>void findSeries()}>{seriesBusy?"Hľadám série…":"Nájsť série v tomto zobrazení"}</button></div></details>}<details {...filtersMenu} className="toolbar-menu"><summary>Filtre</summary><div className="toolbar-pop"><select value={filters.ratingMode??""} onChange={e=>setFilters(f=>({...f,ratingMode:(e.target.value||undefined) as ViewFilters["ratingMode"],ratingValue:e.target.value&&e.target.value!=="unrated"?(f.ratingValue??1):undefined}))}><option value="">Hodnotenie: všetky</option><option value="equal">Rovné</option><option value="atLeast">Aspoň</option><option value="atMost">Najviac</option><option value="unrated">Bez hodnotenia</option></select>{filters.ratingMode&&filters.ratingMode!=="unrated"&&<select value={filters.ratingValue??1} onChange={e=>setFilters(f=>({...f,ratingValue:Number(e.target.value)}))}>{[1,2,3,4,5].map(v=><option value={v} key={v}>{v}</option>)}</select>}<select value={filters.labelFilter??""} onChange={e=>setFilters(f=>({...f,labelFilter:e.target.value||undefined}))}><option value="">Farebný štítok: všetky</option>{availableLabels.filter(v=>v.value).map(v=><option value={v.value} key={v.value}>{labelText(v.value)}</option>)}</select><label><input type="checkbox" checked={!!filters.taggedOnly} onChange={e=>setFilters(f=>({...f,taggedOnly:e.target.checked||undefined}))}/> Len označené</label><select value={filters.fileGroup??""} onChange={e=>setFilters(f=>({...f,fileGroup:(e.target.value||undefined) as ViewFilters["fileGroup"]}))}><option value="">Typ: fotky a videá</option>{[["images","Fotografie"],["raw","RAW"],["video","Video"],["other","Ostatné"],["all","Všetky súbory"]].map(([v,l])=><option value={v} key={v}>{l}</option>)}</select><button onClick={()=>setFilters({})}>Vymazať filtre</button></div></details><details {...displayMenu} className="toolbar-menu"><summary>Zobrazenie</summary><div className="toolbar-pop"><select value={hoverMode} onChange={e=>setHoverMode(e.target.value as typeof hoverMode)} title="Náhľad po prejdení myšou"><option value="off">Náhľad vypnutý</option><option value="hover">Náhľad pri prejdení</option><option value="shift">Náhľad so Shift</option></select><button onClick={()=>setOverlayMode(v=>v===0?2:0)} title="Ikonky na náhľadoch, kláves ]">Ikonky {overlayMode===0?"vypnuté":"zapnuté"}</button><button onClick={()=>setThumbnailInfoOpen(true)}>&#218;daje pod n&#225;h&#318;admi&#8230;</button><button onClick={()=>setHelpOpen(true)}>Pomocník (F1)</button></div></details><div className="center-mode-pills" title="Zobrazenie stredu: mriežka náhľadov alebo mapa katalógu"><button className={centerMode==="grid"?"active":""} onClick={()=>setCenterMode("grid")}>Mriežka</button><button className={centerMode==="map"?"active":""} onClick={()=>setCenterMode("map")}>Mapa</button></div><div className="zoom-control"><span>−</span><input type="range" min="80" max="400" step="8" value={zoom} onChange={e=>setZoom(Number(e.target.value))}/><span>＋</span></div></div></div>
        {aiSearchTarget&&createPortal(<label className="ai-search-box" title={!aiReady?(aiStatus.reason??"AI modul nie je nainštalovaný"):""}><span>AI</span><small className="ai-model-hint">vyžaduje AI model</small><input value={aiQuery} disabled={!aiReady||aiSearchBusy} onChange={e=>setAiQuery(e.target.value)} onKeyDown={e=>{if(e.key==="Enter"&&!aiSearchBusy)void aiSearch()}} placeholder="Hľadať obsahom (AI)…"/>{aiSearchBusy&&<i className="ai-search-spinner" aria-label="Vyh?ad?vam"/>}</label>,aiSearchTarget)}
        {view.suggestion&&<div className="selection-bar ai-review-actions"><strong>Priradiť do {view.suggestion.path}:</strong><button disabled={applyingSuggestion||!!appInfo?.readonly||!view.ids?.length} onClick={()=>void applySuggestion(view.ids??[])}>Všetkým ({view.ids?.length??0})</button><button disabled={applyingSuggestion||!!appInfo?.readonly||!selectedIds.size} onClick={()=>void applySuggestion([...selectedIds])}>Vybraným ({selectedIds.size})</button><button disabled={!selectedIds.size} onClick={()=>{const ids=[...selectedIds];void invoke("ai_reject_suggestion",{assetIds:ids,kind:view.suggestion!.kind,labelId:view.suggestion!.labelId}).finally(()=>refreshAfterWrite());const remaining=(view.ids??[]).filter(id=>!selectedIds.has(id));navigate({...view,ids:remaining});setToast(`Odmietnuté: ${ids.length}`);}}>Odmietnuť vybrané</button></div>}
                {view.kind==="series"&&view.seriesInfo&&<div className="selection-bar ai-review-actions series-review-actions"><strong>{view.seriesInfo.count} sérií · {view.ids?.length??0} fotiek</strong><button disabled={!!appInfo?.readonly||!selectedIds.size} onClick={()=>{const selectedGroups=new Set((view.groups??[]).filter((_,index)=>selectedIds.has(view.ids?.[index]??-1)));const rejected=(view.ids??[]).filter((id,index)=>selectedGroups.has(view.groups?.[index]??-1)&&!selectedIds.has(id));if(rejected.length)void setAssetValues(rejected,{label:"Gray"})}}>Ponechať vybrané, zvyšok série vyradiť</button><button disabled={!!appInfo?.readonly} onClick={()=>{const best=new Set(view.bestIds??[]);const rejected=(view.ids??[]).filter(id=>!best.has(id));if(rejected.length&&window.confirm(`Vyradiť ${rejected.length} fotiek a ponechať najlepšiu z každej série?`))void setAssetValues(rejected,{label:"Gray"})}}>Vyradiť všetko okrem najlepších</button><small>X = vyradiť / vrátiť vybrané</small></div>}
        {(selectedList.some(id=>deletionSuggestions.has(id)) || (view.kind==="special"&&view.value==="delete_suggested")) &&
          <div className="selection-bar ai-review-actions">
            <button disabled={!!appInfo?.readonly || quarantineBusy || !deletionSuggestions.size} onClick={() => setQuarantineConfirm({
              ids: selectedList.some(id => deletionSuggestions.has(id))
                ? selectedList.filter(id => deletionSuggestions.has(id)) : [...deletionSuggestions],
              action: "move", suggestionsOnly: true,
            })}>Presunúť návrhy do karantény</button>
            {selectedList.some(id=>deletionSuggestions.has(id))&&<button disabled={!!appInfo?.readonly}
              onClick={()=>void runDeletionAction("keep_suggested_assets", selectedList.filter(id=>deletionSuggestions.has(id)))}>Odmietnuť návrh na vymazanie</button>}
            {view.kind==="special"&&view.value==="delete_suggested"&&<button disabled={!!appInfo?.readonly||!deletionSuggestions.size}
              onClick={()=>void runDeletionAction("keep_suggested_assets")}>Zrušiť všetky návrhy</button>}
          </div>}
        {view.kind === "special" && view.value === "quarantine" && <div className="selection-bar ai-review-actions">
          <button disabled={!!appInfo?.readonly || quarantineBusy || !selectedList.length}
            onClick={() => setQuarantineConfirm({ ids: selectedList, action: "restore" })}>Vrátiť späť</button>
          <button disabled={!!appInfo?.readonly || quarantineBusy || !selectedList.length}
            onClick={() => void requestQuarantineDelete(selectedList)}>Zmazať teraz</button>
        </div>}
        {quarantineReview && reviewInGrid && <div className="selection-bar ai-review-actions">
          <button onClick={() => setReviewInGrid(false)}>Späť ku kontrole importu a karantény</button>
          <button onClick={() => { setQuarantineReview(null); setReviewInGrid(false); }}>Teraz nie</button>
        </div>}
        {centerMode==="map"?<MapView epoch={epoch} selectedIds={selectedIds} onSelect={select} onNavigate={(ids,title)=>navigate({kind:"people",ids,title})} onGrid={()=>setCenterMode("grid")} onOpenNeighborhood={ids=>void openMapNeighborhood(ids)}/>:<>
        <AssetGrid ref={grid} view={view} quarantineDeleteDays={quarantineSettings?.deleteAfterDays} onQuarantine={requestQuarantineMove} deletionSuggestions={deletionSuggestions} onKeepSuggested={id=>void runDeletionAction("keep_suggested_assets",[id])} qualityToken={aiTick} query={queryDebounced} sort={sort} descending={descending} zoom={zoom} epoch={epoch} refreshToken={refreshTick} selectedIds={selectedIds} primaryId={primaryId} cutIds={cutIds} filters={queryFilters} overlayMode={overlayMode} hoverMode={hoverMode} includeChildren={includeChildren} formats={formats} thumbnailInfo={thumbnailInfo} onOpenThumbnailInfo={()=>setThumbnailInfoOpen(true)} onSetValues={setAssetValues} onFileOperation={beginFileOperation} onFindSimilar={asset=>void findSimilar(asset)} aiReady={aiReady} aiReason={aiStatus.reason??"AI modul nie je nainštalovaný"} readOnly={!!appInfo?.readonly} uiScale={uiScale} onSelect={select} onSelectRange={selectRange} onSelectAll={ids=>void selectAll(ids)} onOpen={(asset,index)=>{if(asset.isDirectory){const id=asset.folderId??asset.id;navigate({kind:"folder",id,ids:asset.folderId?[asset.folderId]:undefined,title:asset.path??asset.name,folderPath:asset.path??undefined});return;}setViewer({asset,index});setPrimaryId(asset.id);setPrimaryAsset(asset);setSelectedIds(new Set([asset.id]));}} onCounts={counts} onViewport={updateViewport}/>
        </>}
        {operationProgress&&<div className="grid-footer"><span>{operationProgress}</span></div>}
        {viewer && <div className="viewer-overlay" ref={viewerOverlayRef} tabIndex={-1}><div className="viewer-bar"><button onClick={()=>setViewer(null)}>‹ Zavrieť</button>
          {!viewer.asset.isDirectory && isImage(viewer.asset, formats) && viewer.asset.quarantinedAt == null && <>
            <button type="button" className="viewer-triage" title="U" aria-pressed={viewer.asset.editPendingSince != null} disabled={!!appInfo?.readonly}
              onClick={() => void setAssetValues([viewer.asset.id], { editPending: "toggle" })}>Na úpravu</button>
            <button type="button" className="viewer-triage" disabled={!!appInfo?.readonly || quarantineBusy || viewer.asset.offline || viewer.asset.missing}
              onClick={() => requestQuarantineMove([viewer.asset.id])}>Do karantény</button>
          </>}{deletionSuggestions.has(viewer.asset.id)&&<>
          <span className="deletion-suggestion-badge" title="Návrh na vymazanie" aria-label="Návrh na vymazanie">✕</span>
          <button disabled={!!appInfo?.readonly} onClick={()=>void runDeletionAction("keep_suggested_assets",[viewer.asset.id])}>Odmietnuť návrh na vymazanie</button>
        </>}{editors.map((editor,index)=><button key={editor.path} title={`${editor.path}${index===0?" (Ctrl+Alt+X)":""}`} disabled={viewer.asset.offline||viewer.asset.isDirectory} onClick={()=>openEditor(editor,[viewer.asset.id])}>Otvoriť v: {editor.name}</button>)}<button className="primary" onClick={()=>setSlideshow(v=>!v)} disabled={total<2}>{slideshow?"❚❚ Pozastaviť":"▶ Spustiť prezentáciu"}</button><label>Interval <select aria-label="Interval prezentácie" value={slideshowSeconds} onChange={e=>setSlideshowSeconds(Number(e.target.value))}>{[2,3,5,10,15,30].map(seconds=><option key={seconds} value={seconds}>{seconds} s</option>)}</select></label><button title="Prepnúť medzi prispôsobením oknu a skutočnou veľkosťou" onClick={()=>{setViewerFit(v=>!v);setViewerZoom(1);setViewerPan({x:0,y:0})}}>{viewerFit?"100 %":"Prispôsobiť"}</button><button title="Zamknúť priblíženie pri prechode na ďalšiu fotku" onClick={()=>setViewerLock(v=>!v)}>{viewerLock?"🔒":"🔓"}</button><span className="viewer-title">{viewer.asset.name}</span><span className="viewer-info">{viewerFit?`${viewerFitPercent} %`:`${Math.round(viewerZoom*100)} %`}{viewerDimensions&&<> · {viewerDimensions.width} × {viewerDimensions.height}{formatFor(viewer.asset,formats)?.group==="raw"?" \u00b7 vložený náhľad":""}{viewerPreviewSource?` \u00b7 zdroj: ${viewerPreviewSource}`:""}</>}</span><span className="viewer-count">{viewer.index+1} / {total.toLocaleString("sk-SK")}</span></div><div ref={viewerCanvasRef} className="viewer-canvas" onPointerDown={e=>{if(!viewerFit&&!(e.target as HTMLElement).closest(".viewer-arrow"))e.currentTarget.setPointerCapture(e.pointerId)}} onWheel={wheelViewer} onPointerMove={e=>{if(e.buttons===1&&!viewerFit&&e.currentTarget.hasPointerCapture(e.pointerId))setViewerPan(p=>clampViewerPan({x:p.x+e.movementX/uiScale,y:p.y+e.movementY/uiScale},viewerZoom))}} onDoubleClick={e=>{if(!(e.target as HTMLElement).closest(".viewer-arrow"))setViewer(null)}}><button className="viewer-arrow" onClick={()=>void moveViewer(-1)}>‹</button>{!viewer.asset.isDirectory&&!viewerBroken?<img className="viewer-image" style={viewerFit?{maxWidth:"100%",maxHeight:"100%",objectFit:"contain"}:{maxWidth:"none",maxHeight:"none",transformOrigin:"center",flex:"none",transform:`translate(${viewerPan.x}px,${viewerPan.y}px) scale(${viewerZoom})`}} src={thumbUrl(viewer.asset,0,epoch)} alt={viewer.asset.name} onLoad={e=>setViewerDimensions({width:e.currentTarget.naturalWidth,height:e.currentTarget.naturalHeight})} onError={()=>setViewerBroken(true)}/>:<div className="viewer-nonimage">{viewer.asset.ext.toUpperCase()}<small>Náhľad nie je k dispozícii</small></div>}<button className="viewer-arrow" onClick={()=>void moveViewer(1)}>›</button></div><Filmstrip key={refreshTick} filter={assetFilter} index={viewer.index} total={total} epoch={epoch} onPick={(asset,index)=>{if(!asset.isDirectory && isImage(asset, formats)) showViewerAsset(asset, index);}}/><div className="viewer-footer">{viewer.asset.path}</div></div>}
      </section>
      <DockSide side="right" layout={dockLayout} setLayout={setDockLayout} hosts={dockHosts} hideNav={!!viewer} dragging={dockDragging} controls={dockControls} sectionPresence={dockPresence}/>
    </div>
    <footer className="status-bar"><span>Spolu {slovakItems(total)} · {formatBytes(totalSize)}</span>{primaryAsset ? <><div className="selected-name">{selectedIds.size > 1 ? `${selectedIds.size} vybraných` : primaryAsset.name}</div><span>{formatBytes(primaryAsset.size)}</span><span>{dateText(primaryAsset.exifDate)}</span><span>{primaryAsset.width && primaryAsset.height ? `${primaryAsset.width} × ${primaryAsset.height}` : "—"}</span></> : null}</footer>
    {thumbnailInfoOpen&&<ThumbnailInfoDialog value={thumbnailInfo} onSave={value=>{setThumbnailInfo(normalizeThumbnailInfo(value));setThumbnailInfoOpen(false);}} onClose={()=>setThumbnailInfoOpen(false)}/> }
    {fileOp&&<FileOperationDialog mode={fileOp.mode} initialPath={view.kind==="folder"?(view.folderPath??""):""} onChoose={path=>void chooseOperationTarget(path)} onClose={()=>setFileOp(null)} onCreate={createFolder}/>}
    {moveWarning&&<div className="modal-backdrop">
      <div className="confirm-dialog" role="alertdialog" aria-modal="true"
        aria-labelledby="move-uncategorized-title" aria-describedby="move-uncategorized-description"
        onKeyDown={event=>{if(event.key==="Escape"){event.stopPropagation();setMoveWarning(null);}}}>
        <b id="move-uncategorized-title">Presunúť fotky bez kategórie?</b>
        <p id="move-uncategorized-description">{moveWarning.count===1
          ? "Fotka, ktorú presúvaš, nie je v žiadnej kategórii."
          : `Počet presúvaných fotiek bez kategórie: ${moveWarning.count}.`}</p>
        <div className="confirm-actions">
          <button autoFocus onClick={()=>setMoveWarning(null)}>Zrušiť</button>
          <button onClick={()=>{
            const {ids,path,conflict}=moveWarning;
            setMoveWarning(null);
            void runFileOperation(ids,"move",path,conflict,true);
          }}>Presunúť aj tak</button>
        </div>
      </div>
    </div>}
    {fileConflict&&<div className="modal-backdrop"><section className="file-op-dialog conflict-dialog"><header>Konflikt názvov</header><p>V cieli už existuje {fileConflict.count} {fileConflict.count===1?"súbor":fileConflict.count<5?"súbory":"súborov"} s rovnakým názvom.</p><footer><button onClick={()=>{void runFileOperation(fileConflict.ids,fileConflict.mode,fileConflict.path,"skip");setFileConflict(null)}}>Preskočiť</button><button onClick={()=>{void runFileOperation(fileConflict.ids,fileConflict.mode,fileConflict.path,"keep_both");setFileConflict(null)}}>Ponechať oba (pridať číslo)</button><button onClick={()=>setFileConflict(null)}>Zrušiť</button></footer></section></div>}
    {folderRenameOp&&<FileNameDialog title="Premenovať priečinok" label="Nový názov priečinka" initialName={folderRenameOp.name} onSubmit={submitFolderRename} onClose={()=>setFolderRenameOp(null)}/>}{nameOp&&<FileNameDialog title={nameOp.kind==="rename"?"Premenovať súbor":"Nový priečinok"} label={nameOp.kind==="rename"?"Nový názov":"Názov priečinka"} initialName={nameOp.kind==="rename"?nameOp.name:""} extension={nameOp.kind==="rename"?nameOp.extension:undefined} onSubmit={submitName} onClose={()=>setNameOp(null)}/>}
    {toast&&<div className="app-toast">{toast}</div>}
    {quarantineReview && <QuarantineReview review={quarantineReview} hidden={reviewInGrid} busy={quarantineBusy} progress={operationProgress}
      onShow={inspectQuarantineReview} onClose={() => setQuarantineReview(null)}
      onExecute={(selection, confirmedNoBinDrives) => void runQuarantine("execute_quarantine_review", { selection, confirmedNoBinDrives })} />}
    {quarantineConfirm && <div className="modal-backdrop"><div className="confirm-dialog" role="alertdialog" aria-modal="true"
      onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); setQuarantineConfirm(null); } }}>
      <b>{quarantineConfirm.action === "restore" ? "Vrátiť fotky z karantény?" : "Presunúť fotky do karantény?"}</b>
      <p>{slovakItems(quarantineConfirm.ids.length)} vrátane príslušných XMP.</p>
      <p className="confirm-path">{quarantineConfirm.action === "restore" ? "Do pôvodných priečinkov. Pri kolízii názvov zostanú oba súbory."
        : quarantineSettings?.quarantineFolder || "Priečinok karantény vyberiete v nastaveniach."}</p>
      <div className="confirm-actions">
        <button autoFocus onClick={() => setQuarantineConfirm(null)}>Zrušiť</button>
        {quarantineConfirm.action === "restore" ? <button onClick={() => {
          setQuarantineConfirm(null);
          void runQuarantine("restore_from_quarantine", { assetIds: quarantineConfirm.ids });
        }}>Vrátiť späť</button> : ([[ "skip", "Presunúť, kolízie preskočiť" ], [ "keep_both", "Presunúť, ponechať oba" ]] as const)
          .map(([conflict, label]) => <button key={conflict} onClick={() => {
            setQuarantineConfirm(null);
            void runQuarantine("move_to_quarantine", { assetIds: quarantineConfirm.ids, conflict,
              suggestionsOnly: !!quarantineConfirm.suggestionsOnly });
          }}>{label}</button>)}
      </div>
    </div></div>}
    {!!quarantineFailures.length && <div className="modal-backdrop"><div className="confirm-dialog" role="alertdialog" aria-modal="true">
      <b>Nedokončené operácie karantény</b>
      <ul className="quarantine-failures">{quarantineFailures.map((failure, index) => <li key={index}>{failure}</li>)}</ul>
      <div className="confirm-actions"><button autoFocus onClick={() => setQuarantineFailures([])}>Zavrieť</button></div>
    </div></div>}
  {helpOpen&&<HelpDialog onClose={()=>setHelpOpen(false)}/>}
    {settingsOpen&&<SettingsDialog incomingFolder={incomingFolder??""} readOnly={!!appInfo?.readonly}
       quarantineSettings={quarantineSettings}
       onQuarantineSettingsChange={async preferences => setQuarantineSettings(await invoke<QuarantineSettings>("set_quarantine_settings", { preferences }))}
       onIncomingFolderChange={async path => {
         setIncomingFolder(await invoke<string>("set_incoming_folder", { path }));
         setQuarantineSettings(await invoke<QuarantineSettings>("get_quarantine_settings"));
       }} onClose={()=>setSettingsOpen(false)} onRule={(path,mode)=>void changeFolderRule(path,mode)} showOtherFiles={showOtherFiles} onShowOtherFiles={setShowOtherFiles} aiEnabled={aiEnabled} aiLoadModelAtStart={aiLoadModelAtStart} onAiLoadModelAtStart={setAiLoadModelAtStart} aiAvailable={aiStatus.available} aiStatusText={aiChipText} onToggleAi={()=>void toggleAi()} uiScale={uiScale} onUiScale={setUiScale} onRemoveSource={path=>void requestRemoveCatalog(path)}/>}
    {removeCatalogConfirm&&<div className="modal-backdrop"><div className="confirm-dialog" role="alertdialog"><b>Odstrániť zdroj z katalógu?</b><p>Z katalógu sa odstráni <b>{removeCatalogConfirm.info.assets.toLocaleString("sk-SK")}</b> záznamov o súboroch a <b>{removeCatalogConfirm.info.folders.toLocaleString("sk-SK")}</b> priečinkov z „{removeCatalogConfirm.path}“.</p><p>Súbory sa nemažú – {removeCatalogConfirm.info.offline?"disk nie je pripojený":"priečinok už neexistuje"}. Kategórie, hodnotenia, kľúčové slová a osoby týchto fotiek sa z katalógu stratia. Pred odstránením sa vytvorí záloha katalógu.</p><div className="confirm-actions"><button onClick={()=>setRemoveCatalogConfirm(null)}>Zrušiť</button><button className="danger" autoFocus onClick={()=>void removeCatalogSource()}>Odstrániť z katalógu</button></div></div></div>}
    {excludeConfirm&&<div className="modal-backdrop"><div className="confirm-dialog" role="alertdialog">
      <b>Vylúčiť priečinok z katalógu?</b>
      <p className="confirm-path">{excludeConfirm.path}</p>
      <p>V katalógu je pod ním <b>{excludeConfirm.impact.files}</b> {excludeConfirm.impact.files===1?"súbor":excludeConfirm.impact.files<5?"súbory":"súborov"}: s kategóriami {excludeConfirm.impact.withCategories}, s kľúčovými slovami {excludeConfirm.impact.withKeywords}, s hodnotením/štítkom/označením {excludeConfirm.impact.withRatingLabelTagged}, s opravou farieb {excludeConfirm.impact.withColorOverrides}.</p>
      <p>Tieto údaje sa z katalógu odstránia. Súbory na disku ostanú a priečinok sa dá ďalej prehliadať.</p>
      <div className="confirm-actions"><button onClick={()=>setExcludeConfirm(null)}>Zrušiť</button><button className="danger" autoFocus onClick={()=>{const target=excludeConfirm.path;setExcludeConfirm(null);void applyFolderRule(target,"exclude")}}>Vylúčiť a odstrániť z katalógu</button></div>
    </div></div>}
    {folderDeleteConfirm&&<div className="modal-backdrop"><div className="confirm-dialog" role="alertdialog"><b>Presunúť priečinok „{folderDeleteConfirm.path.split(/[\\/]/).filter(Boolean).pop()}“ do Koša?</b><p>Obsahuje {folderDeleteConfirm.info.files.toLocaleString("sk-SK")} súborov v {folderDeleteConfirm.info.folders.toLocaleString("sk-SK")} priečinkoch ({formatBytes(folderDeleteConfirm.info.bytes)}), z toho {folderDeleteConfirm.info.catalogued.toLocaleString("sk-SK")} fotiek v katalógu. Údaje o fotkách sa odstránia z katalógu.</p><div className="confirm-actions"><button onClick={()=>setFolderDeleteConfirm(null)}>Zrušiť</button><button className="danger" onClick={()=>void confirmFolderDelete()}>Presunúť do Koša</button></div></div></div>}{deleteConfirm&&(()=>{const info=deleteConfirm.info;const n=info?.count??deleteConfirm.ids.length;const perm=info?.permanentCount??0;const files=(k:number)=>k===1?"súbor":k<5?"súbory":"súborov";const allPerm=perm>=n;return <div className="modal-backdrop"><div className="confirm-dialog" role="alertdialog" onKeyDown={e=>{if(e.key==="Escape"){e.stopPropagation();setDeleteConfirm(null)}}}>
      <b>{allPerm?(n===1?"Zmazať súbor natrvalo?":`Zmazať ${n} ${files(n)} natrvalo?`):(n===1?"Presunúť súbor do Koša?":`Presunúť ${n} ${files(n)} do Koša?`)}</b>
      {(info?.names??(deleteConfirm.name?[deleteConfirm.name]:[])).length>0&&<p className="confirm-path">{(info?.names??[deleteConfirm.name]).join(", ")}{n>(info?.names.length??1)?` a ďalších ${n-(info?.names.length??1)}`:""}</p>}
      {perm>0&&<p className="delete-warning">{allPerm?"Súbory sa zmažú natrvalo.":`${perm} z ${n} sa zmaže natrvalo, ostatné pôjdu do Koša.`}{info&&info.noBinDrives.length>0?` Disk ${info.noBinDrives.join(", ")} nemá Kôš (USB alebo sieťový disk).`:deleteConfirm.permanent?" (Shift+Delete)":""}</p>}
      <p>{info&&info.sidecars>0?`Spolu s nimi sa zmaže ${info.sidecars} ${files(info.sidecars)} .xmp. `:"Sprievodné súbory (.xmp) sa zmažú s nimi. "}Ich údaje sa odstránia z katalógu.</p>
      <div className="confirm-actions"><button autoFocus={allPerm} onClick={()=>setDeleteConfirm(null)}>Zrušiť</button><button className="danger" autoFocus={!allPerm} onClick={()=>{
        if (deleteConfirm.quarantine) {
          setDeleteConfirm(null);
          void runQuarantine("delete_from_quarantine", { assetIds: deleteConfirm.ids, confirmedNoBinDrives: info?.noBinDrives ?? [] });
        } else void runDelete(deleteConfirm.ids,deleteConfirm.permanent,info?.noBinDrives??[]);
      }}>{allPerm?"Zmazať natrvalo":perm>0?"Zmazať":"Presunúť do Koša"}</button></div>
    </div></div>})()}
    <BackupReminder/>
    </main>;
}

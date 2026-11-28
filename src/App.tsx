import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { launchEditor, useEditors, type Editor } from "./editors";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import AssetGrid, { formatBytes, type AssetGridHandle } from "./components/AssetGrid";
import CompareView from "./components/CompareView";
import MapView from "./components/MapView";
import LeftPanel from "./components/LeftPanel";
import RightPanel from "./components/RightPanel";
import IncomingTools from "./components/IncomingTools";
import BackupReminder from "./components/BackupReminder";
import { useTransientDetails } from "./uiState";
import { DockSections, DockSide, useDockDrag, useDockHosts, useDockLayout, useDockPresence } from "./components/Dock";
import { patchGroup, showSection } from "./dockLayout";
import type { FormatTable, Page, ViewFilters, ValueCount } from "./types";
import { readStored, writeStored } from "./storage";
import {
  buildAssetFilter,
  effectiveViewFilters,
  readViewSort,
  sortKeyForView,
  updateViewSort,
  type SavedViewSorts,
} from "./assetFilter";
import { DEFAULT_THUMBNAIL_INFO, normalizeThumbnailInfo, type ThumbnailInfoConfig } from "./thumbnailInfo";
import { useHelpShortcut, useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import {
  useIncomingFolder,
  useDeletionSuggestions,
  useQuarantineReview,
  useDeletionActions,
  useQuarantine,
} from "./hooks/useQuarantine";
import { useViewer, useViewerImage, useViewerAssets, useViewerControls } from "./hooks/useViewer";
import { useNavigation, useListingRefresh, type AppInfo } from "./hooks/useNavigation";
import {
  useSeriesSearchSettings,
  useAiSettings,
  useAiStatus,
  useAiStartup,
  useAiSuggestion,
  useAi,
} from "./hooks/useAi";
import { useFileOperations } from "./hooks/useFileOperations";
import { useGridSelectionState, useAssetValueState, useGridSelection, useAssetValues } from "./hooks/useAssetValues";
import { useIptc } from "./hooks/useIptc";
import { WindowTopBar } from "./components/WindowTopBar";
import { WindowToolbar } from "./components/WindowToolbar";
import { ViewerOverlay } from "./components/ViewerOverlay";
import { WindowDialogs } from "./components/WindowDialogs";
import "./App.css";
import "./manage.css";

function slovakItems(count: number) {
  return `${count.toLocaleString("sk-SK")} ${count === 1 ? "položka" : count >= 2 && count <= 4 ? "položky" : "položiek"}`;
}

function dateText(value: string | null | undefined) {
  return value ? value.replace("T", " ").slice(0, 19) : "—";
}

export default function App() {
  // Keep the effectful hooks in registration order, including the overlapping window shortcuts.
  const seriesMenu = useTransientDetails();
  const filtersMenu = useTransientDetails();
  const displayMenu = useTransientDetails();
  const help = useHelpShortcut();
  const editors = useEditors();
  const openEditor = (editor: Editor, assetIds: number[]) => {
    if (assetIds.length) void launchEditor(editor, assetIds).catch((error) => window.alert(String(error)));
  };
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    void invoke<AppInfo>("get_app_info")
      .then(setAppInfo)
      .catch(() => {});
  }, []);
  const catalogTooltip = appInfo
    ? `Katalóg: ${appInfo.catalog}\nZálohy: ${appInfo.backups}\nCache: ${appInfo.cache}\nExifTool: ${appInfo.exiftool}\nFormáty: ${appInfo.formats}\nMetadata: ${appInfo.metadataView}\nNastavenia: ${appInfo.settings}\nRežim: ${appInfo.portable ? "prenosný" : "vývojový"}`
    : "Cesty katalógu sa načítavajú";
  const [query, setQuery] = useState("");
  const [queryDebounced, setQueryDebounced] = useState("");
  const [savedSorts, setSavedSorts] = useState(() => readStored<SavedViewSorts>("ddreams.sorts", {}));
  const [zoom, setZoom] = useState(168);
  const [centerMode, setCenterMode] = useState<"grid" | "map">(() => {
    const saved = readStored<string>("ddreams.centerMode", "grid");
    return saved === "map" ? "map" : "grid";
  });
  useEffect(() => writeStored("ddreams.centerMode", centerMode), [centerMode]);
  const [dockLayout, setDockLayout] = useDockLayout();
  const dockHosts = useDockHosts();
  const [dockPresence, setDockPresence] = useDockPresence();
  const openKeywordAssignment = useCallback(() => {
    const sectionId = "catalog.edit";
    setDockLayout((current) => {
      const next = showSection(current, sectionId);
      const group = [...next.left, ...next.right].find((group) =>
        group.panels.some((tab) => next.sections[tab]?.includes(sectionId)),
      );
      const tab = group?.panels.find((tab) => next.sections[tab]?.includes(sectionId));
      const expanded = { ...next, collapsed: { ...next.collapsed, [sectionId]: false } };
      return group && tab ? patchGroup(expanded, group.id, { active: tab, collapsed: false }) : expanded;
    });
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const section = dockHosts.section(sectionId);
        const picker = section.querySelector<HTMLInputElement>(".keyword-autocomplete input");
        (picker ?? section).scrollIntoView({ block: "nearest" });
        picker?.focus({ preventScroll: true });
      }),
    );
  }, [dockHosts, setDockLayout]);
  const { dragging: dockDragging, controls: dockControls } = useDockDrag();
  const [overlayMode, setOverlayMode] = useState<0 | 2>(() => (readStored<number>("ddreams.overlay", 2) === 0 ? 0 : 2));
  const [hoverMode, setHoverMode] = useState<"off" | "hover" | "shift">(() => readStored("ddreams.hover", "shift"));
  const [thumbnailInfo, setThumbnailInfo] = useState<ThumbnailInfoConfig>(() =>
    normalizeThumbnailInfo(readStored("ddreams.thumbnailInfo", DEFAULT_THUMBNAIL_INFO)),
  );
  const [thumbnailInfoOpen, setThumbnailInfoOpen] = useState(false);
  const [filters, setFilters] = useState<ViewFilters>({});
  const [showOtherFiles, setShowOtherFiles] = useState(() => readStored("ddreams.showOtherFiles", false));
  const series = useSeriesSearchSettings();
  useEffect(() => writeStored("ddreams.showOtherFiles", showOtherFiles), [showOtherFiles]);
  const [uiScale, setUiScale] = useState(() => readStored("ddreams.uiScale", 1));
  useEffect(() => writeStored("ddreams.uiScale", uiScale), [uiScale]);
  // What the backend gets: by default only photos/RAW/video ("media"); "all" from Filtre shows everything.
  const queryFilters = useMemo(() => effectiveViewFilters(filters, showOtherFiles), [filters, showOtherFiles]);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const incoming = useIncomingFolder();
  const { incomingFolder } = incoming;
  const [includeChildren, setIncludeChildren] = useState<boolean>(() => readStored("ddreams.includeChildren", true));
  const [availableLabels, setAvailableLabels] = useState<ValueCount[]>([]);
  const selection = useGridSelectionState();
  const { selectedIds, setSelectedIds, setPrimaryId, setPrimaryAsset, primaryId, primaryAsset } = selection;
  const [total, setTotal] = useState(0);
  const [totalSize, setTotalSize] = useState(0);
  const [toast, setToast] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const historyRef = useRef<HTMLDivElement>(null);
  const [refreshTick, setRefreshTick] = useState(0);
  const [calendarCatalogueTick, setCalendarCatalogueTick] = useState(0);
  useEffect(() => {
    const subscription = listen("folder-catalogue-changed", () => setCalendarCatalogueTick((tick) => tick + 1));
    return () => {
      void subscription.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);
  const suggestions = useDeletionSuggestions({ refreshTick });
  const { deletionSuggestions } = suggestions;
  const grid = useRef<AssetGridHandle>(null);
  const viewerState = useViewer({ setToast, grid });
  const {
    viewer,
    setViewer,
    comparison,
    setComparison,
    closeComparison,
    restoreGridFocus,
    openComparison,
    rawDecode,
    setRawDecode,
    showFaces,
    setShowFaces,
  } = viewerState;
  const assetValueState = useAssetValueState({ historyOpen, refreshTick, selection });
  const { valueAliases, valueTokens, primaryValueEdit, selectedList, selectedReadIds, primaryReadId } = assetValueState;
  const [formats, setFormats] = useState<FormatTable>({ version: 0, formats: [] });
  useEffect(() => {
    const timer = window.setTimeout(() => setQueryDebounced(query), 180);
    return () => window.clearTimeout(timer);
  }, [query]);
  const navigation = useNavigation({
    setQuery,
    setQueryDebounced,
    setZoom,
    setFilters,
    showOtherFiles,
    setToast,
    selection,
    viewerState,
  });
  const { view, viewResolved, navigate, openMapNeighborhood } = navigation;
  useEffect(() => {
    void invoke<FormatTable>("get_formats")
      .then(setFormats)
      .catch((error) => console.error("Format table failed to load", error));
  }, []);
  useEffect(() => {
    let active = true;
    void invoke<ValueCount[]>("get_catalog_values", { kind: "label", offset: 0, limit: 500 })
      .then((labels) => {
        if (active) setAvailableLabels(labels);
      })
      .catch(() => {
        if (active) setAvailableLabels([]);
      });
    return () => {
      active = false;
    };
  }, [refreshTick, valueTokens.label]);
  useEffect(() => writeStored("ddreams.zoom", zoom), [zoom]);
  useEffect(() => writeStored("ddreams.overlay", overlayMode), [overlayMode]);
  useEffect(() => writeStored("ddreams.hover", hoverMode), [hoverMode]);
  useEffect(() => writeStored("ddreams.thumbnailInfo", thumbnailInfo), [thumbnailInfo]);
  useEffect(() => writeStored("ddreams.includeChildren", includeChildren), [includeChildren]);
  const aiSettings = useAiSettings();
  const { aiStatus, aiEnabled, aiReady, aiTick } = aiSettings;
  const viewerImage = useViewerImage({ uiScale, refreshTick, valueAliases, formats, aiSettings, viewerState });
  const { faceAiAvailable, faceAiReason, facesByAsset } = viewerImage;
  useAiStatus({ navigation, aiSettings });
  const sortKey = sortKeyForView(view);
  const { sort, descending } = readViewSort(savedSorts, sortKey);
  const editPendingView = view.kind === "special" && view.value === "edit_pending";
  const changeSort = (nextSort: string, nextDescending: boolean) => {
    const saved = updateViewSort(savedSorts, sortKey, nextSort, nextDescending);
    setSavedSorts(saved);
    writeStored("ddreams.sorts", saved);
  };
  const assetFilter = useMemo(
    () => buildAssetFilter(view, queryDebounced, sort, descending, queryFilters, includeChildren),
    [view, queryDebounced, sort, descending, queryFilters, includeChildren],
  );
  const currentAssetFilter = useRef(assetFilter);
  currentAssetFilter.current = assetFilter;
  const [ready, setReady] = useState(false);
  const review = useQuarantineReview({ appInfo, setToast, ready, incoming });
  const {
    quarantineSettings,
    quarantineReview,
    setQuarantineReview,
    reviewInGrid,
    setReviewInGrid,
    quarantineBusy,
    setQuarantineConfirm,
  } = review;
  const markReady = useCallback((why: string) => {
    setReady((was) => {
      if (!was) void invoke("startup_mark", { label: `ready (${why})` }).catch(() => {});
      return true;
    });
  }, []);
  const counts = useCallback(
    (n: number, size: number) => {
      setTotal(n);
      setTotalSize(size);
      if (viewResolved.current) markReady("first page");
    },
    [markReady],
  );
  useEffect(() => {
    void invoke("startup_mark", { label: "ui mounted" }).catch(() => {});
    const timer = window.setTimeout(() => markReady("timeout 20 s"), 20000);
    return () => window.clearTimeout(timer);
  }, [markReady]);
  useAiStartup({ ready, aiSettings });
  const gridSelection = useGridSelection({ assetFilter, selection });
  const { select, selectRange, selectAll } = gridSelection;
  const listing = useListingRefresh({ view, setRefreshTick, grid });
  const { epoch, treeRefreshTick, treeRefreshPaths, updateViewport, refreshAfterWrite } = listing;
  const deletion = useDeletionActions({ appInfo, setToast, refreshAfterWrite });
  const { runDeletionAction } = deletion;
  const assetValues = useAssetValues({
    appInfo,
    setToast,
    setRefreshTick,
    grid,
    currentAssetFilter,
    navigation,
    selection,
    viewerState,
    assetValueState,
    listing,
  });
  const {
    setAssetValues,
    setAssetColors,
    setAssetCategories,
    keywordStructureChanged,
    setAssetKeywords,
    createAndAssignKeyword,
    categoryStructureChanged,
    categoryWriteError,
  } = assetValues;
  const { applyingSuggestion, applySuggestion } = useAiSuggestion({ navigation, assetValues });
  const copyCurrentResult = useCallback(async () => {
    let lines: string[] = [];
    for (let offset = 0; offset < total; offset += 500) {
      const p = await invoke<Page>("get_assets", { filter: { ...assetFilter, offset, limit: 500 } });
      for (const a of p.items)
        if (!a.isDirectory)
          lines.push(
            [
              a.name,
              a.size ?? "",
              a.ext,
              a.fileModified ?? "",
              a.width && a.height ? `${a.width} × ${a.height}` : "",
              a.caption ?? "",
              a.rating ?? "",
            ].join("\t"),
          );
      if (p.items.length < 500) break;
    }
    await navigator.clipboard.writeText(lines.join("\n"));
    setToast(`Skopírované: ${slovakItems(lines.length)}`);
    window.setTimeout(() => setToast(""), 1600);
  }, [assetFilter, total]);
  const viewerAssets = useViewerAssets({ formats, currentAssetFilter, selection, viewerState });
  const { showViewerAsset, openComparedPhoto } = viewerAssets;
  const files = useFileOperations({ appInfo, setToast, navigation, selection, viewerState, listing, viewerAssets });
  const {
    cutIds,
    fileClipboard,
    folderClipboard,
    operationProgress,
    beginFileOperation,
    dropFiles,
    folderAction,
    createFolder,
    changeFolderRule,
  } = files;
  const quarantine = useQuarantine({
    appInfo,
    setCenterMode,
    setToast,
    files,
    review,
    selection,
    viewerState,
    navigation,
    listing,
    viewerAssets,
  });
  const { requestQuarantineMove, requestQuarantineDelete } = quarantine;
  const iptc = useIptc({ appInfo, setToast, files, assetValueState, listing, assetValues });
  const { iptcToken, editIptc, openIptcWrite } = iptc;
  useEffect(() => {
    if (!historyOpen) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !historyRef.current?.contains(event.target)) setHistoryOpen(false);
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setHistoryOpen(false);
      }
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", escape);
    };
  }, [historyOpen]);
  const ai = useAi({ setToast, assetFilter, navigation, series, aiSettings });
  const { findSimilar } = ai;
  useKeyboardShortcuts({
    editors,
    openEditor,
    setZoom,
    setOverlayMode,
    copyCurrentResult,
    navigation,
    selection,
    files,
    viewerState,
    listing,
    assetValues,
    gridSelection,
  });
  const viewerControls = useViewerControls({
    uiScale,
    epoch,
    total,
    setToast,
    formats,
    assetFilter,
    showViewerAsset,
    viewerState,
    viewerImage,
  });
  return (
    <main className="app-shell" style={{ zoom: uiScale }}>
      <WindowTopBar
        slovakItems={slovakItems}
        appInfo={appInfo}
        catalogTooltip={catalogTooltip}
        query={query}
        setQuery={setQuery}
        dockLayout={dockLayout}
        setDockLayout={setDockLayout}
        filters={filters}
        setFilters={setFilters}
        setSettingsOpen={setSettingsOpen}
        total={total}
        historyOpen={historyOpen}
        setHistoryOpen={setHistoryOpen}
        historyRef={historyRef}
        help={help}
        listing={listing}
        aiSettings={aiSettings}
        navigation={navigation}
        ai={ai}
      />
      <div className="workspace" inert={!!comparison}>
        <LeftPanel
          hosts={dockHosts}
          onNavigate={navigate}
          activeFolderId={view.kind === "folder" ? (view.id ?? null) : null}
          activeFolderIds={view.kind === "folder" ? view.ids : []}
          activeView={view}
          calendarRefreshToken={refreshTick + calendarCatalogueTick}
          refreshToken={treeRefreshTick}
          refreshPaths={treeRefreshPaths}
          onFileDrop={dropFiles}
          onCreateFolder={createFolder}
          onFolderRule={(path, mode) => void changeFolderRule(path, mode)}
          onFolderAction={(action, path) => void folderAction(action, path)}
          clipboardPath={folderClipboard?.mode === "move" ? folderClipboard.path : null}
          clipboardAvailable={!!folderClipboard || !!fileClipboard}
          onToast={(message) => {
            setToast(message);
            window.setTimeout(() => setToast(""), 3000);
          }}
        />
        <DockSections
          hosts={dockHosts}
          setLayout={setDockLayout}
          controls={dockControls}
          collapsed={dockLayout.collapsed}
          setPresence={setDockPresence}
        >
          <RightPanel
            iptcToken={iptcToken}
            onEditIptc={editIptc}
            onOpenKeywords={openKeywordAssignment}
            primaryReadId={primaryReadId}
            selectedReadIds={selectedReadIds}
            labelCounts={availableLabels}
            valueTokens={valueTokens}
            primaryValues={
              primaryValueEdit?.refreshTick === refreshTick && primaryValueEdit.patch.id === primaryId
                ? primaryValueEdit.patch
                : undefined
            }
            incomingTools={
              <IncomingTools
                folder={incomingFolder}
                ids={selectedList}
                aiReady={aiReady}
                aiReason={!aiEnabled ? "AI je vypnutá" : (aiStatus.reason ?? aiStatus.lastError ?? "AI sa pripravuje")}
                readOnly={!!appInfo?.readonly}
                indexRevision={aiStatus.state === "indexing" ? 0 : aiStatus.indexed}
                refreshToken={refreshTick}
                onMove={(path) => void dropFiles(selectedList, path, false)}
                onShowPhotos={(ids, title, groups) => {
                  setCenterMode("grid");
                  navigate({ kind: "ai", ids, title, groups });
                }}
                onMoveGroup={(ids, path) => void dropFiles(ids, path, false)}
                onChooseFolder={() => beginFileOperation(selectedList, "move")}
                onAnalyze={() => runDeletionAction("analyze_incoming_series")}
              />
            }
            thumbEpoch={epoch}
            catalogReady={ready}
            activeColors={view.kind === "color" ? (view.value ?? "") : ""}
            activeQuality={view.kind === "quality" ? (view.value ?? "") : ""}
            primaryId={primaryId}
            selectedIds={selectedList}
            includeChildren={includeChildren}
            onIncludeChildrenChange={setIncludeChildren}
            onNavigate={navigate}
            onSetValues={setAssetValues}
            onSetColors={setAssetColors}
            aiAvailable={aiStatus.available}
            aiReady={aiReady}
            aiReason={aiStatus.reason ?? "AI modul nie je nainštalovaný"}
            onSetCategory={setAssetCategories}
            onCategoryChanged={categoryStructureChanged}
            onRefresh={refreshAfterWrite}
            onClusterNotice={(message) => {
              setToast(message);
              window.setTimeout(() => setToast(""), 3500);
            }}
            onSetKeyword={setAssetKeywords}
            onCreateAssignKeyword={createAndAssignKeyword}
            onKeywordChanged={keywordStructureChanged}
            onCategoryError={categoryWriteError}
            readOnly={!!appInfo?.readonly}
            refreshToken={refreshTick}
            colorToken={aiTick}
            catalogRefreshToken={refreshTick}
          />
        </DockSections>
        <DockSide
          side="left"
          layout={dockLayout}
          setLayout={setDockLayout}
          hosts={dockHosts}
          hideNav={!!viewer}
          dragging={dockDragging}
          controls={dockControls}
          sectionPresence={dockPresence}
        />
        <section className="center-pane">
          <WindowToolbar
            seriesMenu={seriesMenu}
            filtersMenu={filtersMenu}
            displayMenu={displayMenu}
            zoom={zoom}
            setZoom={setZoom}
            centerMode={centerMode}
            setCenterMode={setCenterMode}
            overlayMode={overlayMode}
            setOverlayMode={setOverlayMode}
            hoverMode={hoverMode}
            setHoverMode={setHoverMode}
            setThumbnailInfoOpen={setThumbnailInfoOpen}
            filters={filters}
            setFilters={setFilters}
            availableLabels={availableLabels}
            sort={sort}
            descending={descending}
            editPendingView={editPendingView}
            changeSort={changeSort}
            help={help}
            navigation={navigation}
            series={series}
            aiSettings={aiSettings}
            listing={listing}
            ai={ai}
          />
          {view.suggestion && (
            <div className="selection-bar ai-review-actions">
              <strong>Priradiť do {view.suggestion.path}:</strong>
              <button
                disabled={applyingSuggestion || !!appInfo?.readonly || !view.ids?.length}
                onClick={() => void applySuggestion(view.ids ?? [])}
              >
                Všetkým ({view.ids?.length ?? 0})
              </button>
              <button
                disabled={applyingSuggestion || !!appInfo?.readonly || !selectedIds.size}
                onClick={() => void applySuggestion([...selectedIds])}
              >
                Vybraným ({selectedIds.size})
              </button>
              <button
                disabled={!selectedIds.size}
                onClick={() => {
                  const ids = [...selectedIds];
                  void invoke("ai_reject_suggestion", {
                    assetIds: ids,
                    kind: view.suggestion!.kind,
                    labelId: view.suggestion!.labelId,
                  }).finally(() => refreshAfterWrite());
                  const remaining = (view.ids ?? []).filter((id) => !selectedIds.has(id));
                  navigate({ ...view, ids: remaining });
                  setToast(`Odmietnuté: ${ids.length}`);
                }}
              >
                Odmietnuť vybrané
              </button>
            </div>
          )}
          {view.kind === "series" && view.seriesInfo && (
            <div className="selection-bar ai-review-actions series-review-actions">
              <strong>
                {view.seriesInfo.count} sérií · {view.ids?.length ?? 0} fotiek
              </strong>
              <button
                disabled={!!appInfo?.readonly || !selectedIds.size}
                onClick={() => {
                  const selectedGroups = new Set(
                    (view.groups ?? []).filter((_, index) => selectedIds.has(view.ids?.[index] ?? -1)),
                  );
                  const rejected = (view.ids ?? []).filter(
                    (id, index) => selectedGroups.has(view.groups?.[index] ?? -1) && !selectedIds.has(id),
                  );
                  if (rejected.length) void setAssetValues(rejected, { label: "Gray" });
                }}
              >
                Ponechať vybrané, zvyšok série vyradiť
              </button>
              <button
                disabled={!!appInfo?.readonly}
                onClick={() => {
                  const best = new Set(view.bestIds ?? []);
                  const rejected = (view.ids ?? []).filter((id) => !best.has(id));
                  if (
                    rejected.length &&
                    window.confirm(`Vyradiť ${rejected.length} fotiek a ponechať najlepšiu z každej série?`)
                  )
                    void setAssetValues(rejected, { label: "Gray" });
                }}
              >
                Vyradiť všetko okrem najlepších
              </button>
              <small>X = vyradiť / vrátiť vybrané</small>
            </div>
          )}
          {(selectedList.some((id) => deletionSuggestions.has(id)) ||
            (view.kind === "special" && view.value === "delete_suggested")) && (
            <div className="selection-bar ai-review-actions">
              <button
                disabled={!!appInfo?.readonly || quarantineBusy || !deletionSuggestions.size}
                onClick={() =>
                  setQuarantineConfirm({
                    ids: selectedList.some((id) => deletionSuggestions.has(id))
                      ? selectedList.filter((id) => deletionSuggestions.has(id))
                      : [...deletionSuggestions],
                    action: "move",
                    suggestionsOnly: true,
                  })
                }
              >
                Presunúť návrhy do karantény
              </button>
              {selectedList.some((id) => deletionSuggestions.has(id)) && (
                <button
                  disabled={!!appInfo?.readonly}
                  onClick={() =>
                    void runDeletionAction(
                      "keep_suggested_assets",
                      selectedList.filter((id) => deletionSuggestions.has(id)),
                    )
                  }
                >
                  Odmietnuť návrh na vymazanie
                </button>
              )}
              {view.kind === "special" && view.value === "delete_suggested" && (
                <button
                  disabled={!!appInfo?.readonly || !deletionSuggestions.size}
                  onClick={() => void runDeletionAction("keep_suggested_assets")}
                >
                  Zrušiť všetky návrhy
                </button>
              )}
            </div>
          )}
          {view.kind === "special" && view.value === "quarantine" && (
            <div className="selection-bar ai-review-actions">
              <button
                disabled={!!appInfo?.readonly || quarantineBusy || !selectedList.length}
                onClick={() => setQuarantineConfirm({ ids: selectedList, action: "restore" })}
              >
                Vrátiť späť
              </button>
              <button
                disabled={!!appInfo?.readonly || quarantineBusy || !selectedList.length}
                onClick={() => void requestQuarantineDelete(selectedList)}
              >
                Zmazať teraz
              </button>
            </div>
          )}
          {quarantineReview && reviewInGrid && (
            <div className="selection-bar ai-review-actions">
              <button onClick={() => setReviewInGrid(false)}>Späť ku kontrole importu a karantény</button>
              <button
                onClick={() => {
                  setQuarantineReview(null);
                  setReviewInGrid(false);
                }}
              >
                Teraz nie
              </button>
            </div>
          )}
          {centerMode === "map" ? (
            <MapView
              epoch={epoch}
              selectedIds={selectedIds}
              onSelect={select}
              onNavigate={(ids, title) => navigate({ kind: "people", ids, title })}
              onGrid={() => setCenterMode("grid")}
              onOpenNeighborhood={(ids) => void openMapNeighborhood(ids)}
            />
          ) : (
            <>
              <AssetGrid
                ref={grid}
                view={view}
                quarantineDeleteDays={quarantineSettings?.deleteAfterDays}
                onQuarantine={requestQuarantineMove}
                deletionSuggestions={deletionSuggestions}
                onKeepSuggested={(id) => void runDeletionAction("keep_suggested_assets", [id])}
                qualityToken={aiTick}
                query={queryDebounced}
                sort={sort}
                descending={descending}
                zoom={zoom}
                epoch={epoch}
                refreshToken={refreshTick}
                selectedIds={selectedIds}
                primaryId={primaryId}
                cutIds={cutIds}
                filters={queryFilters}
                overlayMode={overlayMode}
                hoverMode={hoverMode}
                includeChildren={includeChildren}
                formats={formats}
                thumbnailInfo={thumbnailInfo}
                onOpenThumbnailInfo={() => setThumbnailInfoOpen(true)}
                onSetValues={setAssetValues}
                onFileOperation={beginFileOperation}
                onFindSimilar={(asset) => void findSimilar(asset)}
                aiReady={aiReady}
                aiReason={aiStatus.reason ?? "AI modul nie je nainštalovaný"}
                readOnly={!!appInfo?.readonly}
                onSelect={select}
                onSelectRange={selectRange}
                onSelectAll={(ids) => void selectAll(ids)}
                onCompare={openComparison}
                onWriteIptc={(ids) => void openIptcWrite(ids)}
                comparisonOpen={!!comparison}
                onOpen={(asset, index) => {
                  if (asset.isDirectory) {
                    const id = asset.folderId ?? asset.id;
                    navigate({
                      kind: "folder",
                      id,
                      ids: asset.folderId ? [asset.folderId] : undefined,
                      title: asset.path ?? asset.name,
                      folderPath: asset.path ?? undefined,
                    });
                    return;
                  }
                  setViewer({ asset, index });
                  setPrimaryId(asset.id);
                  setPrimaryAsset(asset);
                  setSelectedIds(new Set([asset.id]));
                }}
                onCounts={counts}
                onViewport={updateViewport}
              />
            </>
          )}
          {operationProgress && (
            <div className="grid-footer">
              <span>{operationProgress}</span>
            </div>
          )}
          <ViewerOverlay
            editors={editors}
            openEditor={openEditor}
            appInfo={appInfo}
            uiScale={uiScale}
            total={total}
            refreshTick={refreshTick}
            formats={formats}
            assetFilter={assetFilter}
            review={review}
            listing={listing}
            aiSettings={aiSettings}
            suggestions={suggestions}
            viewerState={viewerState}
            viewerImage={viewerImage}
            deletion={deletion}
            assetValues={assetValues}
            iptc={iptc}
            viewerAssets={viewerAssets}
            quarantine={quarantine}
            files={files}
            viewerControls={viewerControls}
            ai={ai}
          />
        </section>
        <DockSide
          side="right"
          layout={dockLayout}
          setLayout={setDockLayout}
          hosts={dockHosts}
          hideNav={!!viewer}
          dragging={dockDragging}
          controls={dockControls}
          sectionPresence={dockPresence}
        />
      </div>
      <footer className="status-bar">
        <span>
          Spolu {slovakItems(total)} · {formatBytes(totalSize)}
        </span>
        {primaryAsset ? (
          <>
            <div className="selected-name">
              {selectedIds.size > 1 ? `${selectedIds.size} vybraných` : primaryAsset.name}
            </div>
            <span>{formatBytes(primaryAsset.size)}</span>
            <span>{dateText(primaryAsset.exifDate)}</span>
            <span>
              {primaryAsset.width && primaryAsset.height ? `${primaryAsset.width} × ${primaryAsset.height}` : "—"}
            </span>
          </>
        ) : null}
      </footer>
      {comparison && (
        <CompareView
          photos={comparison}
          initialActive={primaryId}
          epoch={epoch}
          qualityToken={aiTick}
          uiScale={uiScale}
          formats={formats}
          rawDecode={rawDecode}
          onRawDecode={setRawDecode}
          showFaces={showFaces}
          onShowFaces={setShowFaces}
          facesByAsset={facesByAsset}
          faceAiAvailable={faceAiAvailable}
          faceAiReason={faceAiReason}
          editors={editors}
          readOnly={!!appInfo?.readonly}
          aiReady={aiReady}
          aiReason={aiStatus.reason ?? "AI modul nie je nainštalovaný"}
          onClose={closeComparison}
          onRestoreFocus={restoreGridFocus}
          onDrop={(key) =>
            setComparison((current) =>
              current && current.length > 2 ? current.filter((photo) => photo.key !== key) : current,
            )
          }
          onOpen={openComparedPhoto}
          onSetValues={setAssetValues}
          onFileOperation={beginFileOperation}
          onQuarantine={requestQuarantineMove}
          onFindSimilar={(asset) => {
            closeComparison();
            void findSimilar(asset);
          }}
        />
      )}
      <WindowDialogs
        slovakItems={slovakItems}
        appInfo={appInfo}
        thumbnailInfo={thumbnailInfo}
        setThumbnailInfo={setThumbnailInfo}
        thumbnailInfoOpen={thumbnailInfoOpen}
        setThumbnailInfoOpen={setThumbnailInfoOpen}
        showOtherFiles={showOtherFiles}
        setShowOtherFiles={setShowOtherFiles}
        uiScale={uiScale}
        setUiScale={setUiScale}
        settingsOpen={settingsOpen}
        setSettingsOpen={setSettingsOpen}
        toast={toast}
        help={help}
        navigation={navigation}
        incoming={incoming}
        files={files}
        review={review}
        aiSettings={aiSettings}
        iptc={iptc}
        quarantine={quarantine}
        ai={ai}
      />
      <BackupReminder />
    </main>
  );
}

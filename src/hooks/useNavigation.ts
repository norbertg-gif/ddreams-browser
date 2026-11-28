import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
  type RefObject,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { pictureDir } from "@tauri-apps/api/path";
import type { AssetGridHandle } from "../components/AssetGrid";
import type { Folder, Page, View, ViewFilters } from "../types";
import { readStored, writeStored } from "../storage";
import { buildAssetFilter, effectiveViewFilters } from "../assetFilter";
import type { useGridSelectionState } from "./useAssetValues";
import type { useViewer } from "./useViewer";

export type AppInfo = import("../bindings/AppInfo").AppInfo;
type RecentChange = import("../bindings/ChangeSummary").ChangeSummary;

// The last opened folder, or the Windows Pictures folder on the very first start.
async function initialFolderPath(): Promise<string> {
  const stored = readStored<string | null>("ddreams.lastFolder", null);
  if (typeof stored === "string" && stored) return stored;
  return pictureDir().catch(() => "C:\\");
}

type NavigationOptions = {
  setQuery: Dispatch<SetStateAction<string>>;
  setQueryDebounced: Dispatch<SetStateAction<string>>;
  setZoom: Dispatch<SetStateAction<number>>;
  setFilters: Dispatch<SetStateAction<ViewFilters>>;
  showOtherFiles: boolean;
  setToast: Dispatch<SetStateAction<string>>;
  selection: Pick<ReturnType<typeof useGridSelectionState>, "setSelectedIds" | "setPrimaryId" | "setPrimaryAsset">;
  viewerState: Pick<ReturnType<typeof useViewer>, "setViewer">;
};

export function useNavigation({
  setQuery,
  setQueryDebounced,
  setZoom,
  setFilters,
  showOtherFiles,
  setToast,
  selection,
  viewerState,
}: NavigationOptions) {
  const { setSelectedIds, setPrimaryId, setPrimaryAsset } = selection;
  const { setViewer } = viewerState;
  const [view, setView] = useState<View>({ kind: "folder", id: -1, title: "Načítavam katalóg…" });
  const [history, setHistory] = useState<View[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const viewResolved = useRef(0);
  useEffect(() => {
    setZoom(readStored("ddreams.zoom", 168));
    void initialFolderPath().then((path) =>
      invoke<Folder | null>("find_folder_path", { path })
        .then((folder) => {
          const v = folder
            ? {
                kind: "folder" as const,
                id: folder.id,
                ids: folder.folderIds,
                title: folder.path ?? folder.name,
                folderPath: folder.path ?? undefined,
              }
            : { kind: "folder" as const, id: -1, title: path, folderPath: path };
          viewResolved.current = performance.now();
          void invoke("startup_mark", { label: "initial folder resolved" }).catch(() => {});
          setView(v);
          setHistory([v]);
          setHistoryIndex(0);
        })
        .catch(() => {
          viewResolved.current = performance.now();
          const v = { kind: "folder" as const, id: -1, title: path, folderPath: path };
          setView(v);
          setHistory([v]);
          setHistoryIndex(0);
        }),
    );
  }, []);
  const navigate = useCallback(
    (next: View) => {
      setView(next);
      setHistory((h) => {
        const base = h.slice(0, historyIndex + 1);
        base.push(next);
        setHistoryIndex(base.length - 1);
        return base;
      });
      setQuery("");
      setQueryDebounced("");
      setFilters({});
      setSelectedIds(new Set());
      setPrimaryId(null);
      setPrimaryAsset(null);
      setViewer(null);
      if (next.kind === "folder") writeStored("ddreams.lastFolder", next.folderPath ?? next.title);
    },
    [historyIndex],
  );
  const openMapNeighborhood = useCallback(
    async (ids: number[]) => {
      if (!ids.length) return;
      const next: View = { kind: "people", ids, title: "Okolie na mape" };
      navigate(next);
      try {
        const page = await invoke<Page>("get_assets", {
          filter: {
            ...buildAssetFilter(next, "", "name", false, effectiveViewFilters({}, showOtherFiles), false),
            offset: 0,
            limit: 1,
          } satisfies import("../bindings/AssetFilter").AssetFilter,
        });
        const asset = page.items[0];
        if (asset) {
          setViewer({ asset, index: 0 });
          setPrimaryId(asset.id);
          setPrimaryAsset(asset);
          setSelectedIds(new Set([asset.id]));
        }
      } catch (error) {
        setToast(String(error));
      }
    },
    [navigate, showOtherFiles],
  );
  const travel = useCallback(
    (index: number) => {
      const next = history[index];
      if (!next) return;
      setHistoryIndex(index);
      setView(next);
      setQuery("");
      setFilters({});
      setSelectedIds(new Set());
      setPrimaryId(null);
      setPrimaryAsset(null);
      setViewer(null);
    },
    [history],
  );
  const up = useCallback(async () => {
    if (view.kind !== "folder") return;
    const p = view.folderPath ?? view.title;
    const parts = p.split(/[\\/]/).filter(Boolean);
    if (parts.length <= 1) return;
    parts.pop();
    let parent = parts.join("\\");
    if (/^[A-Za-z]:$/.test(parent)) parent += "\\";
    const f = await invoke<Folder | null>("find_folder_path", { path: parent }).catch(() => null);
    if (f)
      navigate({
        kind: "folder",
        id: f.id,
        ids: f.folderIds,
        title: f.path ?? f.name,
        folderPath: f.path ?? undefined,
      });
  }, [view, navigate]);
  const crumbs = useMemo(
    () =>
      view.kind === "folder" ? (view.folderPath ?? view.title).split(/[\\/]/).filter(Boolean) : ["Katalóg", view.title],
    [view],
  );
  const openCrumb = async (index: number) => {
    if (view.kind !== "folder") return;
    const parts = (view.folderPath ?? view.title).split(/[\\/]/).filter(Boolean);
    let path = parts.slice(0, index + 1).join("\\");
    if (index === 0 && /^[A-Za-z]:$/.test(path)) path += "\\";
    const folder = await invoke<Folder | null>("find_folder_path", { path }).catch(() => null);
    if (folder)
      navigate({
        kind: "folder",
        id: folder.id,
        ids: folder.folderIds,
        title: folder.path ?? folder.name,
        folderPath: folder.path ?? undefined,
      });
  };
  return {
    view,
    setView,
    history,
    historyIndex,
    viewResolved,
    navigate,
    openMapNeighborhood,
    travel,
    up,
    crumbs,
    openCrumb,
  };
}

type ListingRefreshOptions = {
  view: View;
  setRefreshTick: Dispatch<SetStateAction<number>>;
  grid: RefObject<AssetGridHandle | null>;
};

export function useListingRefresh({ view, setRefreshTick, grid }: ListingRefreshOptions) {
  const [epoch, setEpoch] = useState(1);
  const [recentChanges, setRecentChanges] = useState<RecentChange[]>([]);
  const [treeRefreshTick, setTreeRefreshTick] = useState(0);
  const [treeRefreshPaths, setTreeRefreshPaths] = useState<string[]>([]);
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
    return () => {
      watchedFolder.current = null;
    };
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
  const refresh = useCallback(() => {
    if (view.kind === "folder" && view.id != null) void invoke("refresh_folder_listing", { folderId: view.id });
    setRefreshTick((v) => v + 1);
    setTreeRefreshPaths([]);
    setTreeRefreshTick((v) => v + 1);
    updateViewport();
  }, [view, updateViewport]);
  const loadRecentChanges = useCallback(() => {
    void invoke<RecentChange[]>("get_recent_changes", { limit: 20 })
      .then(setRecentChanges)
      .catch(() => setRecentChanges([]));
  }, []);
  useEffect(() => {
    loadRecentChanges();
  }, [loadRecentChanges]);
  const refreshAfterWrite = useCallback(
    (refreshTree = false, affectedPaths: string[] = []) => {
      if (view.kind === "folder" && view.id != null) void invoke("refresh_folder_listing", { folderId: view.id });
      setRefreshTick((v) => v + 1);
      if (refreshTree) {
        setTreeRefreshPaths(affectedPaths);
        setTreeRefreshTick((v) => v + 1);
      }
      updateViewport();
      loadRecentChanges();
    },
    [view, updateViewport, loadRecentChanges],
  );
  return {
    epoch,
    recentChanges,
    treeRefreshTick,
    treeRefreshPaths,
    updateViewport,
    refresh,
    loadRecentChanges,
    refreshAfterWrite,
  };
}

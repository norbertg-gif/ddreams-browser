import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FOLDER_RULES_EVENT, type FolderRuleMode } from "../folderRules";
import type { Asset, Folder } from "../types";
import type { AppInfo, useNavigation, useListingRefresh } from "./useNavigation";
import type { useGridSelectionState } from "./useAssetValues";
import type { useViewer, useViewerAssets } from "./useViewer";

export type DeleteInfo = import("../bindings/DeleteInfo").DeleteInfo;

type ExclusionImpact = import("../bindings/ExclusionImpact").ExclusionImpact;
type RemoveCatalogInfo = import("../bindings/RemoveCatalogInfo").RemoveCatalogInfo;
const folderParentPath = (input: string) => {
  const clean = input.replace(/[\\/]+$/g, "");
  if (clean.startsWith("\\\\")) {
    const parts = clean.slice(2).split(/[\\/]+/);
    return parts.length <= 2 ? "" : "\\\\" + parts.slice(0, -1).join("\\");
  }
  const parts = clean.split(/[\\/]+/);
  if (parts.length <= 1) return "";
  const parent = parts.slice(0, -1).join("\\");
  return /^[A-Za-z]:$/.test(parent) ? parent + "\\" : parent;
};
type RemoveCatalogResult = import("../bindings/RemoveCatalogResult").RemoveCatalogResult;

type FileOperationsOptions = {
  appInfo: AppInfo | null;
  setToast: Dispatch<SetStateAction<string>>;
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "navigate">;
  selection: Pick<
    ReturnType<typeof useGridSelectionState>,
    "setSelectedIds" | "setPrimaryId" | "setPrimaryAsset" | "primaryId" | "primaryAsset"
  >;
  viewerState: Pick<ReturnType<typeof useViewer>, "closeIfCompared">;
  listing: Pick<ReturnType<typeof useListingRefresh>, "refreshAfterWrite">;
  viewerAssets: Pick<ReturnType<typeof useViewerAssets>, "advanceAfterRemoval">;
};

export function useFileOperations({
  appInfo,
  setToast,
  navigation,
  selection,
  viewerState,
  listing,
  viewerAssets,
}: FileOperationsOptions) {
  const { view, navigate } = navigation;
  const { setSelectedIds, setPrimaryId, setPrimaryAsset, primaryId, primaryAsset } = selection;
  const { closeIfCompared } = viewerState;
  const { refreshAfterWrite } = listing;
  const { advanceAfterRemoval } = viewerAssets;
  const [deleteConfirm, setDeleteConfirm] = useState<{
    ids: number[];
    name: string | null;
    permanent: boolean;
    quarantine?: boolean;
    info: DeleteInfo | null;
  } | null>(null);
  const [excludeConfirm, setExcludeConfirm] = useState<{ path: string; impact: ExclusionImpact } | null>(null);
  const [removeCatalogConfirm, setRemoveCatalogConfirm] = useState<{ path: string; info: RemoveCatalogInfo } | null>(
    null,
  );
  const [fileOp, setFileOp] = useState<{ ids: number[]; mode: "move" | "copy" } | null>(null);
  const [nameOp, setNameOp] = useState<
    { kind: "rename"; assetId: number; name: string; extension: string } | { kind: "folder"; parentPath: string } | null
  >(null);
  const [fileConflict, setFileConflict] = useState<{
    ids: number[];
    mode: "move" | "copy";
    path: string;
    count: number;
  } | null>(null);
  const [moveWarning, setMoveWarning] = useState<{
    ids: number[];
    path: string;
    conflict: import("../bindings/ConflictPolicy").ConflictPolicy;
    count: number;
  } | null>(null);
  const [cutIds, setCutIds] = useState<number[]>([]);
  const [fileClipboard, setFileClipboard] = useState<{ ids: number[]; mode: "move" | "copy" } | null>(null);
  const [folderClipboard, setFolderClipboard] = useState<{ path: string; mode: "move" | "copy" } | null>(null);
  const [folderRenameOp, setFolderRenameOp] = useState<{ path: string; name: string } | null>(null);
  const [folderDeleteConfirm, setFolderDeleteConfirm] = useState<{
    path: string;
    info: import("../bindings/DeleteFolderInfo").DeleteFolderInfo;
  } | null>(null);
  const [operationProgress, setOperationProgress] = useState("");
  const folderActionRef = useRef<((action: "rename" | "delete", path: string) => Promise<void>) | null>(null);
  const beginFileOperation = useCallback(
    (ids: number[], mode: "move" | "copy" | "rename" | "delete" | "delete-permanent", tileAsset?: Asset) => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        return;
      }
      {
        const target = tileAsset ?? (ids.length === 1 && primaryAsset?.id === ids[0] ? primaryAsset : null);
        if (target?.isDirectory && target.path && (mode === "delete" || mode === "rename")) {
          void folderActionRef.current?.(mode, target.path);
          return;
        }
        if (target?.isDirectory && mode === "delete-permanent") {
          setToast("Priečinky sa dajú odstrániť len do Koša");
          window.setTimeout(() => setToast(""), 3000);
          return;
        }
      }
      if (mode === "delete" || mode === "delete-permanent") {
        if (!ids.length) return;
        const permanent = mode === "delete-permanent";
        void invoke<DeleteInfo>("get_delete_info", { assetIds: ids, permanent })
          .then((info) =>
            setDeleteConfirm({
              ids,
              name: ids.length === 1 ? ((tileAsset ?? primaryAsset)?.name ?? null) : null,
              permanent,
              info,
            }),
          )
          .catch((error) => {
            setToast(String(error));
            window.setTimeout(() => setToast(""), 4000);
          });
        return;
      }
      if (mode === "rename") {
        const asset = tileAsset ?? primaryAsset;
        if (!asset) return;
        const old = asset.name;
        const base =
          asset.ext && old.toLowerCase().endsWith(`.${asset.ext.toLowerCase()}`)
            ? old.slice(0, -asset.ext.length - 1)
            : old;
        setNameOp({ kind: "rename", assetId: ids[0], name: base, extension: asset.ext });
        return;
      }
      setFileOp({ ids, mode });
    },
    [appInfo, primaryAsset],
  );
  const runFileOperation = useCallback(
    async (
      ids: number[],
      mode: "move" | "copy",
      path: string,
      conflict: import("../bindings/ConflictPolicy").ConflictPolicy,
      confirmed = false,
    ) => {
      try {
        if (mode === "move" && !confirmed) {
          const count = await invoke<number>("get_uncategorized_photos_for_move", {
            assetIds: ids,
            targetFolderPath: path,
            conflict,
          });
          if (count) {
            setFileConflict(null);
            setMoveWarning({ ids, path, conflict, count });
            return;
          }
        }
        const result = await invoke<import("../bindings/FileOperationResult").FileOperationResult>(
          mode === "move" ? "move_files" : "copy_files",
          {
            assetIds: ids,
            targetFolderPath: path,
            conflict,
          },
        );
        setFileConflict(null);
        setOperationProgress("");
        setToast(result.summary);
        setCutIds([]);
        if (mode === "move") {
          setFileClipboard(null);
          if (result.changedCount > 0) closeIfCompared(result.affectedIds ?? []);
        }
        if (result.changedCount > 0)
          refreshAfterWrite(true, result.affectedFolderPaths ?? [view.folderPath, path].filter(Boolean));
        if (
          mode === "move" &&
          result.changedCount > 0 &&
          view.kind === "folder" &&
          path.toLowerCase() !== (view.folderPath ?? "").toLowerCase()
        ) {
          setSelectedIds((prev) => new Set([...prev].filter((id) => !ids.includes(id))));
          if (primaryId != null && ids.includes(primaryId)) {
            setPrimaryId(null);
            setPrimaryAsset(null);
          }
        }
        window.setTimeout(() => setToast(""), result.changedCount ? 6000 : 2400);
      } catch (error) {
        setOperationProgress("");
        setToast(String(error));
        window.setTimeout(() => setToast(""), 4000);
      } finally {
        setOperationProgress("");
      }
    },
    [refreshAfterWrite, view, primaryId, closeIfCompared],
  );
  const chooseOperationTarget = useCallback(
    async (path: string) => {
      if (!fileOp) return;
      try {
        const count = await invoke<number>("get_file_operation_conflicts", {
          assetIds: fileOp.ids,
          targetFolderPath: path,
        });
        const pending = { ...fileOp, path, count };
        setFileOp(null);
        if (count) setFileConflict(pending);
        else void runFileOperation(fileOp.ids, fileOp.mode, path, "cancel");
      } catch (error) {
        setToast(String(error));
        setFileOp(null);
      }
    },
    [fileOp, runFileOperation],
  );
  const dropFiles = useCallback(
    async (ids: number[], path: string, copy: boolean) => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        return;
      }
      const mode = copy ? "copy" : "move";
      try {
        const count = await invoke<number>("get_file_operation_conflicts", { assetIds: ids, targetFolderPath: path });
        if (count) setFileConflict({ ids, mode, path, count });
        else void runFileOperation(ids, mode, path, "cancel");
      } catch (error) {
        setToast(String(error));
      }
    },
    [appInfo, runFileOperation],
  );
  const folderAction = useCallback(
    async (action: "reveal" | "copyPath" | "rename" | "delete" | "cut" | "copy" | "paste", path: string) => {
      if (action === "copyPath") {
        await navigator.clipboard.writeText(path);
        setToast("Cesta skopírovaná");
        window.setTimeout(() => setToast(""), 1800);
        return;
      }
      if (action === "reveal") {
        try {
          await invoke("reveal_in_explorer", { path });
        } catch (error) {
          setToast(String(error));
        }
        return;
      }
      if (action === "cut" || action === "copy") {
        setFolderClipboard({ path, mode: action === "cut" ? "move" : "copy" });
        setFileClipboard(null);
        setCutIds([]);
        return;
      }
      if (action === "delete") {
        try {
          const info = await invoke<import("../bindings/DeleteFolderInfo").DeleteFolderInfo>("delete_folder_info", {
            path,
          });
          if (!info.recycleAvailable) {
            setToast("Na tomto disku nie je dostupný Kôš");
            return;
          }
          setFolderDeleteConfirm({ path, info });
        } catch (error) {
          setToast(String(error));
        }
        return;
      }
      if (action === "rename") {
        const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
        setFolderRenameOp({ path, name });
        return;
      }
      if (action === "paste" && fileClipboard) {
        void dropFiles(fileClipboard.ids, path, fileClipboard.mode === "copy");
        return;
      }
      if (action === "paste" && folderClipboard) {
        try {
          const result = await invoke<import("../bindings/FileOperationResult").FileOperationResult>(
            folderClipboard.mode === "move" ? "move_folder" : "copy_folder",
            {
              path: folderClipboard.path,
              target: path,
            },
          );
          setToast(result.summary);
          if (folderClipboard.mode === "move") {
            const newPath = (result.affectedFolderPaths as string[] | undefined)?.[1];
            if (newPath)
              window.dispatchEvent(
                new CustomEvent("ddreams-folder-path-changed", { detail: { oldPath: folderClipboard.path, newPath } }),
              );
            setFolderClipboard(null);
          }
          refreshAfterWrite(true, result.affectedFolderPaths ?? [path, folderClipboard.path]);
        } catch (error) {
          setToast(String(error));
        }
      }
    },
    [folderClipboard, fileClipboard, dropFiles, refreshAfterWrite],
  );
  folderActionRef.current = folderAction;
  const submitFolderRename = useCallback(
    async (name: string) => {
      if (!folderRenameOp) return;
      const { path } = folderRenameOp;
      setFolderRenameOp(null);
      try {
        const result = await invoke<import("../bindings/FolderRenameResult").FolderRenameResult>("rename_folder", {
          path,
          newName: name,
        });
        setToast(result.summary);
        const newPath = result.newPath as string | undefined;
        if (newPath)
          window.dispatchEvent(new CustomEvent("ddreams-folder-path-changed", { detail: { oldPath: path, newPath } }));
        refreshAfterWrite(true, newPath ? [path, newPath] : [path]);
        if (newPath && view.kind === "folder" && view.folderPath) {
          const norm = (v: string) => v.replace(/[\\/]+$/g, "").toLowerCase();
          const cur = norm(view.folderPath),
            old = norm(path);
          if (cur === old || cur.startsWith(old + "\\")) {
            const moved = newPath + view.folderPath.slice(path.replace(/[\\/]+$/g, "").length);
            const folder = await invoke<Folder | null>("find_folder_path", { path: moved }).catch(() => null);
            if (folder)
              navigate({
                kind: "folder",
                id: folder.id,
                ids: folder.folderIds,
                title: folder.path ?? folder.name,
                folderPath: folder.path ?? undefined,
              });
          }
        }
        window.setTimeout(() => setToast(""), 6000);
      } catch (error) {
        setToast(String(error));
      }
    },
    [folderRenameOp, refreshAfterWrite, view, navigate],
  );
  const confirmFolderDelete = useCallback(async () => {
    if (!folderDeleteConfirm) return;
    const { path } = folderDeleteConfirm;
    setFolderDeleteConfirm(null);
    try {
      const result = await invoke<import("../bindings/FileOperationResult").FileOperationResult>("delete_folder", {
        path,
      });
      setToast(result.summary);
      refreshAfterWrite(true, result.affectedFolderPaths ?? [path]);
      const current = view.folderPath ?? "";
      const norm = (v: string) => v.replace(/[\\/]+$/g, "").toLowerCase();
      if (current && (norm(current) === norm(path) || norm(current).startsWith(norm(path) + "\\"))) {
        const parent = folderParentPath(path);
        const folder = parent
          ? await invoke<Folder | null>("find_folder_path", { path: parent }).catch(() => null)
          : null;
        if (folder)
          navigate({
            kind: "folder",
            id: folder.id,
            ids: folder.folderIds,
            title: folder.path ?? folder.name,
            folderPath: folder.path ?? undefined,
          });
        else navigate({ kind: "special", value: "all_photos", title: "Všetky fotky" });
      }
      window.setTimeout(() => setToast(""), 6000);
    } catch (error) {
      setToast(String(error));
    }
  }, [folderDeleteConfirm, refreshAfterWrite, view.folderPath, navigate]);
  const runDelete = useCallback(
    async (ids: number[], permanent: boolean, confirmedNoBinDrives: string[]) => {
      setDeleteConfirm(null);
      try {
        const result = await invoke<import("../bindings/FileOperationResult").FileOperationResult>("delete_files", {
          assetIds: ids,
          permanent,
          confirmedNoBinDrives,
        });
        setToast(result.summary);
        const removed = new Set(result.affectedIds);
        setSelectedIds((prev) => new Set([...prev].filter((id) => !removed.has(id))));
        setPrimaryId((id) => (id != null && removed.has(id) ? null : id));
        setPrimaryAsset((asset) => (asset && removed.has(asset.id) ? null : asset));
        refreshAfterWrite(false, result.affectedFolderPaths);
        await advanceAfterRemoval(result.affectedIds);
        window.setTimeout(() => setToast(""), 8000);
      } catch (error) {
        setToast(String(error));
        window.setTimeout(() => setToast(""), 6000);
      }
    },
    [refreshAfterWrite, advanceAfterRemoval],
  );
  const createFolder = useCallback(
    (parentPath: string) => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        return;
      }
      setFileOp(null);
      setNameOp({ kind: "folder", parentPath });
    },
    [appInfo],
  );
  const submitName = useCallback(
    async (name: string) => {
      if (!nameOp) return;
      const result = await invoke<import("../bindings/FileOperationResult").FileOperationResult>(
        nameOp.kind === "rename" ? "rename_file" : "create_folder",
        nameOp.kind === "rename"
          ? { assetId: nameOp.assetId, newBaseName: name }
          : { parentPath: nameOp.parentPath, name },
      );
      if (nameOp.kind === "rename" && result.changedCount > 0) closeIfCompared([nameOp.assetId]);
      setNameOp(null);
      setToast(result.summary);
      setOperationProgress("");
      const paths =
        result.affectedFolderPaths ??
        (nameOp.kind === "folder"
          ? [nameOp.parentPath]
          : primaryAsset?.path
            ? [primaryAsset.path.substring(0, primaryAsset.path.lastIndexOf("\\"))]
            : []);
      refreshAfterWrite(true, paths);
      window.setTimeout(() => setToast(""), 6000);
    },
    [nameOp, primaryAsset, refreshAfterWrite, closeIfCompared],
  );
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<import("../bindings/FileProgress").FileProgress>("file-operation-progress", (event) => {
      if (event.payload.done >= event.payload.total) setOperationProgress("");
      else setOperationProgress(`${event.payload.operation} ${event.payload.done} / ${event.payload.total}…`);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);
  const applyFolderRule = async (path: string, mode: FolderRuleMode | null) => {
    try {
      const removed = await invoke<number>("set_folder_rule", { path, mode });
      window.dispatchEvent(new Event(FOLDER_RULES_EVENT));
      const text =
        mode === "include"
          ? `Zahrnuté do katalógu: ${path}`
          : mode === "exclude"
            ? removed > 0
              ? `Vylúčené z katalógu: ${removed} ${removed === 1 ? "súbor" : removed < 5 ? "súbory" : "súborov"}`
              : `Vylúčené z katalógu: ${path}`
            : `Pravidlo zrušené: ${path}`;
      setToast(text);
      refreshAfterWrite(true, [path]);
      window.setTimeout(() => setToast(""), removed > 0 ? 8000 : 3500);
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 4000);
    }
  };
  const changeFolderRule = async (path: string, mode: FolderRuleMode | null) => {
    if (mode === "exclude") {
      try {
        const impact = await invoke<ExclusionImpact>("get_exclusion_impact", { path });
        if (impact.files > 0) {
          setExcludeConfirm({ path, impact });
          return;
        }
      } catch (error) {
        setToast(String(error));
        window.setTimeout(() => setToast(""), 4000);
        return;
      }
    }
    void applyFolderRule(path, mode);
  };
  const requestRemoveCatalog = async (path: string) => {
    try {
      const info = await invoke<RemoveCatalogInfo>("remove_folder_from_catalog_info", { folderPath: path });
      if (!info.offline && !info.missing) {
        setToast("Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie");
        window.setTimeout(() => setToast(""), 4000);
        return;
      }
      setRemoveCatalogConfirm({ path, info });
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 4000);
    }
  };
  const removeCatalogSource = async () => {
    if (!removeCatalogConfirm) return;
    const path = removeCatalogConfirm.path;
    setRemoveCatalogConfirm(null);
    try {
      const result = await invoke<RemoveCatalogResult>("remove_folder_from_catalog", { folderPath: path });
      setToast(result.summary);
      refreshAfterWrite(true, [path]);
      window.dispatchEvent(new Event("ddreams-catalog-changed"));
      if (view.kind === "folder" && view.folderPath) {
        const norm = (v: string) => v.replace(/[\\/]+$/g, "").toLowerCase(),
          root = norm(path),
          current = norm(view.folderPath);
        if (current === root || current.startsWith(root + "\\")) {
          let candidate = folderParentPath(path);
          let moved = false;
          while (candidate) {
            if (await invoke<boolean>("folder_exists", { path: candidate }).catch(() => false)) {
              const folder = await invoke<Folder | null>("find_folder_path", { path: candidate }).catch(() => null);
              if (folder) {
                navigate({
                  kind: "folder",
                  id: folder.id,
                  ids: folder.folderIds?.length ? folder.folderIds : [folder.id],
                  title: folder.path ?? folder.name,
                  folderPath: folder.path ?? undefined,
                });
                moved = true;
                break;
              }
            }
            candidate = folderParentPath(candidate);
          }
          if (!moved) {
            const roots = await invoke<Folder[]>("get_folder_children", {
              parentId: null,
              parentIds: null,
              offset: 0,
              limit: 500,
            }).catch(() => []);
            const first = roots.find((folder) => folder.path && !folder.virtualNode);
            if (first)
              navigate({
                kind: "folder",
                id: first.id,
                ids: first.folderIds?.length ? first.folderIds : [first.id],
                title: first.path ?? first.name,
                folderPath: first.path ?? undefined,
              });
            else navigate({ kind: "special", value: "all_photos", title: "Všetky fotky" });
          }
        }
      }
      window.setTimeout(() => setToast(""), 5000);
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 4000);
    }
  };
  return {
    deleteConfirm,
    setDeleteConfirm,
    excludeConfirm,
    setExcludeConfirm,
    removeCatalogConfirm,
    setRemoveCatalogConfirm,
    fileOp,
    setFileOp,
    nameOp,
    setNameOp,
    fileConflict,
    setFileConflict,
    moveWarning,
    setMoveWarning,
    cutIds,
    setCutIds,
    fileClipboard,
    setFileClipboard,
    folderClipboard,
    setFolderClipboard,
    folderRenameOp,
    setFolderRenameOp,
    folderDeleteConfirm,
    setFolderDeleteConfirm,
    operationProgress,
    setOperationProgress,
    beginFileOperation,
    runFileOperation,
    chooseOperationTarget,
    dropFiles,
    folderAction,
    submitFolderRename,
    confirmFolderDelete,
    runDelete,
    createFolder,
    submitName,
    applyFolderRule,
    changeFolderRule,
    requestRemoveCatalog,
    removeCatalogSource,
  };
}

import { useEffect, useState, type Dispatch, type SetStateAction } from "react";
import type { Editor } from "../editors";
import { invoke } from "@tauri-apps/api/core";
import type { useNavigation, useListingRefresh } from "./useNavigation";
import type { useGridSelectionState, useAssetValues, useGridSelection } from "./useAssetValues";
import type { useFileOperations } from "./useFileOperations";
import type { useViewer } from "./useViewer";

export function useHelpShortcut() {
  const [helpOpen, setHelpOpen] = useState(false);
  useEffect(() => {
    const handleHelp = (event: globalThis.KeyboardEvent) => {
      if (event.key === "F1" || (helpOpen && event.key === "Escape")) {
        event.preventDefault();
        event.stopImmediatePropagation();
        if (event.key === "F1") setHelpOpen(true);
        else setHelpOpen(false);
      }
    };
    window.addEventListener("keydown", handleHelp, true);
    return () => window.removeEventListener("keydown", handleHelp, true);
  }, [helpOpen]);
  return { helpOpen, setHelpOpen };
}

type KeyboardShortcutsOptions = {
  editors: Editor[];
  openEditor: (editor: Editor, assetIds: number[]) => void;
  setZoom: Dispatch<SetStateAction<number>>;
  setOverlayMode: Dispatch<SetStateAction<0 | 2>>;
  copyCurrentResult: () => Promise<void>;
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "historyIndex" | "travel" | "up">;
  selection: Pick<ReturnType<typeof useGridSelectionState>, "selectedIds" | "primaryId">;
  files: Pick<
    ReturnType<typeof useFileOperations>,
    | "setCutIds"
    | "fileClipboard"
    | "setFileClipboard"
    | "folderClipboard"
    | "setFolderClipboard"
    | "beginFileOperation"
    | "dropFiles"
    | "folderAction"
  >;
  viewerState: Pick<ReturnType<typeof useViewer>, "viewer" | "comparison" | "viewerMenuOpen">;
  listing: Pick<ReturnType<typeof useListingRefresh>, "refresh">;
  assetValues: Pick<ReturnType<typeof useAssetValues>, "setAssetValues">;
  gridSelection: Pick<ReturnType<typeof useGridSelection>, "selectTaggedInView">;
};

export function useKeyboardShortcuts({
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
}: KeyboardShortcutsOptions) {
  const { view, historyIndex, travel, up } = navigation;
  const { selectedIds, primaryId } = selection;
  const {
    setCutIds,
    fileClipboard,
    setFileClipboard,
    folderClipboard,
    setFolderClipboard,
    beginFileOperation,
    dropFiles,
    folderAction,
  } = files;
  const { viewer, comparison, viewerMenuOpen } = viewerState;
  const { refresh } = listing;
  const { setAssetValues } = assetValues;
  const { selectTaggedInView } = gridSelection;
  useEffect(() => {
    const key = (e: globalThis.KeyboardEvent) => {
      if (comparison) return;
      if (e.key === "Shift") window.__ddreamsShift = true;
      if (viewerMenuOpen && e.key.startsWith("Arrow")) {
        e.preventDefault();
        return;
      }
      if (e.defaultPrevented || document.querySelector(".modal-backdrop,[role=dialog],[role=alertdialog]")) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const editable = (el: HTMLElement | null) =>
        !!el && (["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName) || el.isContentEditable);
      if (editable(target) || editable(active)) return;
      if (e.ctrlKey && e.altKey && !e.shiftKey && e.code === "KeyX") {
        if (document.querySelector(".modal-backdrop,[role=dialog],[role=alertdialog]")) return;
        e.preventDefault();
        if (editors[0])
          openEditor(
            editors[0],
            viewer ? (!viewer.asset.offline && !viewer.asset.isDirectory ? [viewer.asset.id] : []) : [...selectedIds],
          );
        return;
      }
      const ids = [...selectedIds];
      const targets = viewer ? [viewer.asset.id] : ids.length ? ids : primaryId == null ? [] : [primaryId];
      if (!e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "u") {
        if (e.repeat || !targets.length) return;
        e.preventDefault();
        void setAssetValues(targets, { editPending: "toggle" });
        return;
      }
      if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key >= "1" && e.key <= "5") {
        e.preventDefault();
        if (!e.repeat) void setAssetValues(targets, { rating: Number(e.key) });
        return;
      }
      if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key === "0") {
        e.preventDefault();
        if (!e.repeat) void setAssetValues(targets, { rating: null });
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.shiftKey && e.key >= "1" && e.key <= "6") {
        e.preventDefault();
        const labels = ["Red", "Yellow", "Green", "Blue", "Purple", "Gray"];
        if (!e.repeat) void setAssetValues(targets, { label: labels[Number(e.key) - 1] });
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.shiftKey && e.key === "0") {
        e.preventDefault();
        if (!e.repeat) void setAssetValues(targets, { label: null });
        return;
      }
      if (!e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "x") {
        if (
          document.querySelector(".modal-backdrop,[role='dialog'],[role='alertdialog']") ||
          !targets.length ||
          e.repeat
        )
          return;
        e.preventDefault();
        void invoke<[number, string | null][]>("get_asset_labels", { assetIds: targets }).then((rows) => {
          if (rows.length !== targets.length) return;
          const allGray = rows.every(([, label]) => label?.toLowerCase() === "gray");
          void setAssetValues(targets, { label: allGray ? null : "Gray" });
        });
        return;
      }
      if (!e.ctrlKey && !e.altKey && !e.shiftKey && e.key === "\\") {
        e.preventDefault();
        if (!e.repeat) void setAssetValues(targets, { tagged: "toggle" });
        return;
      }
      const inGrid =
        !!target?.closest(".asset-scroll,.tile-context-menu") || !!active?.closest(".asset-scroll,.tile-context-menu");
      const inViewer = !!viewer;
      if (!inGrid && !inViewer) return;
      if (e.ctrlKey && e.key.toLowerCase() === "x") {
        e.preventDefault();
        setFileClipboard({ ids: targets, mode: "move" });
        setFolderClipboard(null);
        setCutIds(targets);
        return;
      }
      if (e.ctrlKey && e.key.toLowerCase() === "c" && !e.shiftKey) {
        e.preventDefault();
        setFileClipboard({ ids: targets, mode: "copy" });
        setFolderClipboard(null);
        setCutIds([]);
        return;
      }
      if (e.ctrlKey && e.key.toLowerCase() === "v") {
        e.preventDefault();
        if (view.kind === "folder" && view.folderPath && fileClipboard)
          void dropFiles(fileClipboard.ids, view.folderPath, fileClipboard.mode === "copy");
        else if (view.kind === "folder" && view.folderPath && folderClipboard)
          void folderAction("paste", view.folderPath);
        return;
      }
      if (e.key === "F2" && primaryId != null) {
        e.preventDefault();
        beginFileOperation(targets, "rename");
        return;
      }
      if (e.key === "Delete" && !e.ctrlKey && !e.altKey && targets.length) {
        e.preventDefault();
        if (!e.repeat) beginFileOperation(targets, e.shiftKey ? "delete-permanent" : "delete", viewer?.asset);
        return;
      }
      if (e.ctrlKey && e.key === "\\") {
        e.preventDefault();
        void selectTaggedInView();
        return;
      }
      if (e.ctrlKey && e.key.toLowerCase() === "g") {
        e.preventDefault();
        void copyCurrentResult();
      } else if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "o" && primaryId != null) {
        e.preventDefault();
        void invoke("asset_action", { assetId: primaryId, action: "open" });
      } else if (e.altKey && e.key === "ArrowLeft") {
        e.preventDefault();
        travel(historyIndex - 1);
      } else if (e.altKey && e.key === "ArrowRight") {
        e.preventDefault();
        travel(historyIndex + 1);
      } else if (e.key === "Backspace") {
        e.preventDefault();
        void up();
      } else if (e.key === "F5") {
        e.preventDefault();
        refresh();
      } else if (e.key === "]") {
        setOverlayMode((v) => (v === 0 ? 2 : 0));
      }
    };
    const keyup = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Shift") window.__ddreamsShift = false;
    };
    const zoomer = (e: Event) => setZoom((v) => Math.max(80, Math.min(400, v + (e as CustomEvent<number>).detail)));
    const mouse = (e: globalThis.MouseEvent) => {
      if (comparison) return;
      if (e.button === 3) {
        e.preventDefault();
        travel(historyIndex - 1);
      } else if (e.button === 4) {
        e.preventDefault();
        travel(historyIndex + 1);
      }
    };
    window.addEventListener("keydown", key);
    window.addEventListener("keyup", keyup);
    window.addEventListener("ddreams-zoom", zoomer);
    window.addEventListener("mouseup", mouse);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("keyup", keyup);
      window.removeEventListener("ddreams-zoom", zoomer);
      window.removeEventListener("mouseup", mouse);
    };
  }, [
    travel,
    historyIndex,
    up,
    refresh,
    copyCurrentResult,
    primaryId,
    viewer,
    selectedIds,
    setAssetValues,
    selectTaggedInView,
    view,
    fileClipboard,
    folderClipboard,
    folderAction,
    dropFiles,
    beginFileOperation,
    editors,
    viewerMenuOpen,
    comparison,
  ]);
}

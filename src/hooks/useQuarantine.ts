import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { QuarantineOutcome, QuarantineSettings, Review } from "../components/QuarantineReview";
import type { DeletionSuggestionChange } from "../components/IncomingTools";
import type { DeleteInfo, useFileOperations } from "./useFileOperations";
import type { useGridSelectionState } from "./useAssetValues";
import type { useViewer, useViewerAssets } from "./useViewer";
import type { AppInfo, useNavigation, useListingRefresh } from "./useNavigation";

export function useIncomingFolder() {
  const [incomingFolder, setIncomingFolder] = useState<string | null>(null);
  useEffect(() => {
    void invoke<string>("get_incoming_folder")
      .then(setIncomingFolder)
      .catch((error) => {
        console.warn("Priečinok s novými fotkami", error);
        setIncomingFolder("");
      });
  }, []);
  return { incomingFolder, setIncomingFolder };
}

type DeletionSuggestionsOptions = {
  refreshTick: number;
};

export function useDeletionSuggestions({ refreshTick }: DeletionSuggestionsOptions) {
  const [deletionSuggestions, setDeletionSuggestions] = useState<Set<number>>(() => new Set());
  useEffect(() => {
    let active = true;
    void invoke<number[]>("get_deletion_suggestions")
      .then((ids) => {
        if (active) setDeletionSuggestions(new Set(ids));
      })
      .catch((error) => console.warn("Návrhy na vymazanie", error));
    return () => {
      active = false;
    };
  }, [refreshTick]);
  return { deletionSuggestions };
}

type QuarantineReviewOptions = {
  appInfo: AppInfo | null;
  setToast: Dispatch<SetStateAction<string>>;
  ready: boolean;
  incoming: Pick<ReturnType<typeof useIncomingFolder>, "setIncomingFolder">;
};

export function useQuarantineReview({ appInfo, setToast, ready, incoming }: QuarantineReviewOptions) {
  const { setIncomingFolder } = incoming;
  const [quarantineSettings, setQuarantineSettings] = useState<QuarantineSettings | null>(null);
  const [quarantineReview, setQuarantineReview] = useState<Review | null>(null);
  const [reviewInGrid, setReviewInGrid] = useState(false);
  const [quarantineBusy, setQuarantineBusy] = useState(false);
  const quarantineRunning = useRef(false);
  const [quarantineConfirm, setQuarantineConfirm] = useState<{
    ids: number[];
    action: "move" | "restore";
    suggestionsOnly?: boolean;
  } | null>(null);
  const [quarantineFailures, setQuarantineFailures] = useState<string[]>([]);
  useEffect(() => {
    if (!ready || !appInfo) return;
    let active = true;
    const showError = (error: unknown) => {
      if (active) setToast(String(error));
    };
    void invoke<QuarantineSettings>("get_quarantine_settings")
      .then((settings) => {
        if (active) setQuarantineSettings(settings);
      })
      .catch(showError);
    if (!appInfo.readonly) {
      void invoke<Review>("preview_quarantine_review")
        .then((review) => {
          if (active && (review.moves.ids.length || review.deletes.ids.length || review.staleEdits.ids.length))
            setQuarantineReview(review);
        })
        .catch(showError);
    }
    return () => {
      active = false;
    };
  }, [ready, appInfo]);
  const saveQuarantineSettings = async (preferences: QuarantineSettings) =>
    setQuarantineSettings(await invoke<QuarantineSettings>("set_quarantine_settings", { preferences }));
  const changeIncomingFolder = async (path: string) => {
    setIncomingFolder(await invoke<string>("set_incoming_folder", { path }));
    setQuarantineSettings(await invoke<QuarantineSettings>("get_quarantine_settings"));
  };
  return {
    quarantineSettings,
    saveQuarantineSettings,
    changeIncomingFolder,
    quarantineReview,
    setQuarantineReview,
    reviewInGrid,
    setReviewInGrid,
    quarantineBusy,
    setQuarantineBusy,
    quarantineRunning,
    quarantineConfirm,
    setQuarantineConfirm,
    quarantineFailures,
    setQuarantineFailures,
  };
}

type DeletionActionsOptions = {
  appInfo: AppInfo | null;
  setToast: Dispatch<SetStateAction<string>>;
  refreshAfterWrite: (refreshTree?: boolean, affectedPaths?: string[]) => void;
};

export function useDeletionActions({ appInfo, setToast, refreshAfterWrite }: DeletionActionsOptions) {
  const showDeletionChange = useCallback(
    (change: DeletionSuggestionChange) => {
      setToast(change.summary);
      if (change.changeId != null) refreshAfterWrite();
      window.setTimeout(() => setToast(""), 6000);
    },
    [refreshAfterWrite],
  );
  useEffect(() => {
    const subscription = listen<DeletionSuggestionChange>("deletion-suggestions-changed", (event) => {
      showDeletionChange(event.payload);
    });
    return () => {
      void subscription.then((unlisten) => unlisten());
    };
  }, [showDeletionChange]);
  const runDeletionAction = async (
    command: "analyze_incoming_series" | "keep_suggested_assets",
    assetIds?: number[],
  ) => {
    if (appInfo?.readonly) return;
    try {
      const args = command === "keep_suggested_assets" ? { assetIds: assetIds ?? null } : undefined;
      showDeletionChange(await invoke<DeletionSuggestionChange>(command, args));
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 4000);
    }
  };
  return { runDeletionAction };
}

type QuarantineOptions = {
  appInfo: AppInfo | null;
  setCenterMode: Dispatch<SetStateAction<"map" | "grid">>;
  setToast: Dispatch<SetStateAction<string>>;
  files: Pick<ReturnType<typeof useFileOperations>, "setDeleteConfirm" | "setOperationProgress">;
  review: Pick<
    ReturnType<typeof useQuarantineReview>,
    | "setQuarantineReview"
    | "setReviewInGrid"
    | "quarantineBusy"
    | "setQuarantineBusy"
    | "quarantineRunning"
    | "setQuarantineConfirm"
    | "setQuarantineFailures"
  >;
  selection: Pick<ReturnType<typeof useGridSelectionState>, "setSelectedIds" | "setPrimaryId" | "setPrimaryAsset">;
  viewerState: Pick<ReturnType<typeof useViewer>, "setViewer" | "comparison">;
  navigation: Pick<ReturnType<typeof useNavigation>, "navigate">;
  listing: Pick<ReturnType<typeof useListingRefresh>, "refreshAfterWrite">;
  viewerAssets: Pick<ReturnType<typeof useViewerAssets>, "advanceAfterRemoval">;
};

export function useQuarantine({
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
}: QuarantineOptions) {
  const { setDeleteConfirm, setOperationProgress } = files;
  const {
    setQuarantineReview,
    setReviewInGrid,
    quarantineBusy,
    setQuarantineBusy,
    quarantineRunning,
    setQuarantineConfirm,
    setQuarantineFailures,
  } = review;
  const { setSelectedIds, setPrimaryId, setPrimaryAsset } = selection;
  const { setViewer, comparison } = viewerState;
  const { navigate } = navigation;
  const { refreshAfterWrite } = listing;
  const { advanceAfterRemoval } = viewerAssets;
  const requestQuarantineMove = (ids: number[]) => {
    if (!appInfo?.readonly && !quarantineBusy) setQuarantineConfirm({ ids, action: "move" });
  };
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
      ]
        .filter(Boolean)
        .join(" · ");
      setToast(summary || "Karanténa bez zmien");
      if (result.affectedFolderPaths.length) {
        let removed: number[] = [];
        if (
          (command === "move_to_quarantine" || command === "delete_from_quarantine") &&
          Array.isArray(args.assetIds)
        ) {
          const ids = args.assetIds as number[];
          const failures = new Set(
            result.failures.flatMap((error) => {
              const match = /^Položka (-?\d+):/.exec(error);
              return match ? [Number(match[1])] : [];
            }),
          );
          const successfulIds = ids.filter((id) => !failures.has(id));
          const count = command === "move_to_quarantine" ? result.moved : result.deleted;
          if (count === successfulIds.length) removed = successfulIds;
          else if (ids.length === 1 && count === 1) removed = ids;
        }
        if (comparison) {
          const ids = new Set(removed);
          setSelectedIds((current) => new Set([...current].filter((id) => !ids.has(id))));
          setPrimaryId((id) => (id != null && ids.has(id) ? null : id));
          setPrimaryAsset((asset) => (asset && ids.has(asset.id) ? null : asset));
        } else {
          setSelectedIds(new Set());
          setPrimaryId(null);
          setPrimaryAsset(null);
        }
        refreshAfterWrite(true, result.affectedFolderPaths);
        if (removed.length) await advanceAfterRemoval(removed);
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
      const info = await invoke<DeleteInfo>("get_quarantine_delete_info", {
        assetIds: ids,
      });
      setDeleteConfirm({ ids, name: null, permanent: false, quarantine: true, info });
    } catch (error) {
      setToast(String(error));
    }
  };

  const inspectQuarantineReview = (ids: number[], title: string) => {
    setCenterMode("grid");
    setViewer(null);
    navigate({ kind: "quarantine_review", ids, title });
    setReviewInGrid(true);
  };
  return { requestQuarantineMove, runQuarantine, requestQuarantineDelete, inspectQuarantineReview };
}

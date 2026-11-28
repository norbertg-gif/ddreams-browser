import type { buildAssetFilter } from "../assetFilter";
import {
  useCallback,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type Dispatch,
  type SetStateAction,
  type RefObject,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AssetGridHandle, AssetValuePatch } from "../components/AssetGrid";
import type { Asset, IptcTextField, Page } from "../types";
import type { AppInfo, useNavigation, useListingRefresh } from "./useNavigation";
import type { useViewer } from "./useViewer";

export type AssetEditValues = {
  changeId: number | null;
  summary: string;
  value?: string | null;
  remappedIds: { oldId: number; newId: number }[];
  assets: {
    id: number;
    rating: number | null;
    label: string | null;
    tagged: boolean;
    editPendingSince: number | null;
  }[];
};

type CategoryMutation = {
  changeId: number | null;
  summary: string;
  categoryId: number | null;
  parentId: number | null;
  rootId: number | null;
  ancestorIds: number[];
  deletedCategoryIds: number[];
  deletedRootIds: number[];
};
type KeywordMutation = {
  changeId: number | null;
  summary: string;
  keywordId: number | null;
  parentId: number | null;
  ancestorIds: number[];
  deletedKeywordIds: number[];
};

export function useGridSelectionState() {
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [storedPrimaryId, setPrimaryId] = useState<number | null>(null);
  const [storedPrimaryAsset, setPrimaryAsset] = useState<Asset | null>(null);
  const primaryId = storedPrimaryId != null && selectedIds.has(storedPrimaryId) ? storedPrimaryId : null;
  const primaryAsset = primaryId != null && storedPrimaryAsset?.id === primaryId ? storedPrimaryAsset : null;
  return { selectedIds, setSelectedIds, setPrimaryId, setPrimaryAsset, primaryId, primaryAsset };
}

type AssetValueStateOptions = {
  historyOpen: boolean;
  refreshTick: number;
  selection: Pick<ReturnType<typeof useGridSelectionState>, "selectedIds" | "primaryId">;
};

export function useAssetValueState({ historyOpen, refreshTick, selection }: AssetValueStateOptions) {
  const { selectedIds, primaryId } = selection;
  const valueWrites = useRef(Promise.resolve());
  // A photo that just received a catalogue ID is still the same selection.
  const valueAliases = useRef(new Map<number, number>());
  const [valueTokens, setValueTokens] = useState({ rating: 0, label: 0, special: 0 });
  const [primaryValueEdit, setPrimaryValueEdit] = useState<{ patch: AssetValuePatch; refreshTick: number }>();
  const valueContext = useRef({ primaryId, refreshTick, historyOpen });
  valueContext.current = { primaryId, refreshTick, historyOpen };
  const selectedList = useMemo(() => [...selectedIds], [selectedIds]);
  const selectedReadIds = useMemo(() => selectedList.map((id) => valueAliases.current.get(id) ?? id), [selectedList]);
  const primaryReadId = primaryId == null ? null : (valueAliases.current.get(primaryId) ?? primaryId);
  return {
    valueWrites,
    valueAliases,
    valueTokens,
    setValueTokens,
    primaryValueEdit,
    setPrimaryValueEdit,
    valueContext,
    selectedList,
    selectedReadIds,
    primaryReadId,
  };
}

type GridSelectionOptions = {
  assetFilter: ReturnType<typeof buildAssetFilter>;
  selection: Pick<
    ReturnType<typeof useGridSelectionState>,
    "selectedIds" | "setSelectedIds" | "setPrimaryId" | "setPrimaryAsset" | "primaryId"
  >;
};

export function useGridSelection({ assetFilter, selection }: GridSelectionOptions) {
  const { selectedIds, setSelectedIds, setPrimaryId, setPrimaryAsset, primaryId } = selection;
  const select = useCallback(
    (asset: Asset, _index: number, event: ReactMouseEvent | ReactKeyboardEvent) => {
      if (event.ctrlKey || event.metaKey) {
        const next = new Set(selectedIds);
        if (next.has(asset.id)) {
          next.delete(asset.id);
          if (primaryId === asset.id || !next.size) {
            setPrimaryId(null);
            setPrimaryAsset(null);
          }
        } else {
          next.add(asset.id);
          setPrimaryId(asset.id);
          setPrimaryAsset(asset);
        }
        setSelectedIds(next);
      } else if (!event.shiftKey) {
        setSelectedIds(new Set([asset.id]));
        setPrimaryId(asset.id);
        setPrimaryAsset(asset);
      }
    },
    [selectedIds, primaryId],
  );
  const selectRange = useCallback((ids: number[], asset: Asset, _index: number, add = false) => {
    setSelectedIds((current) => new Set(add ? [...current, ...ids] : ids));
    setPrimaryId(asset.id);
    setPrimaryAsset(asset);
  }, []);
  const selectAll = useCallback(
    async (ids: number[]) => {
      setSelectedIds(new Set(ids));
      if (!ids.length) {
        setPrimaryId(null);
        setPrimaryAsset(null);
        return;
      }
      const wanted = new Set(ids);
      const page = await invoke<Page>("get_assets", { filter: { ...assetFilter, offset: 0, limit: 500 } });
      const asset = page.items.find((a) => wanted.has(a.id));
      if (asset) {
        setPrimaryId(asset.id);
        setPrimaryAsset(asset);
      }
    },
    [assetFilter],
  );
  const selectTaggedInView = useCallback(async () => {
    const filter = { ...assetFilter, taggedOnly: true };
    const first = await invoke<Page>("get_assets", { filter: { ...filter, offset: 0, limit: 1 } });
    const ids: number[] = [];
    for (let offset = 0; offset < first.total; offset += 500) {
      const page = await invoke<Page>("get_assets", { filter: { ...filter, offset, limit: 500 } });
      ids.push(...page.items.filter((asset) => !asset.isDirectory).map((asset) => asset.id));
      if (page.items.length < 500) break;
    }
    setSelectedIds(new Set(ids));
    if (ids.length) {
      const page = await invoke<Page>("get_assets", { filter: { ...filter, offset: 0, limit: 500 } });
      const asset = page.items.find((item) => item.id === ids[0]);
      if (asset) {
        setPrimaryId(asset.id);
        setPrimaryAsset(asset);
      }
    }
  }, [assetFilter]);
  return { select, selectRange, selectAll, selectTaggedInView };
}

type AssetValuesOptions = {
  appInfo: AppInfo | null;
  setToast: Dispatch<SetStateAction<string>>;
  setRefreshTick: Dispatch<SetStateAction<number>>;
  grid: RefObject<AssetGridHandle | null>;
  currentAssetFilter: RefObject<ReturnType<typeof buildAssetFilter>>;
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "navigate">;
  selection: Pick<
    ReturnType<typeof useGridSelectionState>,
    "setSelectedIds" | "setPrimaryId" | "setPrimaryAsset" | "primaryId"
  >;
  viewerState: Pick<
    ReturnType<typeof useViewer>,
    "setViewer" | "setComparison" | "currentViewer" | "viewerValuePatches"
  >;
  assetValueState: Pick<
    ReturnType<typeof useAssetValueState>,
    "valueWrites" | "valueAliases" | "setValueTokens" | "setPrimaryValueEdit" | "valueContext"
  >;
  listing: Pick<ReturnType<typeof useListingRefresh>, "loadRecentChanges" | "refreshAfterWrite">;
};

export function useAssetValues({
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
}: AssetValuesOptions) {
  const { view, navigate } = navigation;
  const { setSelectedIds, setPrimaryId, setPrimaryAsset, primaryId } = selection;
  const { setViewer, setComparison, currentViewer, viewerValuePatches } = viewerState;
  const { valueWrites, valueAliases, setValueTokens, setPrimaryValueEdit, valueContext } = assetValueState;
  const { loadRecentChanges, refreshAfterWrite } = listing;
  const applyAssetValues = useCallback(
    (result: AssetEditValues, changes: Record<string, unknown>) => {
      const textField: IptcTextField | null = "caption" in changes ? "caption" : "author" in changes ? "author" : null;
      const ratingEdited = "rating" in changes,
        labelEdited = "label" in changes;
      const taggedEdited = "tagged" in changes,
        pendingEdited = "editPending" in changes;
      const patches = new Map<number, AssetValuePatch>(
        result.assets.map((asset) => [
          asset.id,
          {
            id: asset.id,
            rating: asset.rating,
            label: asset.label,
            tagged: Number(asset.tagged),
            ...(pendingEdited ? { editPendingSince: asset.editPendingSince } : {}),
            ...(textField ? { [textField]: result.value ?? null } : {}),
          },
        ]),
      );
      const remap = new Map(result.remappedIds.map(({ oldId, newId }) => [oldId, newId]));
      for (const [oldId, newId] of remap) {
        const patch = patches.get(newId);
        if (patch) patches.set(oldId, patch);
      }
      if (remap.size) {
        for (const [oldId, newId] of remap) valueAliases.current.set(newId, oldId);
        setSelectedIds((previous) =>
          [...previous].some((id) => remap.has(id))
            ? new Set([...previous].map((id) => remap.get(id) ?? id))
            : previous,
        );
        setPrimaryId((id) => (id == null ? id : (remap.get(id) ?? id)));
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
      setPrimaryAsset((previous) => previous && patchAsset(previous));
      setViewer((previous) => {
        if (!previous) return previous;
        const asset = patchAsset(previous.asset);
        return asset === previous.asset ? previous : { ...previous, asset };
      });
      setComparison(
        (current) =>
          current?.map((photo) => {
            const asset = patchAsset(photo.asset);
            return asset === photo.asset ? photo : { ...photo, asset };
          }) ?? null,
      );
      const context = valueContext.current;
      const patch = context.primaryId == null ? undefined : patches.get(context.primaryId);
      if (patch) {
        setPrimaryValueEdit((previous) => ({
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
          (pendingEdited && filter.kind === "special" && filter.value === "edit_pending") ||
          (textField === "caption" && (filter.sort === "caption" || !!filter.query));
        grid.current?.applyValues(patches, reloadListing);
        setValueTokens((tokens) => ({
          rating: tokens.rating + Number(ratingEdited || !!remap.size),
          label: tokens.label + Number(labelEdited || !!remap.size),
          special: tokens.special + Number(taggedEdited || pendingEdited || !!remap.size),
        }));
        if (context.historyOpen) loadRecentChanges();
      } else {
        grid.current?.applyValues(patches, false);
      }
    },
    [loadRecentChanges],
  );
  const setAssetValues = useCallback(
    (ids: number[], changes: Record<string, unknown>) => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        window.setTimeout(() => setToast(""), 2200);
        return;
      }
      if (!ids.length) return;
      const write = async () => {
        try {
          const result = await invoke<AssetEditValues>("set_asset_values", { assetIds: ids, changes });
          applyAssetValues(result, changes);
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
    },
    [appInfo?.readonly, applyAssetValues],
  );
  const setAssetColors = useCallback(
    async (ids: number[], color: string, state: boolean | null) => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        return;
      }
      try {
        const count = await invoke<number>("set_asset_colors", { assetIds: ids, color, state });
        if (count) {
          setToast(`Farba ${color} opravená`);
          refreshAfterWrite();
          window.setTimeout(() => setRefreshTick((v) => v + 1), 2400);
          window.setTimeout(() => setToast(""), 6000);
        }
      } catch (error) {
        setToast(String(error));
        window.setTimeout(() => setToast(""), 3500);
      }
    },
    [appInfo, refreshAfterWrite],
  );
  const setAssetCategories = useCallback(
    async (ids: number[], categoryId: number, assign: boolean): Promise<boolean> => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        window.setTimeout(() => setToast(""), 2200);
        return false;
      }
      if (!ids.length) return false;
      try {
        const result = await invoke<{
          changeId: number | null;
          summary: string;
          changedCount: number;
          remappedIds: { oldId: number; newId: number }[];
        }>("set_asset_categories", { assetIds: ids, categoryId, assign });
        const remap = new Map(result.remappedIds.map((item) => [item.oldId, item.newId]));
        setSelectedIds((previous) => new Set([...previous].map((id) => remap.get(id) ?? id)));
        const nextPrimary = primaryId == null ? null : (remap.get(primaryId) ?? primaryId);
        if (nextPrimary != null) {
          setPrimaryId(nextPrimary);
          setPrimaryAsset((previous) => (previous ? { ...previous, id: nextPrimary } : previous));
        }
        setToast(result.summary);
        refreshAfterWrite();
        window.setTimeout(() => setToast(""), 6000);
        return true;
      } catch (error) {
        const message = String(error);
        setToast(message.toLowerCase().includes("read-only") ? "Katalóg je iba na čítanie" : message);
        window.setTimeout(() => setToast(""), 3000);
        return false;
      }
    },
    [appInfo, primaryId, refreshAfterWrite],
  );
  const keywordStructureChanged = useCallback(
    (result: KeywordMutation, deleted: boolean) => {
      setToast(result.summary);
      refreshAfterWrite();
      if (deleted && view.kind === "keyword" && view.id != null && result.deletedKeywordIds.includes(view.id))
        navigate({ kind: "special", value: "all_photos", title: "Všetky fotky" });
      window.setTimeout(() => setToast(""), 6000);
    },
    [refreshAfterWrite, view, navigate],
  );
  const setAssetKeywords = useCallback(
    async (ids: number[], keywordId: number, assign: boolean): Promise<boolean> => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        window.setTimeout(() => setToast(""), 2200);
        return false;
      }
      if (!ids.length) return false;
      try {
        const result = await invoke<{
          changeId: number | null;
          summary: string;
          changedCount: number;
          remappedIds: { oldId: number; newId: number }[];
        }>("set_asset_keywords", { assetIds: ids, keywordId, assign });
        const remap = new Map(result.remappedIds.map((item) => [item.oldId, item.newId]));
        setSelectedIds((previous) => new Set([...previous].map((id) => remap.get(id) ?? id)));
        const nextPrimary = primaryId == null ? null : (remap.get(primaryId) ?? primaryId);
        if (nextPrimary != null) {
          setPrimaryId(nextPrimary);
          setPrimaryAsset((previous) => (previous ? { ...previous, id: nextPrimary } : previous));
        }
        setToast(result.summary);
        refreshAfterWrite();
        window.setTimeout(() => setToast(""), 6000);
        return result.changedCount > 0;
      } catch (error) {
        const message = String(error);
        setToast(
          message.toLowerCase().includes("read-only") || appInfo?.readonly ? "Katalóg je iba na čítanie" : message,
        );
        window.setTimeout(() => setToast(""), 3000);
        return false;
      }
    },
    [appInfo, primaryId, refreshAfterWrite],
  );
  const createAndAssignKeyword = useCallback(
    async (ids: number[], parentId: number | null, name: string) => {
      if (appInfo?.readonly) {
        setToast("Katalóg je iba na čítanie");
        window.setTimeout(() => setToast(""), 2200);
        return;
      }
      if (!ids.length) return;
      try {
        const result = await invoke<{
          changeId: number | null;
          summary: string;
          changedCount: number;
          remappedIds: { oldId: number; newId: number }[];
          keywordId: number | null;
          parentId: number | null;
          ancestorIds: number[];
        }>("create_and_assign_keyword", { assetIds: ids, parentId, name });
        const remap = new Map(result.remappedIds.map((item) => [item.oldId, item.newId]));
        setSelectedIds((previous) => new Set([...previous].map((id) => remap.get(id) ?? id)));
        const nextPrimary = primaryId == null ? null : (remap.get(primaryId) ?? primaryId);
        if (nextPrimary != null) {
          setPrimaryId(nextPrimary);
          setPrimaryAsset((previous) => (previous ? { ...previous, id: nextPrimary } : previous));
        }
        if (result.keywordId != null)
          keywordStructureChanged(
            {
              changeId: result.changeId,
              summary: result.summary,
              keywordId: result.keywordId,
              parentId: result.parentId,
              ancestorIds: result.ancestorIds,
              deletedKeywordIds: [],
            },
            false,
          );
      } catch (error) {
        const message = String(error);
        setToast(
          message.toLowerCase().includes("read-only") || appInfo?.readonly ? "Katalóg je iba na čítanie" : message,
        );
        window.setTimeout(() => setToast(""), 3500);
      }
    },
    [appInfo, primaryId, keywordStructureChanged],
  );
  const categoryStructureChanged = useCallback(
    (result: CategoryMutation, deleted: boolean) => {
      setToast(result.summary);
      refreshAfterWrite();
      if (
        deleted &&
        view.kind === "category" &&
        ((view.id != null && result.deletedCategoryIds.includes(view.id)) ||
          (view.rootId != null && result.deletedRootIds.includes(view.rootId)))
      )
        navigate({ kind: "special", value: "all_photos", title: "Všetky fotky" });
      window.setTimeout(() => setToast(""), 6000);
    },
    [refreshAfterWrite, view, navigate],
  );
  const categoryWriteError = useCallback(
    (error: unknown) => {
      const message = String(error);
      setToast(
        appInfo?.readonly || message.toLowerCase().includes("read-only") ? "Katalóg je iba na čítanie" : message,
      );
      window.setTimeout(() => setToast(""), 4000);
    },
    [appInfo],
  );
  return {
    applyAssetValues,
    setAssetValues,
    setAssetColors,
    setAssetCategories,
    keywordStructureChanged,
    setAssetKeywords,
    createAndAssignKeyword,
    categoryStructureChanged,
    categoryWriteError,
  };
}

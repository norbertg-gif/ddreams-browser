import type { AssetFilter as IpcAssetFilter } from "./bindings/AssetFilter";
import type { View, ViewFilters } from "./types";

export type SavedViewSorts = Record<string, { sort: string; descending: boolean; stamp: number }>;

export function sortKeyForView(view: View): string {
  if (view.kind !== "folder") return "catalog";
  const path = (view.folderPath ?? view.title).replace(/[\\/]+$/g, "").replace(/\//g, "\\");
  return "folder:" + path.toLowerCase();
}

export function readViewSort(saved: SavedViewSorts, key: string) {
  return saved[key] ?? { sort: "name", descending: false };
}

export function updateViewSort(saved: SavedViewSorts, key: string, sort: string, descending: boolean): SavedViewSorts {
  const updated = { ...saved, [key]: { sort, descending, stamp: Date.now() } };
  return Object.fromEntries(
    Object.entries(updated)
      .sort((a, b) => b[1].stamp - a[1].stamp)
      .slice(0, 500),
  );
}

export function effectiveViewFilters(filters: ViewFilters, showOtherFiles: boolean): ViewFilters {
  return {
    ...filters,
    fileGroup: filters.fileGroup === "all" ? undefined : (filters.fileGroup ?? (showOtherFiles ? undefined : "media")),
  };
}

export function buildAssetFilter(
  view: View,
  query: string | null,
  sort: string,
  descending: boolean,
  filters: ViewFilters,
  includeChildren: boolean,
) {
  return {
    kind: view.kind,
    id: view.id ?? null,
    rootId: view.rootId ?? null,
    includeChildren: (view.kind === "category" || view.kind === "keyword") && includeChildren,
    ids: view.ids ?? null,
    folderPath: view.folderPath ?? null,
    value: view.value ?? null,
    dateFrom: view.dateFrom ?? null,
    dateTo: view.dateTo ?? null,
    dateSource: view.dateSource ?? null,
    dateNoDate: view.dateNoDate ?? false,
    query,
    sort,
    descending,
    ratingMode: filters.ratingMode ?? null,
    ratingValue: filters.ratingValue ?? null,
    labelFilter: filters.labelFilter ?? null,
    taggedOnly: filters.taggedOnly ?? false,
    categoryFilter: filters.categoryFilter ?? null,
    keywordFilter: filters.keywordFilter ?? null,
    fileGroup: filters.fileGroup ?? null,
  } satisfies Omit<IpcAssetFilter, "offset" | "limit">;
}

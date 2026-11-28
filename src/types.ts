import type { Asset as IpcAsset } from "./bindings/Asset";

// Local asset patches use null to clear a timestamp that IPC omits.
export type Asset = Omit<IpcAsset, "editPendingSince"> & { editPendingSince?: IpcAsset["editPendingSince"] | null };
export type TileExifMetadata = import("./bindings/TileExifValues").TileExifValues;
export type TileExifMetadataItem = import("./bindings/TileExifItem").TileExifItem;
export type View = {
  kind:
    | "folder"
    | "auto"
    | "category"
    | "keyword"
    | "rating"
    | "label"
    | "special"
    | "date"
    | "ai"
    | "color"
    | "quality"
    | "series"
    | "people"
    | "quarantine_review";
  id?: number;
  rootId?: number;
  ids?: number[];
  groups?: number[];
  bestIds?: number[];
  seriesInfo?: { count: number; analyzed: number; truncated: boolean };
  value?: string;
  title: string;
  folderPath?: string;
  dateFrom?: string;
  dateTo?: string;
  dateSource?: "taken" | "database" | "modified";
  dateNoDate?: boolean;
  suggestion?: { kind: "category" | "keyword"; labelId: number; path: string };
};
export type AssetFilter = Pick<import("./bindings/AssetFilter").AssetFilter, "kind"> &
  Partial<import("./bindings/AssetFilter").AssetFilter> &
  Pick<View, "kind">;
export type ViewFilters = {
  ratingMode?: "equal" | "atLeast" | "atMost" | "unrated";
  ratingValue?: number;
  labelFilter?: string;
  taggedOnly?: boolean;
  categoryFilter?: "with" | "without";
  keywordFilter?: "with" | "without";
  fileGroup?: "images" | "raw" | "video" | "other" | "media" | "all";
};
export type Page = import("./bindings/AssetPage").AssetPage;
export type Folder = import("./bindings/Folder").Folder;
export type Node = import("./bindings/CatalogNode").CatalogNode;
export type Root = import("./bindings/Root").Root;
export type CatalogAssetLinks = import("./bindings/CatalogAssetLinks").CatalogAssetLinks;
export type ValueCount = import("./bindings/ValueCount").ValueCount;
export type AssetQuality = import("./bindings/AssetQuality").AssetQuality;
export type MetadataRow = import("./bindings/MetadataRow").MetadataRow;
export type Metadata = import("./bindings/MetadataResponse").MetadataResponse;
export type CatalogAssetMetadata = import("./bindings/AssetMetadata").AssetMetadata;
export type IptcTextField = import("./bindings/AssetTextField").AssetTextField;
export type IptcPhoto = import("./bindings/EditorPhoto").EditorPhoto;
export type IptcTarget = import("./bindings/EditorTarget").EditorTarget;
export type IptcEdit = import("./bindings/ManualEdit").ManualEdit;
export type IptcIssue = import("./bindings/Issue").Issue;
export type IptcSummary = import("./bindings/WriteSummary").WriteSummary;

export type FileFormat = import("./bindings/Format").Format;
export type FormatTable = import("./bindings/FormatTable").FormatTable;

export type Asset = {
  id: number; folderId: number | null; name: string; ext: string; size: number | null;
  width: number | null; height: number | null; bpp: number | null; caption: string | null;
  notes: string | null; author: string | null; rating: number | null; tagged: number | null;
  label: string | null; fileModified: string | null; exifDate: string | null;
  acdseeDate: string | null; mdDirty: number | null; isOrphan: number | null; path: string | null;
  isDirectory?: boolean; offline?: boolean;
  hasCategories?: boolean;
  displayCategories?: string | null; displayKeywords?: string | null;
};
export type TileExifMetadata = { iso?: string | null; aperture?: string | null; shutter?: string | null; focalLength?: string | null; lens?: string | null; camera?: string | null; dateTaken?: string | null };
export type TileExifMetadataItem = TileExifMetadata & { assetId: number };
export type View = { kind: "folder" | "category" | "keyword" | "rating" | "label" | "special" | "date" | "ai" | "color" | "quality" | "series" | "people"; id?: number; rootId?: number; ids?: number[]; groups?: number[]; bestIds?: number[]; seriesInfo?: { count: number; analyzed: number; truncated: boolean }; value?: string; title: string; folderPath?: string; dateFrom?: string; dateTo?: string; dateSource?: "taken" | "database" | "modified"; dateNoDate?: boolean; suggestion?: {kind:"category"|"keyword";labelId:number;path:string} };
export type AssetFilter = { kind: View["kind"]; id?: number | null; rootId?: number | null; includeChildren?: boolean; ids?: number[] | null; folderPath?: string | null; value?: string | null; query?: string | null; sort?: string; descending?: boolean; offset?: number; limit?: number; ratingMode?: ViewFilters["ratingMode"] | null; ratingValue?: number | null; labelFilter?: string | null; taggedOnly?: boolean; fileGroup?: ViewFilters["fileGroup"] | null; dateFrom?: string | null; dateTo?: string | null; dateSource?: View["dateSource"] | null; dateNoDate?: boolean; includeCategories?: boolean; includeKeywords?: boolean };
export type ViewFilters = { ratingMode?: "equal" | "atLeast" | "atMost" | "unrated"; ratingValue?: number; labelFilter?: string; taggedOnly?: boolean; fileGroup?: "images" | "raw" | "video" | "other" | "media" | "all" };
export type Page = { items: Asset[]; total: number; totalSize: number };
export type Folder = { id: number; parentId: number | null; rootId: number | null; name: string; path: string | null; count: number; folderIds: number[]; virtualNode?: string | null; offline?: boolean };
export type Node = { id: number; parentId: number | null; rootId: number | null; name: string; count: number; hasChildren: boolean };
export type Root = { id: number; name: string; count: number; mergedCategoryId: number | null; hasChildren: boolean };
export type CatalogAssetLinks = { categoryIds: number[]; categoryPaths: {id:number;path:string}[]; categoryAncestorIds: number[]; keywordIds: number[]; keywordPaths: {id:number;path:string}[]; keywordAncestorIds: number[]; rootIds: number[] };
export type ValueCount = { value: string; count: number };
export type AssetQuality = { assetId: number; sharp: number; hi: number; lo: number; mean: number; flags: string[] };
export type MetadataRow = { group: string; subgroup: string; name: string; value: string };
export type Metadata = { asset: Asset; offline: boolean; rows: MetadataRow[] };
export type CatalogAssetMetadata = { asset: Asset; categories: string[]; keywords: string[] };

export type FileFormat = { ext: string; group: "image" | "raw" | "video" | "document" | "other"; name: string; hidden?: boolean };
export type FormatTable = { version: number; formats: FileFormat[] };

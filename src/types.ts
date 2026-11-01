export type Asset = {
  id: number; folderId: number | null; name: string; ext: string; size: number | null;
  width: number | null; height: number | null; bpp: number | null; caption: string | null;
  notes: string | null; author: string | null; rating: number | null; tagged: number | null;
  label: string | null; fileModified: string | null; exifDate: string | null;
  acdseeDate: string | null; mdDirty: number | null; isOrphan: number | null; path: string | null;
  isDirectory?: boolean; offline?: boolean;
};
export type View = { kind: "folder" | "category" | "keyword" | "rating" | "label"; id?: number; ids?: number[]; value?: string; title: string; folderPath?: string };
export type Page = { items: Asset[]; total: number; totalSize: number };
export type Folder = { id: number; parentId: number | null; rootId: number | null; name: string; path: string | null; count: number; folderIds: number[]; virtualNode?: string | null; offline?: boolean };
export type Node = { id: number; parentId: number | null; rootId: number | null; name: string; count: number };
export type Root = { id: number; name: string };
export type ValueCount = { value: string; count: number };
export type MetadataRow = { group: string; name: string; value: string };
export type Metadata = { asset: Asset; offline: boolean; rows: MetadataRow[] };

import { invoke } from "@tauri-apps/api/core";

export type CategoryMutation = { changeId:number|null; summary:string; categoryId:number|null; parentId:number|null; rootId:number|null; ancestorIds:number[]; deletedCategoryIds:number[]; deletedRootIds:number[] };
export type KeywordMutation = { changeId:number|null;summary:string;keywordId:number|null;parentId:number|null;ancestorIds:number[];deletedKeywordIds:number[] };
type ColorItem={value:string;name:string;hex:string;auto:boolean;effective:boolean;manualState:boolean|null};export type AssetColorInfo={assetId:number;computed:boolean;colors:ColorItem[]};
export type AiSuggestion={labelId:number;path:string;score:number;count?:number;assetIds?:number[]};
export type UnsortedCluster={ids:number[];total:number;cohesion:number;labelId:number|null;labelPath:string|null;name:string|null};
export type FacePerson={id:number;name:string;confirmed:number;auto:number;assets:number;cover:number|null};
export type FaceInfo={faceId:number;assetId:number;x:number|null;y:number|null;w:number|null;h:number|null;personId:number|null;name:string|null;state:string;score:number|null};
export type FaceReview={faceId:number;assetId:number;score:number|null};
export type FaceUnknownCluster={faceIds:number[];total:number;assetIds:number[];assets:number;guess:{personId:number;name:string;score:number}|null};
export type FaceStatus={available:boolean;scanned:number;pending:number;faces:number;named:number;unknown:number;persons:number};
type FaceThumb={faceId:number;dataUrl:string};
export async function fetchFaceThumbs(faceIds:number[]):Promise<Record<number,string>>{const result:Record<number,string>={};for(let i=0;i<faceIds.length;i+=300){const rows=await invoke<FaceThumb[]>('ai_face_thumbs',{faceIds:faceIds.slice(i,i+300)});for(const row of rows)result[row.faceId]=row.dataUrl;}return result;}

import { invoke } from "@tauri-apps/api/core";

export type CategoryMutation = import("../bindings/CategoryMutationResult").CategoryMutationResult;
export type KeywordMutation = import("../bindings/KeywordMutationResult").KeywordMutationResult;
export type AssetColorInfo = import("../bindings/AssetColors").AssetColors;
export type AiSuggestion = { labelId: number; path: string; score: number; count?: number; assetIds?: number[] };
export type UnsortedCluster = import("../bindings/AiUnsortedCluster").AiUnsortedCluster;
export type FacePerson = import("../bindings/AiFacePerson").AiFacePerson;
export type FaceInfo = import("../bindings/AiFaceAsset").AiFaceAsset;
export type FaceReview = import("../bindings/AiFaceReview").AiFaceReview;
export type FaceUnknownCluster = import("../bindings/AiFaceCluster").AiFaceCluster;
export type FaceStatus = import("../bindings/AiFacesStatus").AiFacesStatus;
type FaceThumb = import("../bindings/AiFaceThumb").AiFaceThumb;
export async function fetchFaceThumbs(faceIds: number[]): Promise<Record<number, string>> {
  const result: Record<number, string> = {};
  for (let i = 0; i < faceIds.length; i += 300) {
    const rows = await invoke<FaceThumb[]>("ai_face_thumbs", { faceIds: faceIds.slice(i, i + 300) });
    for (const row of rows) result[row.faceId] = row.dataUrl;
  }
  return result;
}

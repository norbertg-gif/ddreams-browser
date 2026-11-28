import { useCallback, useState, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { IptcEdit, IptcSummary, IptcTarget } from "../types";
import type { useFileOperations } from "./useFileOperations";
import type { useAssetValueState, useAssetValues } from "./useAssetValues";
import type { AppInfo, useListingRefresh } from "./useNavigation";

type IptcOptions = {
  appInfo: AppInfo | null;
  setToast: Dispatch<SetStateAction<string>>;
  files: Pick<ReturnType<typeof useFileOperations>, "setOperationProgress">;
  assetValueState: Pick<ReturnType<typeof useAssetValueState>, "valueWrites">;
  listing: Pick<ReturnType<typeof useListingRefresh>, "refreshAfterWrite">;
  assetValues: Pick<ReturnType<typeof useAssetValues>, "applyAssetValues">;
};

export function useIptc({ appInfo, setToast, files, assetValueState, listing, assetValues }: IptcOptions) {
  const { setOperationProgress } = files;
  const { valueWrites } = assetValueState;
  const { refreshAfterWrite } = listing;
  const { applyAssetValues } = assetValues;
  const [iptcIds, setIptcIds] = useState<number[] | null>(null);
  const [iptcToken, setIptcToken] = useState(0);
  const [iptcEditIssues, setIptcEditIssues] = useState<IptcSummary | null>(null);
  const editIptc = useCallback(
    (targets: IptcTarget[], action: IptcEdit) => {
      const write = async () => {
        try {
          const result = await invoke<import("../bindings/IptcEditResult").IptcEditResult>("edit_asset_iptc", {
            targets,
            action,
          });
          if (action.kind === "text" && result.text) applyAssetValues(result.text, { [action.field]: action.value });
          if (result.catalogKeywordsChanged) refreshAfterWrite();
          if (result.errors.length || result.warnings.length) setIptcEditIssues(result);
          setToast(
            "Upravené súbory: " +
              result.written +
              ", bez zmeny: " +
              result.unchanged +
              ", chyby: " +
              result.errors.length,
          );
          window.setTimeout(() => setToast(""), 6000);
        } catch (error) {
          setToast(String(error));
          window.setTimeout(() => setToast(""), 6000);
        } finally {
          if (targets.length > 5) setOperationProgress("");
          setIptcToken((token) => token + 1);
        }
      };
      valueWrites.current = valueWrites.current.then(write);
      return valueWrites.current;
    },
    [applyAssetValues, refreshAfterWrite],
  );
  const openIptcWrite = async (ids: number[]) => {
    if (appInfo?.readonly || !ids.length) return;
    await valueWrites.current;
    setIptcIds(ids);
  };
  const finishIptcWrite = (result: IptcSummary) => {
    setOperationProgress("");
    setIptcToken((token) => token + 1);
    if (result.written) refreshAfterWrite();
    setToast("Zapísané: " + result.written + ", bez zmeny: " + result.unchanged + ", chyby: " + result.errors.length);
    window.setTimeout(() => setToast(""), 8000);
  };
  return {
    iptcIds,
    setIptcIds,
    iptcToken,
    iptcEditIssues,
    setIptcEditIssues,
    editIptc,
    openIptcWrite,
    finishIptcWrite,
  };
}

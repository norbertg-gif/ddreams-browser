import { createContext, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type FolderRuleMode = "include" | "exclude";
export type FolderRule = { path: string; mode: FolderRuleMode; created: string };
export type FolderRuleAction = (path: string, mode: FolderRuleMode | null) => void;

/** Fired after any rule change so every view of the rules reloads. */
export const FOLDER_RULES_EVENT = "ddreams-folder-rules";

export type FolderCatalogueState = {
  effective: FolderRuleMode | null;
  explicitMode: FolderRuleMode | null;
  rulePath: string | null;
  listedCount: number;
  hasCataloguedDescendant: boolean;
  partiallyExcluded: boolean;
};

export function useFolderRules(): FolderRule[] {
  const [rules, setRules] = useState<FolderRule[]>([]);
  useEffect(() => {
    let active = true;
    const load = () => void invoke<FolderRule[]>("get_folder_rules").then(r => { if (active) setRules(r); }).catch(() => {});
    load();
    window.addEventListener(FOLDER_RULES_EVENT, load);
    return () => { active = false; window.removeEventListener(FOLDER_RULES_EVENT, load); };
  }, []);
  return rules;
}

export const FolderRulesContext = createContext<{ states: Record<string, FolderCatalogueState>; onRule?: FolderRuleAction }>({ states: {} });

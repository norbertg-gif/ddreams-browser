import { createContext, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type FolderRuleMode = "include" | "exclude";
export type FolderRule = import("./bindings/FolderRule").FolderRule;
export type FolderRuleAction = (path: string, mode: FolderRuleMode | null) => void;

/** Fired after any rule change so every view of the rules reloads. */
export const FOLDER_RULES_EVENT = "ddreams-folder-rules";

export type FolderCatalogueState = import("./bindings/FolderCatalogueState").FolderCatalogueState;

export function useFolderRules(): FolderRule[] {
  const [rules, setRules] = useState<FolderRule[]>([]);
  useEffect(() => {
    let active = true;
    const load = () =>
      void invoke<FolderRule[]>("get_folder_rules")
        .then((r) => {
          if (active) setRules(r);
        })
        .catch(() => {});
    load();
    window.addEventListener(FOLDER_RULES_EVENT, load);
    return () => {
      active = false;
      window.removeEventListener(FOLDER_RULES_EVENT, load);
    };
  }, []);
  return rules;
}

export const FolderRulesContext = createContext<{
  states: Record<string, FolderCatalogueState>;
  onRule?: FolderRuleAction;
}>({ states: {} });

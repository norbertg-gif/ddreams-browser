import { createContext, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type FolderRuleMode = "include" | "exclude";
export type FolderRule = { path: string; mode: FolderRuleMode; created: string };
export type FolderRuleAction = (path: string, mode: FolderRuleMode | null) => void;

/** Fired after any rule change so every view of the rules reloads. */
export const FOLDER_RULES_EVENT = "ddreams-folder-rules";

const normalize = (path: string) => path.replace(/\//g, "\\").replace(/\\+$/, "").toLocaleLowerCase("sk");

/** Longest matching rule on whole path components — same resolution as the backend. */
export function effectiveRule(rules: FolderRule[], path: string | null | undefined): FolderRule | null {
  if (!path) return null;
  const target = normalize(path);
  let best: FolderRule | null = null;
  for (const rule of rules) {
    const root = normalize(rule.path);
    if (target === root || target.startsWith(root + "\\")) { if (!best || root.length > normalize(best.path).length) best = rule; }
  }
  return best;
}

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

export const FolderRulesContext = createContext<{ rules: FolderRule[]; onRule?: FolderRuleAction }>({ rules: [] });

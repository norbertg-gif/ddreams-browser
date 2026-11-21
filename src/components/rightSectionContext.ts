import { createContext, useContext } from "react";
import type { AiSectionData } from "./AiPanel";
import type { CatalogSectionData } from "./CatalogPanel";
import type { PropertiesSectionData } from "./RightPanel";

type RightSectionModels = {
  properties: PropertiesSectionData;
  catalog: CatalogSectionData;
  ai: AiSectionData;
};

export const RightSectionContext = createContext<Partial<RightSectionModels>>({});

export function useRightSectionData<K extends keyof RightSectionModels>(pane: K): RightSectionModels[K] {
  const state = useContext(RightSectionContext)[pane];
  if (!state) throw new Error("Missing model for dock section");
  return state;
}

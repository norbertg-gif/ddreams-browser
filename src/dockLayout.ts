export const panelTitles = {
  folders: "Priečinky", calendar: "Kalendár", properties: "Vlastnosti", catalog: "Katalóg", ai: "AI",
};
export type PanelId = keyof typeof panelTitles;
export type SectionId = `${"catalog" | "ai" | "properties"}.${string}`;
export type TabId = PanelId | `stack:${string}`;
export type DockSideName = "left" | "right";
export type DockGroup = { id: string; panels: TabId[]; active: TabId; width: number; collapsed?: boolean };
export type DockLayout = {
  left: DockGroup[];
  right: DockGroup[];
  sections: Partial<Record<TabId, SectionId[]>>;
  hidden: SectionId[];
  collapsed: Record<string, boolean>;
};
export type LayoutPreset = { name: string; layout: DockLayout };
export type DockDrag = { kind: "section"; id: SectionId } | { kind: "tab"; id: TabId };
export type DockTarget =
  | { kind: "stack"; tab: TabId; section?: SectionId; after?: boolean }
  | { kind: "tab"; group: string; before?: TabId }
  | { kind: "column"; side: DockSideName; before?: string };

const titles: Partial<Record<SectionId, string>> = {
  "catalog.edit": "Upraviť vybrané fotky", "catalog.matches": "Zhody kategórií",
  "catalog.categories": "Kategórie", "catalog.special": "Špeciálne položky",
  "catalog.people": "Osoby", "catalog.unknownFaces": "Neznáme tváre",
  "catalog.ratings": "Hodnotenie", "catalog.labels": "Farebné štítky", "catalog.colors": "Farby",
  "catalog.keywords": "Kľúčové slová", "catalog.autoCategories": "Auto kategórie",
  "ai.incoming": "Kam to patrí?", "ai.aiDuplicates": "Duplicity (AI)", "ai.aiSuggestions": "Návrhy AI",
  "ai.aiClusters": "Nezaradené skupiny", "ai.quality": "Technická kvalita",
  "properties.faces": "Tváre",
};
export const propertySection = (name: string): SectionId => `properties.group:${name}`;
export const sectionTitle = (id: SectionId) => titles[id] ?? id.slice("properties.group:".length);
export const sectionHome = (id: SectionId) => id.split(".", 1)[0] as "catalog" | "ai" | "properties";
export const isNav = (id: TabId) => id === "folders" || id === "calendar";
const isPanel = (value: unknown): value is PanelId => typeof value === "string" && Object.prototype.hasOwnProperty.call(panelTitles, value);
const isSection = (value: unknown): value is SectionId => typeof value === "string"
  && (Object.prototype.hasOwnProperty.call(titles, value) || value.startsWith("properties.group:") && value.length > "properties.group:".length);
const isTab = (value: unknown): value is TabId => isPanel(value) || typeof value === "string" && value.startsWith("stack:");
const sectionIds = [
  ...Object.keys(titles).filter(id => !id.startsWith("properties.")) as SectionId[],
  ...["IPTC", "EXIF", "Katalóg", "Všetky metadáta"].map(propertySection), "properties.faces",
] as SectionId[];
const uid = () => `g${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const record = (value: unknown): Record<string, unknown> => value && typeof value === "object" && !Array.isArray(value)
  ? value as Record<string, unknown> : {};
const array = (value: unknown): unknown[] => Array.isArray(value) ? value : [];

export function defaultLayout(): DockLayout {
  return {
    left: [{ id: "g-nav", panels: ["folders", "calendar"], active: "folders", width: 260 }],
    right: [{ id: "g-props", panels: ["properties", "ai"], active: "properties", width: 320 },
      { id: "g-catalog", panels: ["catalog"], active: "catalog", width: 280 }],
    sections: {
      properties: sectionIds.filter(id => sectionHome(id) === "properties"),
      catalog: sectionIds.filter(id => sectionHome(id) === "catalog"),
      ai: sectionIds.filter(id => sectionHome(id) === "ai"),
    },
    hidden: [],
    collapsed: {},
  };
}

export function sectionCollapsed(layout: Pick<DockLayout, "collapsed">, id: string): boolean {
  if (Object.prototype.hasOwnProperty.call(layout.collapsed, id)) return layout.collapsed[id];
  return id === "catalog.edit" || id.startsWith("catalog.autoGroup:") ||
    id.startsWith("properties.group:") && !["IPTC", "EXIF", "Katalóg"].some(name => id === propertySection(name));
}

export function tabTitle(layout: DockLayout, tab: TabId): string {
  if (isPanel(tab)) return panelTitles[tab];
  const first = layout.sections[tab]?.[0];
  return first ? sectionTitle(first) : "";
}

export function patchGroup(layout: DockLayout, id: string, change: Partial<DockGroup>): DockLayout {
  return {
    ...layout,
    left: layout.left.map(group => group.id === id ? { ...group, ...change } : group),
    right: layout.right.map(group => group.id === id ? { ...group, ...change } : group),
  };
}

function removeTab(layout: DockLayout, tab: TabId): DockLayout {
  const remove = (groups: DockGroup[]) => groups.flatMap(group => {
    if (!group.panels.includes(tab)) return [group];
    const panels = group.panels.filter(item => item !== tab);
    return panels.length ? [{ ...group, panels, active: group.active === tab ? panels[0] : group.active }] : [];
  });
  return { ...layout, left: remove(layout.left), right: remove(layout.right) };
}

function replaceSections(layout: DockLayout, tab: TabId, items: SectionId[]): DockLayout {
  const sections = { ...layout.sections };
  if (items.length) {
    sections[tab] = items;
    return { ...layout, sections };
  }
  delete sections[tab];
  return removeTab({ ...layout, sections }, tab);
}

const groupOf = (layout: DockLayout, tab: TabId) => [...layout.left, ...layout.right].find(group => group.panels.includes(tab));
function tabOf(layout: DockLayout, section: SectionId): TabId | undefined {
  return (Object.keys(layout.sections) as TabId[]).find(tab => layout.sections[tab]?.includes(section));
}

function addToHome(layout: DockLayout, section: SectionId): DockLayout {
  const tab = sectionHome(section);
  const items = [...(layout.sections[tab] ?? [])];
  const before = section.startsWith("properties.group:") ? items.indexOf(propertySection("Všetky metadáta")) : -1;
  items.splice(before < 0 ? items.length : before, 0, section);
  const next = replaceSections(layout, tab, items);
  if (groupOf(next, tab)) return next;
  return { ...next, right: [...next.right, { id: uid(), panels: [tab], active: tab, width: 280 }] };
}

export function ensureSections(layout: DockLayout, ids: readonly SectionId[]): DockLayout {
  return ids.reduce((next, id) => tabOf(next, id) || next.hidden.includes(id) ? next : addToHome(next, id), layout);
}

export function knownSections(layout: DockLayout): SectionId[] {
  return [...Object.values(layout.sections).flatMap(items => items ?? []), ...layout.hidden];
}

/** Repairs persisted layouts and gives legacy pane tabs their original section stacks. */
export function normalizeDockLayout(value: unknown, legacyCollapsed: Record<string, boolean> = {}): DockLayout {
  if (!value || typeof value !== "object") return { ...defaultLayout(), collapsed: legacyCollapsed };
  const input = record(value);
  const modern = !!input.sections;
  const source = record(modern ? input.sections : defaultLayout().sections);
  const hidden = [...new Set(array(input.hidden).filter(isSection))];
  const seenSections = new Set(hidden);
  const seenTabs = new Set<TabId>();
  const seenGroups = new Set<string>();
  const sections: DockLayout["sections"] = {};
  const readSide = (raw: unknown) => array(raw).flatMap(entry => {
    const group = record(entry);
    const panels = array(group.panels).filter((tab): tab is TabId => {
      if (!isTab(tab) || seenTabs.has(tab)) return false;
      if (!isNav(tab)) {
        const items = array(source[tab]).filter((id): id is SectionId =>
          isSection(id) && !seenSections.has(id) && !!seenSections.add(id));
        if (!items.length) return false;
        sections[tab] = items;
      }
      seenTabs.add(tab);
      return true;
    });
    if (!panels.length) return [];
    const id = typeof group.id === "string" && group.id && !seenGroups.has(group.id) ? group.id : uid();
    seenGroups.add(id);
    const active = isTab(group.active) && panels.includes(group.active) ? group.active : panels[0];
    const width = typeof group.width === "number" && group.width >= 150 && group.width <= 1200 ? group.width : 280;
    return [{ id, panels, active, width, collapsed: group.collapsed === true }];
  });
  const left = readSide(input.left);
  const right = readSide(input.right);
  for (const panel of ["folders", "calendar"] as const) {
    if (!seenTabs.has(panel)) left.push({ id: uid(), panels: [panel], active: panel, width: 280, collapsed: false });
  }
  const collapsed = modern ? Object.fromEntries(Object.entries(record(input.collapsed)).filter(([, flag]) =>
    typeof flag === "boolean")) as Record<string, boolean> : legacyCollapsed;
  return ensureSections({ left, right, sections, hidden, collapsed }, sectionIds);
}

export function hideSection(layout: DockLayout, section: SectionId): DockLayout {
  const tab = tabOf(layout, section);
  if (!tab) return layout;
  return replaceSections({ ...layout, hidden: [...layout.hidden, section] }, tab,
    layout.sections[tab]!.filter(id => id !== section));
}

export function showSection(layout: DockLayout, section: SectionId): DockLayout {
  if (!layout.hidden.includes(section)) return layout;
  const next = addToHome({ ...layout, hidden: layout.hidden.filter(id => id !== section) }, section);
  const tab = sectionHome(section);
  return patchGroup(next, groupOf(next, tab)!.id, { active: tab, collapsed: false });
}

export function moveDockItem(layout: DockLayout, drag: DockDrag, target: DockTarget): DockLayout {
  const sourceTab = drag.kind === "section" ? tabOf(layout, drag.id) : drag.id;
  if (!sourceTab) return layout;
  const sourceGroup = groupOf(layout, sourceTab);
  if (!sourceGroup) return layout;

  if (target.kind === "stack") {
    const destination = layout.sections[target.tab];
    if (drag.kind !== "section" || !destination || target.section === drag.id) return layout;
    const items = destination.filter(id => id !== drag.id);
    const index = target.section ? items.indexOf(target.section) : -1;
    items.splice(index < 0 ? items.length : index + Number(!!target.after), 0, drag.id);
    const next = sourceTab === target.tab ? layout : replaceSections(layout, sourceTab,
      layout.sections[sourceTab]!.filter(id => id !== drag.id));
    return replaceSections(next, target.tab, items);
  }

  const standalone = drag.kind === "section" && sourceTab.startsWith("stack:") && layout.sections[sourceTab]?.length === 1;
  const movingTab = drag.kind === "tab" || standalone;
  const tab: TabId = movingTab ? sourceTab : `stack:${uid()}`;
  if (target.kind === "tab" && target.before === tab) return layout;
  let next = movingTab ? removeTab(layout, tab) : replaceSections(layout, sourceTab,
    layout.sections[sourceTab]!.filter(id => id !== drag.id));
  if (!movingTab && drag.kind === "section") next = replaceSections(next, tab, [drag.id]);

  if (target.kind === "tab") {
    const group = [...next.left, ...next.right].find(group => group.id === target.group);
    if (!group) return layout;
    const panels = [...group.panels];
    const position = target.before ? panels.indexOf(target.before) : -1;
    panels.splice(position < 0 ? panels.length : position, 0, tab);
    return patchGroup(next, group.id, { panels, active: tab, collapsed: false });
  }
  const original = layout[target.side];
  const index = target.before ? original.findIndex(group => group.id === target.before) : original.length;
  if (index < 0) return layout;
  const anchor = original.slice(index).find(group => next[target.side].some(current => current.id === group.id));
  const groups = [...next[target.side]];
  groups.splice(anchor ? groups.findIndex(group => group.id === anchor.id) : groups.length, 0,
    { id: uid(), panels: [tab], active: tab, width: sourceGroup.width });
  return { ...next, [target.side]: groups };
}

export function savePreset(presets: LayoutPreset[], name: string, layout: DockLayout): LayoutPreset[] {
  const trimmed = name.trim();
  if (!trimmed || presets.some(preset => preset.name === trimmed)) return presets;
  return [...presets, { name: trimmed, layout }];
}

export function renamePreset(presets: LayoutPreset[], name: string, replacement: string): LayoutPreset[] {
  const trimmed = replacement.trim();
  if (!trimmed || presets.some(preset => preset.name === trimmed)) return presets;
  return presets.map(preset => preset.name === name ? { ...preset, name: trimmed } : preset);
}

export function switchPreset(current: DockLayout, preset: LayoutPreset): DockLayout {
  return ensureSections(normalizeDockLayout(preset.layout), knownSections(current));
}

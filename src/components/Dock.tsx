import {
  createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState,
  type Dispatch, type DragEvent, type MouseEvent, type ReactNode, type SetStateAction,
} from "react";
import { createPortal } from "react-dom";
import Splitter from "./Splitter";
import { readStored } from "../storage";
import { readUiState, usePersistentState, useTransientDetails } from "../uiState";
import {
  defaultLayout, ensureSections, hideSection, isNav, knownSections, moveDockItem, normalizeDockLayout,
  patchGroup, propertySection, renamePreset, savePreset, sectionCollapsed, sectionHome, sectionTitle,
  showSection, switchPreset, tabTitle, type DockDrag, type DockLayout, type DockSideName,
  type DockTarget, type LayoutPreset, type SectionId, type TabId,
} from "../dockLayout";
import "./Dock.css";

type SetLayout = Dispatch<SetStateAction<DockLayout>>;
type ChromeId = "propertiesTop" | "propertiesBottom" | "catalogIntro";
const DRAG_TYPE = "application/x-ddreams-panel";
const STRIP = 28;

function readLegacyCollapse() {
  const result: Record<string, boolean> = {};
  const catalog = readUiState<Record<string, boolean>>("ddreams.ui.catalog.sections", {}, "ddreams.catalogCollapsed");
  const ai = readUiState<Record<string, boolean>>("ddreams.ui.ai.sections", {}, "ddreams.catalogCollapsed");
  for (const [key, value] of Object.entries(catalog)) if (typeof value === "boolean") result["catalog." + key] = value;
  for (const [key, value] of Object.entries(ai)) if (typeof value === "boolean") result["ai." + key] = value;
  result["catalog.edit"] = !readUiState("ddreams.ui.catalog.editSelection", false);
  const properties = readUiState("ddreams.ui.properties.groups", ["IPTC", "EXIF", "Katalóg"], "ddreams.metadataOpen");
  for (const name of new Set(["IPTC", "EXIF", "Katalóg", ...properties])) {
    if (typeof name === "string") result[propertySection(name === "ACDSee" ? "Katalóg" : name)] =
      !properties.includes(name) && !(name === "Katalóg" && properties.includes("ACDSee"));
  }
  for (const group of readUiState<string[]>("ddreams.ui.catalog.autoGroups", [])) {
    if (typeof group === "string") result["catalog.autoGroup:" + group] = false;
  }
  return result;
}

export function useDockLayout() {
  const [initial] = useState(() => normalizeDockLayout(readStored("ddreams.dockLayout.v1", null), readLegacyCollapse()));
  return usePersistentState("ddreams.ui.dock.layout", initial, undefined, normalizeDockLayout);
}

/** Section portals always target the same nodes, including when their tabs are hidden. */
export function useDockHosts() {
  return useMemo(() => {
    const host = (className: string) => Object.assign(document.createElement("div"), { className });
    const sections = new Map<SectionId, HTMLDivElement>();
    return {
      folders: host("dock-host dock-host-folders"),
      calendar: host("dock-host dock-host-calendar"),
      chrome: {
        propertiesTop: host("dock-chrome"), propertiesBottom: host("dock-chrome"),
        catalogIntro: host("dock-chrome"),
      },
      section(id: SectionId) {
        let node = sections.get(id);
        if (!node) {
          node = host("dock-section-content " + (sectionHome(id) === "catalog" ? "filter-zone-content" : ""));
          sections.set(id, node);
        }
        return node;
      },
    };
  }, []);
}
type Hosts = ReturnType<typeof useDockHosts>;
type DragControls = { start: (drag: DockDrag) => void; end: () => void };
type SectionMenu = { x: number; y: number; id: SectionId };
type SectionPresence = Partial<Record<SectionId, boolean>>;

export function useDockPresence() {
  return useState<SectionPresence>({ "catalog.matches": false, "properties.faces": false });
}

function startDrag(event: DragEvent, drag: DockDrag, controls: DragControls) {
  event.stopPropagation();
  event.dataTransfer.setData(DRAG_TYPE, JSON.stringify(drag));
  event.dataTransfer.effectAllowed = "move";
  controls.start(drag);
}

const SectionContext = createContext<{
  hosts: Hosts; setLayout: SetLayout; controls: DragControls; collapsed: DockLayout["collapsed"];
  setMenu: Dispatch<SetStateAction<SectionMenu | null>>;
  setPresence: Dispatch<SetStateAction<SectionPresence>>;
} | null>(null);

export function DockSections({ hosts, setLayout, controls, collapsed, setPresence, children }: {
  hosts: Hosts; setLayout: SetLayout; controls: DragControls; collapsed: DockLayout["collapsed"];
  setPresence: Dispatch<SetStateAction<SectionPresence>>; children: ReactNode;
}) {
  const [menu, setMenu] = useState<SectionMenu | null>(null);
  const value = useMemo(() => ({ hosts, setLayout, controls, collapsed, setMenu, setPresence }),
    [hosts, setLayout, controls, collapsed, setPresence]);
  return <SectionContext.Provider value={value}>{children}
    {menu && <DockSectionMenu menu={menu} close={() => setMenu(null)} hide={() => {
      setLayout(layout => hideSection(layout, menu.id));
      setMenu(null);
    }}/>}
  </SectionContext.Provider>;
}
function useSections() {
  const state = useContext(SectionContext);
  if (!state) throw new Error("Dock section needs a provider");
  return state;
}

export function useRegisterSections(ids: readonly SectionId[]) {
  const { setLayout } = useSections();
  useLayoutEffect(() => { setLayout(layout => ensureSections(layout, ids)); }, [ids, setLayout]);
}

export function useSectionCollapse() {
  const { collapsed, setLayout } = useSections();
  return {
    isCollapsed: (id: string) => sectionCollapsed({ collapsed }, id),
    setCollapsed: (id: string, flag?: boolean) => {
      setLayout(layout => {
        const value = sectionCollapsed(layout, id);
        const next = flag ?? !value;
        return next === value ? layout : { ...layout, collapsed: { ...layout.collapsed, [id]: next } };
      });
    },
  };
}

export function DockSection({ id, available, children }: { id: SectionId; available?: boolean; children: ReactNode }) {
  const { hosts, setPresence } = useSections();
  useLayoutEffect(() => {
    if (available === undefined) return;
    setPresence(current => current[id] === available ? current : { ...current, [id]: available });
  }, [id, available, setPresence]);
  return createPortal(children, hosts.section(id));
}

export function DockChrome({ id, children }: { id: ChromeId; children: ReactNode }) {
  return createPortal(children, useSections().hosts.chrome[id]);
}

export function SectionHeading({ id, title = sectionTitle(id), tools, count, metadata = false }: {
  id: SectionId; title?: string; tools?: ReactNode; count?: number; metadata?: boolean;
}) {
  const { isCollapsed, setCollapsed } = useSectionCollapse();
  const collapsed = isCollapsed(id);
  return <div className={metadata ? "dock-metadata-heading" : "catalog-section-heading"} {...useSectionMenu(id)}>
    <button className={metadata ? "accordion-head" : undefined} aria-expanded={!collapsed} onClick={() => setCollapsed(id)}>
      {metadata ? <><span>{collapsed ? "▸" : "▾"} {title}</span><span>{count}</span></>
        : <><span>{collapsed ? "▸" : "▾"}</span>{title}</>}
    </button>
    {tools}<SectionHandle id={id}/>
  </div>;
}

export function SectionHandle({ id }: { id: SectionId }) {
  const { controls } = useSections();
  const menu = useSectionMenu(id);
  return <button type="button" className="dock-section-handle" draggable aria-label={`Presunúť sekciu ${sectionTitle(id)}`}
    title="Potiahni sekciu. Pravým klikom otvoríš ponuku."
    {...menu} onClick={menu.onContextMenu}
    onDragStart={event => startDrag(event, { kind: "section", id }, controls)} onDragEnd={controls.end}>⠿</button>;
}

export function useSectionMenu(id: SectionId) {
  const { setMenu } = useSections();
  return {
    onContextMenu: (event: MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const anchor = event.currentTarget.getBoundingClientRect();
      setMenu({ id, x: event.clientX || anchor.left, y: event.clientY || anchor.bottom });
    },
  };
}

function DockSectionMenu({ menu, close, hide }: { menu: SectionMenu; close: () => void; hide: () => void }) {
  useEffect(() => {
    document.addEventListener("pointerdown", close);
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [close]);
  return createPortal(<div className="category-context-menu dock-menu"
    style={{ left: Math.max(0, Math.min(menu.x, window.innerWidth - 190)), top: Math.max(0, Math.min(menu.y, window.innerHeight - 70)) }}
    onPointerDown={event => event.stopPropagation()}>
    <button onClick={hide}>Skryť sekciu</button>
  </div>, document.body);
}

export function useDockDrag() {
  const [dragging, setDragging] = useState<DockDrag | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const controls = useMemo<DragControls>(() => ({
    start: drag => {
      window.clearTimeout(timer.current);
      // Start showing targets after the browser has captured the drag image.
      timer.current = window.setTimeout(() => setDragging(drag), 0);
    },
    end: () => {
      window.clearTimeout(timer.current);
      document.querySelectorAll("[data-dock-over]").forEach(node => node.removeAttribute("data-dock-over"));
      setDragging(null);
    },
  }), []);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return { dragging, controls };
}

function Host({ node, className, hidden, drop }: {
  node: HTMLDivElement; className?: string; hidden?: boolean; drop?: ReturnType<typeof dropEvents>;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { ref.current?.appendChild(node); }, [node]);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element || !drop) return;
    // Section content is a portal. The drop follows its DOM ancestry, not its React owner.
    const listeners = {
      dragover: (event: globalThis.DragEvent) => drop.onDragOverCapture({ event, target: element }),
      dragleave: (event: globalThis.DragEvent) => drop.onDragLeave({ event, target: element }),
      drop: (event: globalThis.DragEvent) => drop.onDropCapture({ event, target: element }),
    };
    element.addEventListener("dragover", listeners.dragover, true);
    element.addEventListener("dragleave", listeners.dragleave);
    element.addEventListener("drop", listeners.drop, true);
    return () => {
      element.removeEventListener("dragover", listeners.dragover, true);
      element.removeEventListener("dragleave", listeners.dragleave);
      element.removeEventListener("drop", listeners.drop, true);
      delete element.dataset.dockOver;
    };
  }, [drop]);
  return <div ref={ref} className={className} style={hidden ? { display: "none" } : undefined}/>;
}

type DropInput = { event: DragEvent | globalThis.DragEvent; target: HTMLElement };
function dropEvents(controls: DragControls, apply: (drag: DockDrag, input: DropInput) => void,
  mark: (input: DropInput) => string = () => "target") {
  return {
    onDragOverCapture: ({ event, target }: DropInput) => {
      if (!event.dataTransfer?.types.includes(DRAG_TYPE)) return;
      event.preventDefault();
      event.stopPropagation();
      event.dataTransfer.dropEffect = "move";
      const value = mark({ event, target });
      if (target.dataset.dockOver !== value) target.dataset.dockOver = value;
    },
    onDragLeave: ({ event, target }: DropInput) => {
      if (!(event.relatedTarget instanceof Node) || !target.contains(event.relatedTarget))
        delete target.dataset.dockOver;
    },
    onDropCapture: ({ event, target }: DropInput) => {
      if (!event.dataTransfer?.types.includes(DRAG_TYPE)) return;
      let drag: DockDrag;
      try { drag = JSON.parse(event.dataTransfer.getData(DRAG_TYPE)); } catch { return; }
      if (!drag || (drag.kind !== "section" && drag.kind !== "tab") || typeof drag.id !== "string") return;
      event.preventDefault();
      event.stopPropagation();
      controls.end();
      apply(drag, { event, target });
    },
  };
}

function reactDropEvents(drop: ReturnType<typeof dropEvents>) {
  return {
    onDragOverCapture: (event: DragEvent<HTMLElement>) => drop.onDragOverCapture({ event, target: event.currentTarget }),
    onDragLeave: (event: DragEvent<HTMLElement>) => drop.onDragLeave({ event, target: event.currentTarget }),
    onDropCapture: (event: DragEvent<HTMLElement>) => drop.onDropCapture({ event, target: event.currentTarget }),
  };
}

function lowerHalf({ event, target }: DropInput) {
  const box = target.getBoundingClientRect();
  return event.clientY >= box.top + box.height / 2;
}

const isNavTab = (tab: TabId): tab is "folders" | "calendar" => isNav(tab);
type SideProps = {
  side: DockSideName; layout: DockLayout; setLayout: SetLayout; hosts: Hosts; hideNav: boolean;
  dragging: DockDrag | null; controls: DragControls; sectionPresence: SectionPresence;
};

export function DockSide({ side, layout, setLayout, hosts, hideNav, dragging, controls, sectionPresence }: SideProps) {
  const [menu, setMenu] = useState<{ x: number; y: number; tab: TabId; group: string } | null>(null);
  const columns = useRef(new Map<string, HTMLElement>());
  const groups = layout[side];
  const allGroups = [...layout.left, ...layout.right];
  const hasSections = (tab: TabId) => layout.sections[tab]?.some(id => sectionPresence[id] !== false);
  const visibleStacks = allGroups.flatMap(group => group.panels).filter(hasSections);
  const propertyTab = hasSections("properties") ? "properties" :
    visibleStacks.find(tab => layout.sections[tab]?.some(id => sectionHome(id) === "properties"));
  const catalogTab = hasSections("catalog") ? "catalog" :
    visibleStacks.find(tab => layout.sections[tab]?.some(id =>
      id === "catalog.categories" || id === "catalog.keywords" || id === "catalog.matches"));
  const patch = (id: string, change: Parameters<typeof patchGroup>[2]) => setLayout(current => patchGroup(current, id, change));
  const move = (drag: DockDrag, target: DockTarget) => setLayout(current => moveDockItem(current, drag, target));
  const target = (destination: DockTarget) => reactDropEvents(dropEvents(controls, drag => move(drag, destination)));
  const newColumn = (before?: string) => target({ kind: "column", side, before });
  const edge = dragging && <div key={side} className={`dock-window-edge dock-window-edge-${side}`}
    title={side === "left" ? "Nový stĺpec úplne vľavo" : "Nový stĺpec úplne vpravo"}
    {...newColumn(side === "left" ? groups[0]?.id : undefined)}/>;

  const body = groups.map((group, index) => {
    const visible = group.panels.filter(tab => isNav(tab) ? !hideNav : hasSections(tab));
    if (!visible.length) return null;
    const active = visible.includes(group.active) ? group.active : visible[0];
    const tabDrop = target({ kind: "tab", group: group.id });
    const splitter = <div key={`${group.id}-split`} className="dock-column-divider">
      {!group.collapsed && <Splitter direction="x" invert={side === "right"} value={group.width} min={170} max={900}
        onChange={width => {
          const column = columns.current.get(group.id);
          if (column) column.style.width = `${width}px`;
        }} onCommit={width => patch(group.id, { width })} onReset={() => patch(group.id, { width: 280 })}/>}
      {dragging && <div className="dock-column-drop" title="Nový stĺpec"
        {...newColumn(side === "right" ? group.id : groups[index + 1]?.id)}/>}
    </div>;
    const column = <aside key={group.id} className={`dock-group dock-${side}${group.collapsed ? " dock-collapsed" : ""}`}
      ref={node => { if (node) columns.current.set(group.id, node); else columns.current.delete(group.id); }}
      style={{ width: group.collapsed ? STRIP : group.width }}>
      {group.collapsed
        ? <button className="dock-strip-button" title="Rozbaliť panel" onClick={() => patch(group.id, { collapsed: false })} {...tabDrop}>
          <span className="dock-expand">{side === "left" ? "›" : "‹"}</span>
          {visible.map(tab => <span key={tab} className="dock-vertical-title">{tabTitle(layout, tab)}</span>)}
        </button>
        : <div className="dock-tabs">
          {visible.map(tab => <button key={tab} className={tab === active ? "active" : ""} draggable
            title="Ťahaním presunieš panel, pravým klikom otvoríš ponuku"
            onClick={() => patch(group.id, { active: tab })}
            onDragStart={event => startDrag(event, { kind: "tab", id: tab }, controls)} onDragEnd={controls.end}
            onContextMenu={event => { event.preventDefault(); setMenu({ x: event.clientX, y: event.clientY, tab, group: group.id }); }}
            {...reactDropEvents(dropEvents(controls, (drag, { event, target }) => {
              const box = target.getBoundingClientRect();
              const before = event.clientX < box.left + box.width / 2 ? tab : group.panels[group.panels.indexOf(tab) + 1];
              move(drag, { kind: "tab", group: group.id, before });
            }))}>{tabTitle(layout, tab)}</button>)}
          <span className="dock-tabs-spacer" {...tabDrop}/>
          <button className="dock-collapse" title="Zbaliť panel" onClick={() => patch(group.id, { collapsed: true })}>{side === "left" ? "‹" : "›"}</button>
        </div>}
      {group.panels.map(tab => {
        const hidden = group.collapsed || tab !== active;
        if (isNavTab(tab)) return <Host key={tab} node={hosts[tab]} hidden={hidden} className="dock-slot"/>;
        const sections = layout.sections[tab] ?? [];
        const pinned = tab === "catalog" && sections[0] === "catalog.edit" || tab === "ai" && sections[0] === "ai.incoming";
        const render = (id: SectionId, fixed = false) => <Host key={id} node={hosts.section(id)}
          className={`dock-section-slot dock-section-${sectionHome(id)}${fixed ? " dock-section-pinned" : ""}`}
          drop={dragging?.kind === "section" ? dropEvents(controls,
            (drag, event) => move(drag, { kind: "stack", tab, section: id, after: lowerHalf(event) }),
            event => lowerHalf(event) ? "after" : "before") : undefined}/>;
        return <div key={tab} className={`dock-slot right-pane dock-stack dock-stack-${tab.split(":")[0]}`}
          style={hidden ? { display: "none" } : undefined}>
          {pinned && render(sections[0], true)}
          <div className="dock-stack-scroll">
            {tab === propertyTab && <Host node={hosts.chrome.propertiesTop}/>}
            {tab === catalogTab && <Host node={hosts.chrome.catalogIntro}/>}
            {(pinned ? sections.slice(1) : sections).map(id => render(id))}
            {tab === propertyTab && <Host className="dock-properties-footer" node={hosts.chrome.propertiesBottom}/>}
            {dragging?.kind === "section" && <div className="dock-stack-tail" title="Pridať na koniec" {...target({ kind: "stack", tab })}/>}
          </div>
        </div>;
      })}
      {dragging?.kind === "tab" && !group.collapsed && <div className="dock-tab-drop"
        {...tabDrop}>Ako záložka</div>}
    </aside>;
    return side === "left" ? [column, splitter] : [splitter, column];
  });
  const reset = () => setLayout(current => ensureSections(defaultLayout(), knownSections(current)));
  return <>
    {side === "left" && edge}{body}{side === "right" && edge}
    {menu && createPortal(<div className="category-context-menu dock-menu"
      style={{ left: Math.max(0, Math.min(menu.x, window.innerWidth - 270)), top: Math.max(0, Math.min(menu.y, window.innerHeight - 260)) }}
      onMouseLeave={() => setMenu(null)}>
      <small>Panel {tabTitle(layout, menu.tab)}</small>
      <button onClick={() => { move({ kind: "tab", id: menu.tab }, { kind: "column", side: "left", before: layout.left[0]?.id }); setMenu(null); }}>Samostatný stĺpec vľavo</button>
      <button onClick={() => { move({ kind: "tab", id: menu.tab }, { kind: "column", side: "right" }); setMenu(null); }}>Samostatný stĺpec vpravo</button>
      {allGroups.filter(group => group.id !== menu.group).map(group =>
        <button key={group.id} onClick={() => { move({ kind: "tab", id: menu.tab }, { kind: "tab", group: group.id }); setMenu(null); }}>
          Ako záložka k: {group.panels.map(tab => tabTitle(layout, tab)).join(", ")}
        </button>)}
      <hr/><button onClick={() => { patch(menu.group, { collapsed: true }); setMenu(null); }}>Zbaliť panel</button>
      <button onClick={() => { reset(); setMenu(null); }}>Obnoviť pôvodné rozloženie</button>
    </div>, document.body)}
  </>;
}

function normalizePresets(value: LayoutPreset[]) {
  return value.filter(preset => typeof preset?.name === "string" && preset.name.trim() && preset.layout);
}

export function DockLayoutMenu({ layout, setLayout }: { layout: DockLayout; setLayout: SetLayout }) {
  const [presets, setPresets] = usePersistentState<LayoutPreset[]>("ddreams.ui.dock.presets", [], undefined, normalizePresets);
  const details = useTransientDetails();
  const ref = useRef<HTMLDetailsElement>(null);
  const [edit, setEdit] = useState<{ original?: string; value: string } | null>(null);
  const close = useCallback(() => {
    if (ref.current) ref.current.open = false;
    setEdit(null);
  }, []);
  useEffect(() => {
    if (!details.open) return;
    const outside = (event: PointerEvent) => { if (event.target instanceof Node && !ref.current?.contains(event.target)) close(); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [details.open, close]);
  const invalid = !edit?.value.trim() || presets.some(preset => preset.name === edit.value.trim());
  return <details className="dock-layout-menu" ref={ref} {...details}><summary>Rozloženie</summary>
    <div className="dock-layout-pop">
      <button onClick={() => setEdit({ value: "" })}>Uložiť rozloženie ako…</button>
      {edit && <form onSubmit={event => {
        event.preventDefault();
        if (invalid) return;
        setPresets(current => edit.original ? renamePreset(current, edit.original, edit.value) : savePreset(current, edit.value, layout));
        close();
      }}>
        <input aria-label="Názov rozloženia" placeholder="Názov rozloženia" maxLength={80} autoFocus
          value={edit.value} onChange={event => setEdit({ ...edit, value: event.target.value })}/>
        <button disabled={invalid}>Uložiť</button><button type="button" onClick={() => setEdit(null)}>Zrušiť</button>
      </form>}
      {presets.map(preset => <div key={preset.name} className="dock-preset">
        <button className="dock-preset-name" onClick={() => { setLayout(current => switchPreset(current, preset)); close(); }} title={preset.name}>{preset.name}</button>
        <button aria-label={`Premenovať ${preset.name}`} title="Premenovať" onClick={() => setEdit({ original: preset.name, value: preset.name })}>✎</button>
        <button aria-label={`Odstrániť ${preset.name}`} title="Odstrániť" onClick={() => {
          setPresets(current => current.filter(item => item.name !== preset.name));
          if (edit?.original === preset.name) setEdit(null);
        }}>×</button>
      </div>)}
      {!!layout.hidden.length && <><hr/><small>Skryté sekcie</small>{layout.hidden.map(id =>
        <button key={id} onClick={() => { setLayout(current => showSection(current, id)); close(); }}>Zobraziť · {sectionTitle(id)}</button>)}</>}
      <hr/><button onClick={() => {
        setLayout(current => ensureSections(defaultLayout(), knownSections(current)));
        close();
      }}>Obnoviť pôvodné rozloženie</button>
    </div>
  </details>;
}

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { createPortal } from "react-dom";
import Splitter from "./Splitter";
import { readStored, writeStored } from "../storage";

export type PanelId = "folders" | "calendar" | "properties" | "catalog" | "ai";
export type DockSideName = "left" | "right";
export type DockGroup = { id: string; panels: PanelId[]; active: PanelId; width: number; collapsed?: boolean };
export type DockLayout = { left: DockGroup[]; right: DockGroup[] };
type DropMode = "tab" | "before" | "after";

export const PANELS: { id: PanelId; title: string; nav?: boolean }[] = [
  { id: "folders", title: "Priečinky", nav: true },
  { id: "calendar", title: "Kalendár", nav: true },
  { id: "properties", title: "Vlastnosti" },
  { id: "catalog", title: "Katalóg" },
  { id: "ai", title: "AI" },
];
const panelTitle = (id: PanelId) => PANELS.find(p => p.id === id)?.title ?? id;
const isNav = (id: PanelId) => !!PANELS.find(p => p.id === id)?.nav;
const DRAG_TYPE = "application/x-ddreams-panel";
const STRIP = 28;
const uid = () => `g${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;

export const defaultLayout = (): DockLayout => ({
  left: [{ id: "g-nav", panels: ["folders", "calendar"], active: "folders", width: 260 }],
  right: [{ id: "g-props", panels: ["properties", "ai"], active: "properties", width: 320 }, { id: "g-catalog", panels: ["catalog"], active: "catalog", width: 280 }],
});

/** Repairs a stored layout: every panel appears exactly once, groups are non-empty and active tabs exist. */
function normalize(value: unknown): DockLayout {
  const fallback = defaultLayout();
  if (!value || typeof value !== "object") return fallback;
  const seen = new Set<PanelId>();
  const side = (raw: unknown): DockGroup[] => (Array.isArray(raw) ? raw : []).map((g: Partial<DockGroup>) => {
    const panels = (Array.isArray(g?.panels) ? g.panels : []).filter((p): p is PanelId => PANELS.some(x => x.id === p) && !seen.has(p) && !!seen.add(p));
    const width = Number(g?.width);
    return { id: typeof g?.id === "string" ? g.id : uid(), panels, active: panels.includes(g?.active as PanelId) ? g!.active as PanelId : panels[0], width: width >= 150 && width <= 1200 ? width : 280, collapsed: !!g?.collapsed };
  }).filter(g => g.panels.length);
  const layout = { left: side((value as DockLayout).left), right: side((value as DockLayout).right) };
  for (const panel of PANELS) if (!seen.has(panel.id)) {
    const home = fallback.left.some(g => g.panels.includes(panel.id)) ? "left" : "right";
    layout[home].push({ id: uid(), panels: [panel.id], active: panel.id, width: 280 });
  }
  return layout;
}

export function useDockLayout(): [DockLayout, Dispatch<SetStateAction<DockLayout>>] {
  const [layout, setLayout] = useState<DockLayout>(() => normalize(readStored<unknown>("ddreams.dockLayout.v1", null)));
  useEffect(() => { writeStored("ddreams.dockLayout.v1", layout); }, [layout]);
  return [layout, setLayout];
}

/** One stable DOM node per panel. Panels render into them through portals, so moving a panel never remounts it. */
export function useDockHosts(): Record<PanelId, HTMLDivElement> {
  return useMemo(() => Object.fromEntries(PANELS.map(p => { const el = document.createElement("div"); el.className = `dock-host dock-host-${p.id}`; return [p.id, el]; })) as Record<PanelId, HTMLDivElement>, []);
}

function without(layout: DockLayout, panel: PanelId): DockLayout {
  const strip = (groups: DockGroup[]) => groups.map(g => g.panels.includes(panel) ? { ...g, panels: g.panels.filter(p => p !== panel), active: g.active === panel ? g.panels.filter(p => p !== panel)[0] : g.active } : g).filter(g => g.panels.length);
  return { left: strip(layout.left), right: strip(layout.right) };
}
function sourceWidth(layout: DockLayout, panel: PanelId) { return [...layout.left, ...layout.right].find(g => g.panels.includes(panel))?.width ?? 280; }

/** Moves a panel into another group as a tab, or into a new column next to a group. */
export function movePanel(layout: DockLayout, panel: PanelId, targetGroup: string, mode: DropMode): DockLayout {
  const current = [...layout.left, ...layout.right].find(g => g.panels.includes(panel));
  if (current?.id === targetGroup && (mode === "tab" || current.panels.length === 1)) return layout;
  const width = sourceWidth(layout, panel), next = without(layout, panel);
  for (const side of ["left", "right"] as const) {
    const index = next[side].findIndex(g => g.id === targetGroup);
    if (index < 0) continue;
    const groups = [...next[side]];
    if (mode === "tab") groups[index] = { ...groups[index], panels: [...groups[index].panels, panel], active: panel, collapsed: false };
    else groups.splice(mode === "before" ? index : index + 1, 0, { id: uid(), panels: [panel], active: panel, width });
    return { ...next, [side]: groups };
  }
  return layout;
}
/** Moves a panel into a new column at the outer edge of a side. */
export function movePanelToSide(layout: DockLayout, panel: PanelId, side: DockSideName): DockLayout {
  const width = sourceWidth(layout, panel), next = without(layout, panel);
  const group: DockGroup = { id: uid(), panels: [panel], active: panel, width };
  return { ...next, [side]: side === "left" ? [group, ...next.left] : [...next.right, group] };
}

function PanelHost({ node, hidden }: { node: HTMLDivElement; hidden: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { ref.current?.appendChild(node); }, [node]);
  return <div ref={ref} className="dock-slot" style={hidden ? { display: "none" } : undefined} />;
}

type SideProps = { side: DockSideName; layout: DockLayout; setLayout: Dispatch<SetStateAction<DockLayout>>; hosts: Record<PanelId, HTMLDivElement>; hideNav: boolean; dragging: PanelId | null; setDragging: (panel: PanelId | null) => void };

export function DockSide({ side, layout, setLayout, hosts, hideNav, dragging, setDragging }: SideProps) {
  const groups = layout[side];
  const [menu, setMenu] = useState<{ x: number; y: number; panel: PanelId; group: string } | null>(null);
  const [zone, setZone] = useState<string | null>(null);
  useEffect(() => { if (!dragging) setZone(null); }, [dragging]);
  const patch = (id: string, change: Partial<DockGroup>) => setLayout(l => ({ ...l, [side]: l[side].map(g => g.id === id ? { ...g, ...change } : g) }));
  const dropProps = (key: string, apply: (panel: PanelId) => void) => ({
    onDragOver: (e: React.DragEvent) => { if (!e.dataTransfer.types.includes(DRAG_TYPE)) return; e.preventDefault(); e.dataTransfer.dropEffect = "move"; setZone(key); },
    onDragLeave: () => setZone(z => z === key ? null : z),
    onDrop: (e: React.DragEvent) => { const panel = e.dataTransfer.getData(DRAG_TYPE) as PanelId; if (!panel) return; e.preventDefault(); e.stopPropagation(); setZone(null); setDragging(null); apply(panel); },
  });
  const edge = dragging ? <div key="edge" className={`dock-edge-drop ${zone === `edge-${side}` ? "over" : ""}`} title={side === "left" ? "Nový stĺpec úplne vľavo" : "Nový stĺpec úplne vpravo"} {...dropProps(`edge-${side}`, panel => setLayout(l => movePanelToSide(l, panel, side)))} /> : null;
  const allGroups = [...layout.left.map(g => ({ g, s: "vľavo" })), ...layout.right.map(g => ({ g, s: "vpravo" }))];
  const body = groups.map(group => {
    const visible = group.panels.filter(p => !(hideNav && isNav(p)));
    if (!visible.length) return null;
    const active = visible.includes(group.active) ? group.active : visible[0];
    const splitter = <Splitter key={`${group.id}-s`} direction="x" invert={side === "right"} value={group.width} min={170} max={900} onChange={width => patch(group.id, { width })} onReset={() => patch(group.id, { width: 280 })} />;
    const column = group.collapsed
      ? <aside key={group.id} className="dock-group dock-collapsed" style={{ width: STRIP }} role="button" tabIndex={0} title="Rozbaliť panel" onClick={() => patch(group.id, { collapsed: false })} onKeyDown={e => { if (e.key === "Enter" || e.key === " ") patch(group.id, { collapsed: false }); }} {...dropProps(`${group.id}-tab`, panel => setLayout(l => movePanel(l, panel, group.id, "tab")))}>
          <span className="dock-expand">{side === "left" ? "›" : "‹"}</span>{visible.map(p => <span key={p} className="dock-vertical-title">{panelTitle(p)}</span>)}{group.panels.map(p => <PanelHost key={p} node={hosts[p]} hidden />)}
        </aside>
      : <aside key={group.id} className={`dock-group dock-${side}`} style={{ width: group.width }}>
          <div className={`dock-tabs ${zone === `${group.id}-tab` ? "over" : ""}`} {...dropProps(`${group.id}-tab`, panel => setLayout(l => movePanel(l, panel, group.id, "tab")))}>
            {visible.map(p => <button key={p} draggable className={p === active ? "active" : ""} title="Ťahaním presunieš panel inam, pravým klikom otvoríš ponuku" onClick={() => patch(group.id, { active: p })}
              onDragStart={e => { e.dataTransfer.setData(DRAG_TYPE, p); e.dataTransfer.effectAllowed = "move"; window.setTimeout(() => setDragging(p), 0); }} onDragEnd={() => setDragging(null)}
              onContextMenu={e => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, panel: p, group: group.id }); }}>{panelTitle(p)}</button>)}
            <span className="dock-tabs-spacer" />
            <button className="dock-collapse" title="Zbaliť panel" onClick={() => patch(group.id, { collapsed: true })}>{side === "left" ? "‹" : "›"}</button>
          </div>
          {group.panels.map(p => <PanelHost key={p} node={hosts[p]} hidden={p !== active} />)}
          {dragging && <div className="dock-drop-overlay">
            <div className={zone === `${group.id}-before` ? "over" : ""} {...dropProps(`${group.id}-before`, panel => setLayout(l => movePanel(l, panel, group.id, "before")))}><span>Vedľa vľavo</span></div>
            <div className={zone === `${group.id}-mid` ? "over" : ""} {...dropProps(`${group.id}-mid`, panel => setLayout(l => movePanel(l, panel, group.id, "tab")))}><span>Ako záložka</span></div>
            <div className={zone === `${group.id}-after` ? "over" : ""} {...dropProps(`${group.id}-after`, panel => setLayout(l => movePanel(l, panel, group.id, "after")))}><span>Vedľa vpravo</span></div>
          </div>}
        </aside>;
    return side === "left" ? [column, group.collapsed ? <div key={`${group.id}-gap`} className="dock-gap" /> : splitter] : [group.collapsed ? <div key={`${group.id}-gap`} className="dock-gap" /> : splitter, column];
  });
  return <>
    {side === "left" && edge}{body}{side === "right" && edge}
    {menu && createPortal(<div className="category-context-menu dock-menu" style={{ left: Math.min(menu.x, window.innerWidth - 270), top: Math.min(menu.y, window.innerHeight - 260) }} onMouseLeave={() => setMenu(null)}>
      <small>Panel {panelTitle(menu.panel)}</small>
      <button onClick={() => { setLayout(l => movePanelToSide(l, menu.panel, "left")); setMenu(null); }}>Samostatný stĺpec vľavo</button>
      <button onClick={() => { setLayout(l => movePanelToSide(l, menu.panel, "right")); setMenu(null); }}>Samostatný stĺpec vpravo</button>
      {allGroups.filter(({ g }) => g.id !== menu.group).map(({ g, s }) => <button key={g.id} onClick={() => { setLayout(l => movePanel(l, menu.panel, g.id, "tab")); setMenu(null); }}>Ako záložka k: {g.panels.map(panelTitle).join(", ")} ({s})</button>)}
      <hr className="menu-separator" />
      <button onClick={() => { patch(menu.group, { collapsed: true }); setMenu(null); }}>Zbaliť panel</button>
      <button onClick={() => { setLayout(defaultLayout()); setMenu(null); }}>Obnoviť pôvodné rozloženie</button>
    </div>, document.body)}
  </>;
}

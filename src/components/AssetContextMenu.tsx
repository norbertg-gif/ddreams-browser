import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { launchEditor, type Editor } from "../editors";
import type { Asset } from "../types";

type MenuAnchor = { x: number; y: number };

export function useAssetContextMenu() {
  const [anchor, setAnchor] = useState<MenuAnchor | null>(null);
  const close = useCallback(() => setAnchor(null), []);
  const open = (event: MouseEvent) => {
    event.preventDefault();
    setAnchor({ x: event.clientX, y: event.clientY });
  };
  return { anchor, open, close };
}

type Props = {
  anchor: MenuAnchor;
  onClose: () => void;
  asset: Asset;
  actionIds: () => number[];
  offline: boolean;
  isPhoto: boolean;
  editors: Editor[];
  readOnly: boolean;
  aiReady: boolean;
  aiReason: string;
  onOpen?: () => void;
  onCompare?: () => void;
  canCompare?: boolean;
  onOpenThumbnailInfo?: () => void;
  onSetValues: (ids: number[], changes: Record<string, unknown>) => void;
  onFileOperation: (ids: number[], mode: "move" | "copy" | "rename" | "delete" | "delete-permanent", asset?: Asset) => void;
  onQuarantine: (ids: number[]) => void;
  onFindSimilar: (asset: Asset) => void;
};

export default function AssetContextMenu({
  anchor, onClose, asset, actionIds, offline, isPhoto, editors, readOnly, aiReady, aiReason,
  onOpen, onCompare, canCompare, onOpenThumbnailInfo, onSetValues, onFileOperation, onQuarantine, onFindSimilar,
}: Props) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number }>();

  useLayoutEffect(() => {
    const clamp = () => {
      const rect = menuRef.current?.getBoundingClientRect();
      if (!rect) return;
      const pad = 8;
      // The portal lives outside the zoomed app shell, in viewport coordinates.
      setPosition({
        left: Math.max(pad, Math.min(anchor.x, window.innerWidth - rect.width - pad)),
        top: Math.max(pad, Math.min(anchor.y, window.innerHeight - rect.height - pad)),
      });
    };
    clamp();
    window.addEventListener("resize", clamp);
    return () => window.removeEventListener("resize", clamp);
  }, [anchor, editors.length]);

  useEffect(() => {
    const dismissOutside = (event: Event) => {
      if (!(event.target instanceof Node) || !menuRef.current?.contains(event.target)) onClose();
    };
    const dismissOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      onClose();
    };
    window.addEventListener("pointerdown", dismissOutside, true);
    window.addEventListener("scroll", dismissOutside, true);
    window.addEventListener("keydown", dismissOnEscape, true);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("pointerdown", dismissOutside, true);
      window.removeEventListener("scroll", dismissOutside, true);
      window.removeEventListener("keydown", dismissOnEscape, true);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  const canMarkPhoto = !asset.isDirectory && isPhoto && asset.quarantinedAt == null;
  const setValues = (changes: Record<string, unknown>) => {
    onSetValues(actionIds(), changes);
    onClose();
  };

  return createPortal(
    <div ref={menuRef} className="tile-context-menu"
      style={{ left: position?.left ?? anchor.x, top: position?.top ?? anchor.y, maxHeight: "calc(100vh - 16px)", overflowY: "auto" }}
      onPointerDown={event => event.stopPropagation()}
      onMouseDown={event => event.stopPropagation()}
      onClick={event => event.stopPropagation()}
      onDoubleClick={event => event.stopPropagation()}
      onContextMenu={event => { event.preventDefault(); event.stopPropagation(); }}
      onMouseLeave={onClose}>
      {onOpen && <button onClick={() => { onClose(); onOpen(); }}>Otvoriť</button>}
      {onCompare && <button disabled={!canCompare} onClick={() => { onClose(); onCompare(); }}>Porovnať vybrané (C)</button>}
      <button disabled={offline} onClick={() => { onClose(); void invoke("asset_action", { assetId: asset.id, action: "open" }); }}>Otvoriť v predvolenom programe</button>
      {!asset.isDirectory && editors.map((editor, index) =>
        <button key={editor.path} disabled={offline} title={editor.path} onClick={() => {
          onClose();
          void launchEditor(editor, actionIds()).catch(error => window.alert(String(error)));
        }}>Otvoriť v: {editor.name}{index === 0 ? " (predvolený)" : ""}</button>)}
      <button disabled={offline} onClick={() => { onClose(); void invoke("asset_action", { assetId: asset.id, action: "reveal" }); }}>Ukázať v Prieskumníkovi</button>
      <button onClick={async () => {
        onClose();
        const paths = await invoke<string[]>("get_asset_paths", { ids: actionIds() });
        await navigator.clipboard.writeText(paths.join("\n"));
      }}>Kopírovať cestu</button>
      {onOpenThumbnailInfo && <button onClick={() => { onClose(); onOpenThumbnailInfo(); }}>Údaje pod náhľadmi…</button>}
      <hr className="menu-separator" />
      <div className={readOnly ? "write-menu-group readonly" : "write-menu-group"}>
        <div className="write-menu-row"><span>Hodnotenie</span>
          {[1, 2, 3, 4, 5].map(value => <button key={value} title={"Ctrl+" + value} disabled={readOnly} onClick={() => setValues({ rating: value })}>★{value}</button>)}
          <button title="Ctrl+0" disabled={readOnly} onClick={() => setValues({ rating: null })}>×</button>
        </div>
        <div className="write-menu-row"><span>Farebný štítok</span>
          {[["Red", "Červená"], ["Yellow", "Žltá"], ["Green", "Zelená"], ["Blue", "Modrá"], ["Purple", "Fialová"], ["Gray", "Na vyradenie"]].map(([value, label], index) =>
            <button key={value} title={label + " (Alt+" + (index + 1) + ")"} disabled={readOnly} onClick={() => setValues({ label: value })}>
              <span className="write-menu-dot" style={{ backgroundColor: value.toLowerCase() === "gray" ? "#8a8f98" : value.toLowerCase() }} />
            </button>)}
          <button title="Alt+0" disabled={readOnly} onClick={() => setValues({ label: null })}>×</button>
        </div>
        <button title="\\" disabled={readOnly} onClick={() => setValues({ tagged: "toggle" })}>Označiť / Zrušiť označenie (\)</button>
        {canMarkPhoto && <button title="U" disabled={readOnly} onClick={() => setValues({ editPending: "toggle" })}>Na úpravu (U)</button>}
      </div>
      <hr className="menu-separator" />
      <button disabled={readOnly || offline} onClick={() => { onClose(); onFileOperation(actionIds(), "move"); }}>Presunúť do…</button>
      {canMarkPhoto && <button disabled={readOnly || offline} onClick={() => { onClose(); onQuarantine(actionIds()); }}>Do karantény</button>}
      <button disabled={readOnly || offline} onClick={() => { onClose(); onFileOperation(actionIds(), "copy"); }}>Kopírovať do…</button>
      <button disabled={readOnly || offline} onClick={() => { onClose(); onFileOperation([asset.id], "rename", asset); }}>Premenovať (F2)</button>
      <hr className="menu-separator" />
      {asset.id > 0 && !asset.isDirectory && <button disabled={!aiReady} title={!aiReady ? aiReason : ""} onClick={() => { onClose(); onFindSimilar(asset); }}>Nájsť podobné</button>}
      <hr className="menu-separator" />
      <hr className="menu-separator" />
      <button disabled={readOnly || (asset.isDirectory && (!asset.path || offline))} onClick={() => { onClose(); onFileOperation(asset.isDirectory ? [asset.id] : actionIds(), "delete", asset); }}>Odstrániť do Koša… (Del)</button>
      <button className="menu-danger" disabled={readOnly || asset.isDirectory} onClick={() => { onClose(); onFileOperation(actionIds(), "delete-permanent", asset); }}>Zmazať natrvalo… (Shift+Del)</button>
    </div>, document.body,
  );
}

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Asset } from "../types";
import { thumbUrl } from "./AssetGrid";

type Page = { items: Asset[]; total: number };
type Props = {
  /** Same filter the grid uses, without offset/limit. */
  filter: Record<string, unknown>;
  index: number;
  total: number;
  epoch: number;
  onPick: (asset: Asset, index: number) => void;
};

const HALF = 40;

/** Strip of neighbouring photos under the viewer; click jumps to a photo. */
export default function Filmstrip({ filter, index, total, epoch, onPick }: Props) {
  const [items, setItems] = useState<{ start: number; assets: Asset[] }>({ start: 0, assets: [] });
  const filterKey = JSON.stringify(filter);
  const current = useRef<HTMLButtonElement | null>(null);
  const loaded = useRef({ key: "", start: 0, end: 0 });
  useEffect(() => {
    const have = loaded.current;
    // Refetch only when the filter changed or the current photo gets close to the loaded window's edge.
    if (have.key === filterKey && index - have.start >= Math.min(10, index) && have.end - index > Math.min(10, total - 1 - index)) return;
    const start = Math.max(0, index - HALF);
    let active = true;
    void invoke<Page>("get_assets", { filter: { ...filter, offset: start, limit: HALF * 2 + 1 } }).then(page => {
      if (!active) return;
      loaded.current = { key: filterKey, start, end: start + page.items.length };
      setItems({ start, assets: page.items });
    }).catch(() => {});
    return () => { active = false; };
  }, [filterKey, index, total]);
  useEffect(() => { current.current?.scrollIntoView({ block: "nearest", inline: "center" }); }, [index, items]);
  return <div className="filmstrip" onWheel={e => { e.currentTarget.scrollLeft += e.deltaY; }}>
    {items.assets.map((asset, i) => {
      if (asset.isDirectory) return null;
      const at = items.start + i;
      return <button key={asset.id} ref={at === index ? current : undefined} className={`filmstrip-item${at === index ? " active" : ""}`} title={asset.name} onClick={() => onPick(asset, at)}>
        {asset.path && !asset.offline ? <img src={thumbUrl(asset, 160, epoch)} loading="lazy" draggable={false} alt="" onError={e => { e.currentTarget.style.visibility = "hidden"; }} /> : <span>{asset.ext.toUpperCase()}</span>}
      </button>;
    })}
  </div>;
}

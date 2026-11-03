import { useEffect, useMemo, useState } from "react";
import { DEFAULT_THUMBNAIL_INFO, THUMBNAIL_FIELD_BY_ID, THUMBNAIL_FIELD_GROUPS, type ThumbnailFieldChoice, type ThumbnailInfoConfig } from "../thumbnailInfo";

export default function ThumbnailInfoDialog({ value, onSave, onClose }: { value: ThumbnailInfoConfig; onSave: (value: ThumbnailInfoConfig) => void; onClose: () => void }) {
  const [draft, setDraft] = useState<ThumbnailFieldChoice[]>(value.fields.map(field => ({ ...field })));
  const [activeId, setActiveId] = useState<string | null>(null);
  const [availableId, setAvailableId] = useState<string | null>(null);
  const chosen = useMemo(() => new Set(draft.map(field => field.id)), [draft]);
  useEffect(() => { const key = (event: KeyboardEvent) => { if (event.key === "Escape") onClose(); if (event.key === "Enter" && !(event.target instanceof HTMLButtonElement)) onSave({ fields: draft }); }; window.addEventListener("keydown", key); return () => window.removeEventListener("keydown", key); }, [draft, onClose, onSave]);
  const add = (id = availableId) => { if (!id || chosen.has(id)) return; setDraft(fields => [...fields, { id, lines: 1 }]); setActiveId(id); };
  const remove = () => { if (!activeId) return; setDraft(fields => fields.filter(field => field.id !== activeId)); setActiveId(null); };
  const move = (delta: number) => { if (!activeId) return; setDraft(fields => { const index = fields.findIndex(field => field.id === activeId), next = index + delta; if (index < 0 || next < 0 || next >= fields.length) return fields; const copy = [...fields]; [copy[index], copy[next]] = [copy[next], copy[index]]; return copy; }); };
  return <div className="thumbnail-info-shade" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="thumbnail-info-dialog" role="dialog" aria-modal="true" aria-labelledby="thumbnail-info-title">
      <header><b id="thumbnail-info-title">Údaje pod náhľadmi</b><button onClick={onClose} aria-label="Zavrieť">×</button></header>
      <div className="thumbnail-info-columns">
        <div className="thumbnail-fields-tree"><b>Dostupné údaje</b>{THUMBNAIL_FIELD_GROUPS.map(group => <section key={group.name}><strong>{group.name}</strong>{group.fields.map(field => <label key={field.id} className={availableId === field.id ? "active" : ""} onClick={() => setAvailableId(field.id)}><input type="checkbox" checked={chosen.has(field.id)} onChange={() => chosen.has(field.id) ? (setDraft(items => items.filter(item => item.id !== field.id)), activeId === field.id && setActiveId(null)) : add(field.id)} />{field.label}</label>)}</section>)}</div>
        <div className="thumbnail-info-actions"><button disabled={!availableId || chosen.has(availableId)} onClick={() => add()}>Pridať →</button><button disabled={!activeId} onClick={remove}>← Odobrať</button><button disabled={!activeId || draft.findIndex(field => field.id === activeId) <= 0} onClick={() => move(-1)}>Hore</button><button disabled={!activeId || draft.findIndex(field => field.id === activeId) < 0 || draft.findIndex(field => field.id === activeId) >= draft.length - 1} onClick={() => move(1)}>Dole</button><button onClick={() => { setDraft(DEFAULT_THUMBNAIL_INFO.fields.map(field => ({ ...field }))); setActiveId(null); }}>Predvolené</button></div>
        <div className="thumbnail-fields-chosen"><b>Vybrané údaje</b><div className="thumbnail-chosen-head"><span>Pole</span><span>Riadky</span></div>{draft.map(field => <div key={field.id} className={`thumbnail-chosen-row ${activeId === field.id ? "active" : ""}`} onClick={() => setActiveId(field.id)}><span>{THUMBNAIL_FIELD_BY_ID.get(field.id)?.label ?? field.id}</span><select aria-label={`Počet riadkov: ${THUMBNAIL_FIELD_BY_ID.get(field.id)?.label ?? field.id}`} value={field.lines} onClick={event => event.stopPropagation()} onChange={event => { setActiveId(field.id); const lines = Number(event.target.value) as 1 | 2 | 3; setDraft(items => items.map(item => item.id === field.id ? { ...item, lines } : item)); }}><option value={1}>1</option><option value={2}>2</option><option value={3}>3</option></select></div>)}</div>
      </div>
      <footer><button onClick={onClose}>Zrušiť</button><button className="primary-action" onClick={() => onSave({ fields: draft })}>OK</button></footer>
    </section>
  </div>;
}

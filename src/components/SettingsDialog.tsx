import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useFolderRules, type FolderRuleAction } from "../folderRules";

type Props = {
  onClose: () => void;
  onRule: FolderRuleAction;
  showOtherFiles: boolean;
  onShowOtherFiles: (value: boolean) => void;
  aiEnabled: boolean;
  aiAvailable: boolean;
  aiStatusText: string;
  onToggleAi: () => void;
};

/** Application settings. Sections grow as features arrive. */
export default function SettingsDialog({ onClose, onRule, showOtherFiles, onShowOtherFiles, aiEnabled, aiAvailable, aiStatusText, onToggleAi }: Props) {
  const rules = useFolderRules();
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [onClose]);
  const add = async (mode: "include" | "exclude") => {
    const value = path.trim();
    if (!value) return;
    const exists = await invoke<boolean>("folder_exists", { path: value }).catch(() => false);
    if (!exists) { setError("Priečinok neexistuje alebo nie je dostupný."); return; }
    setError(""); setPath(""); onRule(value, mode);
  };
  return <div className="modal-backdrop" onPointerDown={e => { if (!box.current?.contains(e.target as Node)) onClose(); }}>
    <div className="settings-dialog" ref={box} role="dialog" aria-label="Nastavenia">
      <div className="settings-head"><b>Nastavenia</b><button onClick={onClose} title="Zavrieť (Esc)">✕</button></div>
      <section className="settings-section">
        <h3>Katalóg</h3>
        <p className="settings-help">Zahrnuté priečinky: nové fotky a videá sa pridajú do katalógu, keď priečinok otvoríš. Vylúčené priečinky sa dajú prehliadať, ale nie sú v katalógu. Pravidlá nastavíš aj pravým klikom na priečinok v strome.</p>
        {rules.length === 0 ? <small className="settings-empty">Zatiaľ žiadne pravidlá — katalóg sa správa ako doteraz.</small> :
          <div className="settings-rules">{rules.map(rule => <div key={rule.path} className={`settings-rule ${rule.mode}`}>
            <span className="rule-mark">{rule.mode === "include" ? "●" : "⊘"}</span><span className="rule-path" title={rule.path}>{rule.path}</span>
            <span className="rule-mode">{rule.mode === "include" ? "Zahrnutý" : "Vylúčený"}</span>
            <button onClick={() => onRule(rule.path, null)} title="Zrušiť pravidlo">Zrušiť</button>
          </div>)}</div>}
        <div className="settings-add">
          <input value={path} onChange={e => setPath(e.target.value)} placeholder="Cesta k priečinku, napr. F:\Fotky\Panasonic" onKeyDown={e => { if (e.key === "Enter") void add("include"); }} />
          <button onClick={() => void add("include")}>Zahrnúť</button><button onClick={() => void add("exclude")}>Vylúčiť</button>
        </div>
        {error && <small className="settings-error">{error}</small>}
      </section>
      <section className="settings-section">
        <h3>Zobrazenie</h3>
        <label className="settings-check"><input type="checkbox" checked={showOtherFiles} onChange={e => onShowOtherFiles(e.target.checked)} /> Zobrazovať aj iné súbory než fotky a videá</label>
        <p className="settings-help">Inak sú skryté (zip, log, pdf…). Dočasne ich ukážeš vo Filtroch voľbou „Typ: všetky súbory“.</p>
      </section>
      <section className="settings-section">
        <h3>AI</h3>
        {aiAvailable ? <>
          <label className="settings-check"><input type="checkbox" checked={aiEnabled} onChange={onToggleAi} /> Zapnúť AI (hľadanie obsahom, návrhy, farby, duplicity)</label>
          <p className="settings-help">{aiStatusText}. Vypnutím sa uvoľní pamäť grafickej karty.</p>
        </> : <p className="settings-help">AI modul nie je nainštalovaný.</p>}
      </section>
    </div>
  </div>;
}

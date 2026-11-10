import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useFolderRules, type FolderRuleAction } from "../folderRules";
import { saveEditors, useEditors, type Editor } from "../editors";

type Props = {
  onClose: () => void;
  onRule: FolderRuleAction;
  showOtherFiles: boolean;
  onShowOtherFiles: (value: boolean) => void;
  aiEnabled: boolean;
  aiAvailable: boolean;
  aiStatusText: string;
  onToggleAi: () => void;
  uiScale: number;
  onUiScale: (value: number) => void;
};

type BackupStatus = { lastBackupAt: number | null; lastBackupFile: string | null; backupCount: number; backupsDir: string; intervalDays: number; snoozedUntil: number | null; due: boolean };

/** Application settings. Sections grow as features arrive. */
export default function SettingsDialog({ onClose, onRule, showOtherFiles, onShowOtherFiles, aiEnabled, aiAvailable, aiStatusText, onToggleAi, uiScale, onUiScale }: Props) {
  const rules = useFolderRules();
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  const [backupDone, setBackupDone] = useState(false);
  const [backupError, setBackupError] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const editors = useEditors();
  const [editorError, setEditorError] = useState("");
  const [picking, setPicking] = useState(false);
  const setEditor = (index: number, patch: Partial<Editor>) => saveEditors(editors.map((e, i) => i === index ? { ...e, ...patch } : e));
  const moveEditor = (index: number, to: number) => { if (to < 0 || to >= editors.length) return; const next = [...editors]; const [item] = next.splice(index, 1); next.splice(to, 0, item); saveEditors(next); };
  const addEditor = async () => {
    setEditorError(""); setPicking(true);
    try {
      const path = await invoke<string | null>("pick_editor_program");
      if (!path) return;
      if (editors.some(e => e.path.toLowerCase() === path.toLowerCase())) { setEditorError("Tento program už v zozname je."); return; }
      const name = (path.split(/[\\/]/).pop() ?? path).replace(/\.exe$/i, "");
      saveEditors([...editors, { name, path, multi: true }]);
    } catch (e) { setEditorError(String(e)); } finally { setPicking(false); }
  };
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [onClose]);
  useEffect(() => { void invoke<BackupStatus>("get_backup_status").then(setBackup).catch(e => setBackupError(String(e))); }, []);
  const backupNow = async () => {
    setBackupBusy(true); setBackupDone(false); setBackupError("");
    try { await invoke<string>("backup_catalog_now"); setBackup(await invoke<BackupStatus>("get_backup_status")); setBackupDone(true); window.setTimeout(() => setBackupDone(false), 1800); }
    catch (e) { setBackupError(String(e)); } finally { setBackupBusy(false); }
  };
  const changeInterval = async (days: number) => {
    setBackupError("");
    try { await invoke("set_backup_interval_days", { days }); setBackup(await invoke<BackupStatus>("get_backup_status")); }
    catch (e) { setBackupError(String(e)); }
  };
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
        <div className="backup-settings">
          <b>Záloha katalógu</b>
          <small>Posledná záloha: {backup?.lastBackupAt ? new Date(backup.lastBackupAt * 1000).toLocaleString("sk-SK") : "zatiaľ žiadna"} · {backup?.backupCount ?? 0} záloh</small>
          <small className="backup-path" title={backup?.backupsDir}>{backup?.backupsDir ?? "Načítavam…"}</small>
          <button disabled={backupBusy} onClick={() => void backupNow()}>{backupBusy ? "Zálohujem…" : backupDone ? "Záloha hotová" : "Zálohovať teraz"}</button>
          <label>Pripomínať zálohu: <select value={backup?.intervalDays ?? 30} onChange={e => void changeInterval(Number(e.target.value))}><option value={30}>každý mesiac</option><option value={14}>každé 2 týždne</option><option value={7}>každý týždeň</option><option value={0}>nikdy</option></select></label>
          {backupError && <small className="settings-error">{backupError}</small>}
        </div>
      </section>
      <section className="settings-section">
        <h3>Zobrazenie</h3>
        <label className="settings-check"><input type="checkbox" checked={showOtherFiles} onChange={e => onShowOtherFiles(e.target.checked)} /> Zobrazovať aj iné súbory než fotky a videá</label>
        <label className="settings-check ui-scale-setting">Veľkosť písma rozhrania: <select value={uiScale} onChange={e => onUiScale(Number(e.target.value))}><option value={1}>Malé (predvolené)</option><option value={1.1}>Stredné</option><option value={1.2}>Veľké</option></select></label>
        <p className="settings-help">Inak sú skryté (zip, log, pdf…). Dočasne ich ukážeš vo Filtroch voľbou „Typ: všetky súbory“.</p>
      </section>
      <section className="settings-section">
        <h3>Externé editory</h3>
        <p className="settings-help">Programy, v ktorých otvoríš fotky z kontextového menu. Prvý v zozname je predvolený.</p>
        {editors.length === 0 ? <small className="settings-empty">Zatiaľ žiadny editor.</small> :
          <div className="settings-editors">{editors.map((editor, index) => <div key={editor.path} className="settings-editor">
            <input className="editor-name" value={editor.name} onChange={e => setEditor(index, { name: e.target.value })} title="Názov v menu" />
            <span className="rule-path" title={editor.path}>{editor.path}</span>
            <label title="Program otvorí všetky vybrané súbory naraz"><input type="checkbox" checked={editor.multi} onChange={e => setEditor(index, { multi: e.target.checked })} /> viac súborov</label>
            <button disabled={index === 0} onClick={() => moveEditor(index, index - 1)} title="Vyššie">↑</button>
            <button disabled={index === editors.length - 1} onClick={() => moveEditor(index, index + 1)} title="Nižšie">↓</button>
            <button disabled={index === 0} onClick={() => moveEditor(index, 0)} title="Nastaviť ako predvolený">Predvolený</button>
            <button onClick={() => saveEditors(editors.filter((_, i) => i !== index))} title="Odstrániť zo zoznamu">Odstrániť</button>
          </div>)}</div>}
        <div className="settings-add"><button disabled={picking} onClick={() => void addEditor()}>{picking ? "Vyberáš program…" : "Pridať program…"}</button></div>
        {editorError && <small className="settings-error">{editorError}</small>}
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

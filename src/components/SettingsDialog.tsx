import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import FileOperationDialog from "./FileOperationDialog";
import { useFolderRules, type FolderRuleAction } from "../folderRules";
import { saveEditors, useEditors, editorsSuggested, type Editor } from "../editors";
import type { QuarantineSettings } from "./QuarantineReview";

type Props = {
  onClose: () => void;
  incomingFolder: string;
  readOnly: boolean;
  onIncomingFolderChange: (path:string) => Promise<void>;
  quarantineSettings: QuarantineSettings | null;
  onQuarantineSettingsChange: (settings: QuarantineSettings) => Promise<void>;
  onRule: FolderRuleAction;
  showOtherFiles: boolean;
  onShowOtherFiles: (value: boolean) => void;
  aiEnabled: boolean;
  aiLoadModelAtStart: boolean;
  onAiLoadModelAtStart: (value: boolean) => void;
  aiAvailable: boolean;
  aiStatusText: string;
  onToggleAi: () => void;
  uiScale: number;
  onUiScale: (value: number) => void;
  onRemoveSource:(path:string)=>void|Promise<void>;
};

type MissingSource={path:string;assets:number;offline:boolean;missing:boolean};
type BackupStatus = { lastBackupAt: number | null; lastBackupFile: string | null; backupCount: number; backupsDir: string; intervalDays: number; snoozedUntil: number | null; due: boolean };

/** Application settings. Sections grow as features arrive. */
export default function SettingsDialog({ onClose, incomingFolder, onIncomingFolderChange, quarantineSettings, onQuarantineSettingsChange, readOnly, onRule, showOtherFiles, onShowOtherFiles, aiEnabled, aiLoadModelAtStart, onAiLoadModelAtStart, aiAvailable, aiStatusText, onToggleAi, uiScale, onUiScale,onRemoveSource }: Props) {
  const rules = useFolderRules();
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  const [incomingPicker, setIncomingPicker] = useState(false);
  const [incomingError, setIncomingError] = useState("");
  const [incomingBusy, setIncomingBusy] = useState(false);
  const [quarantine, setQuarantine] = useState(quarantineSettings);
  useEffect(() => setQuarantine(quarantineSettings), [quarantineSettings]);
  const saveQuarantine = async () => {
    if (!quarantine) return;
    setIncomingError("");
    setIncomingBusy(true);
    try { await onQuarantineSettingsChange(quarantine); }
    catch (error) { setIncomingError(String(error)); }
    finally { setIncomingBusy(false); }
  };
  const saveIncoming = async (path: string) => {
    setIncomingPicker(false);
    setIncomingError("");
    setIncomingBusy(true);
    try { await onIncomingFolderChange(path); }
    catch (error) { setIncomingError(String(error)); }
    finally { setIncomingBusy(false); }
  };
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const [backupBusy, setBackupBusy] = useState(false);
  const [backupDone, setBackupDone] = useState(false);
  const [backupError, setBackupError] = useState("");
  const [missingSources,setMissingSources]=useState<MissingSource[]>([]);
  const [showEmptySources,setShowEmptySources]=useState(false);
  const [sourcesLoading,setSourcesLoading]=useState(false);
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
    const reloadSources=()=>{setSourcesLoading(true);void invoke<MissingSource[]>("get_catalog_missing_sources").then(setMissingSources).catch(()=>setMissingSources([])).finally(()=>setSourcesLoading(false))};window.addEventListener("ddreams-catalog-changed",reloadSources);return()=>window.removeEventListener("ddreams-catalog-changed",reloadSources);
  }, []);
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); if (incomingPicker) setIncomingPicker(false); else onClose(); } };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [onClose, incomingPicker]);
  useEffect(() => { void invoke<BackupStatus>("get_backup_status").then(setBackup).catch(e => setBackupError(String(e)));setSourcesLoading(true);void invoke<MissingSource[]>("get_catalog_missing_sources").then(setMissingSources).catch(()=>setMissingSources([])).finally(()=>setSourcesLoading(false)); }, []);
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
  return <><div className="modal-backdrop" onPointerDown={e => { if (!box.current?.contains(e.target as Node)) onClose(); }}>
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
      <section className="settings-section"><div className="settings-section-heading"><h3>Offline a chýbajúce zdroje</h3><button disabled={sourcesLoading} onClick={()=>window.dispatchEvent(new Event("ddreams-catalog-changed"))}>{sourcesLoading?"Obnovujem…":"Obnoviť zoznam"}</button></div>{(()=>{const merged=new Map<string,MissingSource>();for(const source of missingSources){const key=source.path.toLowerCase();const old=merged.get(key);if(old)old.assets+=source.assets;else merged.set(key,{...source});}const all=[...merged.values()].sort((x,y)=>y.assets-x.assets||x.path.localeCompare(y.path,"sk"));const withAssets=all.filter(x=>x.assets>0);const empty=all.length-withAssets.length;const shown=showEmptySources?all:withAssets;const kind=(x:MissingSource)=>x.offline?"Disk nie je pripojený":/^[A-Za-z]:\\|^\\\\/.test(x.path)?"Priečinok neexistuje":"Zariadenie nie je pripojené";return <>{withAssets.length>0&&<small className="settings-empty">Spolu {withAssets.reduce((n,x)=>n+x.assets,0).toLocaleString("sk-SK")} záznamov o súboroch v {withAssets.length} zdrojoch, ktoré teraz nie sú dostupné.</small>}{shown.map(source=><div className="settings-missing-source" key={source.path}><span><b>{kind(source)}</b><span title={source.path}>{source.path}</span><small>{source.assets.toLocaleString("sk-SK")} záznamov o súboroch</small></span><button onClick={()=>void onRemoveSource(source.path)}>Odstrániť z katalógu…</button></div>)}{!all.length&&<small className="settings-empty">Katalóg neobsahuje žiadne offline ani chýbajúce zdroje.</small>}{empty>0&&<label className="settings-empty"><input type="checkbox" checked={showEmptySources} onChange={e=>setShowEmptySources(e.target.checked)}/> Zobraziť aj {empty} prázdnych záznamov priečinkov (bez fotiek)</label>}</>})()}</section>
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
        <div className="settings-add">{editorsSuggested()&&<button onClick={() => saveEditors(editors)}>Uložiť zistený editor</button>}<button disabled={picking} onClick={() => void addEditor()}>{picking ? "Vyberáš program…" : "Pridať program…"}</button></div>
        {editorError && <small className="settings-error">{editorError}</small>}
      </section>
      <section className="settings-section">
        <h3>Import a karanténa</h3>
        <strong>Importovací (zdrojový) priečinok</strong>
        <p className="settings-help" title={incomingFolder}>{incomingFolder || "Priečinok zatiaľ nie je vybraný."}</p>
        <div className="settings-add">
          <button disabled={readOnly||incomingBusy} onClick={()=>setIncomingPicker(true)}>Vybrať priečinok…</button>
          {incomingFolder&&<button disabled={readOnly||incomingBusy} onClick={()=>void saveIncoming("")}>Zrušiť výber</button>}
        </div>
        {quarantine && <form className="quarantine-settings" onSubmit={event => { event.preventDefault(); void saveQuarantine(); }}>
          <fieldset disabled={readOnly || incomingBusy}>
            <label>Priečinok karantény
              <input value={quarantine.folder} placeholder={quarantine.quarantineFolder || "_Karanténa"}
                onChange={event => setQuarantine({ ...quarantine, folder: event.target.value })} />
            </label>
            <label>Presunúť do karantény po (dňoch)
              <input type="number" min="0" max="4294967295" required value={Number.isNaN(quarantine.moveAfterDays) ? "" : quarantine.moveAfterDays}
                onChange={event => setQuarantine({ ...quarantine, moveAfterDays: event.target.valueAsNumber })} />
            </label>
            <label>Zmazať z karantény po (dňoch)
              <input type="number" min="0" max="4294967295" required value={Number.isNaN(quarantine.deleteAfterDays) ? "" : quarantine.deleteAfterDays}
                onChange={event => setQuarantine({ ...quarantine, deleteAfterDays: event.target.valueAsNumber })} />
            </label>
            <label>Spravovať len fotky nasnímané od (dátum)
              <input type="date" required value={quarantine.manageFrom}
                onChange={event => setQuarantine({ ...quarantine, manageFrom: event.target.value })} />
            </label>
            <small>Prázdna cesta použije priečinok _Karanténa vedľa importu. Hodnota 0 vypne príslušnú kontrolu pri štarte.</small>
            <div><button type="submit">Uložiť nastavenia karantény</button></div>
          </fieldset>
        </form>}
        {incomingError&&<small className="settings-error">{incomingError}</small>}
      </section>
      <section className="settings-section">
        <h3>AI</h3>
        {aiAvailable ? <>
          <label className="settings-check"><input type="checkbox" checked={aiEnabled} onChange={onToggleAi} /> Zapnúť AI (hľadanie obsahom, návrhy, farby, duplicity)</label>
          <label className="settings-check"><input type="checkbox" checked={aiLoadModelAtStart} onChange={event => onAiLoadModelAtStart(event.target.checked)} /> Načítať AI model pri štarte</label>
          <p className="settings-help">Zmena sa prejaví pri ďalšom zapnutí AI.</p>
          <p className="settings-help">{aiStatusText}. Vypnutím sa uvoľní pamäť grafickej karty.</p>
        </> : <p className="settings-help">AI modul nie je nainštalovaný.</p>}
      </section>
    </div>
  </div>
  {incomingPicker&&<FileOperationDialog mode="select" initialPath={incomingFolder}
    onChoose={path=>void saveIncoming(path)} onClose={()=>setIncomingPicker(false)}/>}
  </>;
}

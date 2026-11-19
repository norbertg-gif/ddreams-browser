import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type FolderSuggestion = { folder: string; share: number };
type Suggestions = { eligible: boolean; folders: FolderSuggestion[] };
export type DeletionSuggestionChange = { changeId: number | null; summary: string };

type Props = {
  folder: string | null;
  ids: number[];
  aiReady: boolean;
  aiReason: string;
  readOnly: boolean;
  indexRevision: number;
  refreshToken: number;
  onMove: (folder: string) => void;
  onChooseFolder: () => void;
  onAnalyze: () => Promise<void>;
};

export default function IncomingTools({folder, ids, aiReady, aiReason, readOnly, indexRevision,
  refreshToken, onMove, onChooseFolder, onAnalyze}: Props) {
  const [result, setResult] = useState<(Suggestions & { ids: number[]; folder: string }) | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setResult(null);
    if (!folder || !ids.length) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void invoke<Suggestions>("suggest_incoming_folders", {assetIds:ids}).then(value => {
        if (active) setResult({...value, ids, folder});
      }).catch(error => {
        if (active) console.warn("Návrhy priečinkov", error);
      });
    }, 180);
    return () => { active = false; window.clearTimeout(timer); };
  }, [ids, folder, aiReady, indexRevision, refreshToken]);

  if (folder === null) return null;
  if (!folder) return <section className="incoming-tools">
    <small>V Nastaveniach vyberte priečinok s novými fotkami.</small>
  </section>;

  const current = result?.ids === ids && result.folder === folder ? result : null;
  const folders = current?.folders ?? [];
  return <section className="incoming-tools">
    {current?.eligible && (!aiReady || folders.length > 0) && <div className="incoming-folder-suggestions">
      <strong>Kam to patrí?</strong>
      {!aiReady && <small>{aiReason}</small>}
      {aiReady && folders.map(({folder, share}) => <div className="incoming-folder-row" key={folder}>
        <span title={folder}>{folder.split(/[\\/]/).filter(Boolean).slice(-2).join(" › ")}</span>
        <small title="Podiel váhy najpodobnejších záberov">{Math.round(share * 100)} %</small>
        <button disabled={readOnly} onClick={() => onMove(folder)}>Presunúť sem</button>
      </div>)}
      <button disabled={readOnly} onClick={onChooseFolder}>Iný priečinok…</button>
    </div>}
    <button disabled={!aiReady || readOnly || busy} title={!aiReady ? aiReason : undefined} onClick={() => {
      setBusy(true);
      void onAnalyze().finally(() => setBusy(false));
    }}>{busy ? "Analyzujú sa série…" : "Navrhnúť slabšie zábery na vymazanie"}</button>
  </section>;
}

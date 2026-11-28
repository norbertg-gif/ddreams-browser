import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type FolderSuggestion = import("../bindings/FolderSuggestion").FolderSuggestion;
type Suggestions = import("../bindings/FolderSuggestions").FolderSuggestions;
type Groups = { groups: number[][]; unindexed: number; omitted: number };
export type DeletionSuggestionChange = import("../bindings/DeletionSuggestionChange").DeletionSuggestionChange;

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
  onShowPhotos: (ids: number[], title: string, groups?: number[]) => void;
  onMoveGroup: (ids: number[], folder: string) => void;
};

export default function IncomingTools({
  folder,
  ids,
  aiReady,
  aiReason,
  readOnly,
  indexRevision,
  refreshToken,
  onMove,
  onChooseFolder,
  onAnalyze,
  onShowPhotos,
  onMoveGroup,
}: Props) {
  const [result, setResult] = useState<(Suggestions & { ids: number[]; folder: string }) | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setResult(null);
    if (!folder || !ids.length) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void invoke<Suggestions>("suggest_incoming_folders", { assetIds: ids })
        .then((value) => {
          if (active) setResult({ ...value, ids, folder });
        })
        .catch((error) => {
          if (active) console.warn("Návrhy priečinkov", error);
        });
    }, 180);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [ids, folder, aiReady, indexRevision, refreshToken]);

  if (folder === null) return null;
  if (!folder)
    return (
      <section className="incoming-tools">
        <small>V Nastaveniach vyberte priečinok s novými fotkami.</small>
      </section>
    );

  const current = result?.ids === ids && result.folder === folder ? result : null;
  const folders = current?.folders ?? [];
  return (
    <section className="incoming-tools">
      {current?.eligible && (!aiReady || folders.length > 0) && (
        <div className="incoming-folder-suggestions">
          <strong>Kam to patrí?</strong>
          {!aiReady && <small>{aiReason}</small>}
          {aiReady &&
            folders.map(({ folder, share }) => (
              <div className="incoming-folder-row" key={folder}>
                <span title={folder}>{folder.split(/[\\/]/).filter(Boolean).slice(-2).join(" › ")}</span>
                <small title="Podiel váhy najpodobnejších záberov">{Math.round(share * 100)} %</small>
                <button disabled={readOnly} onClick={() => onMove(folder)}>
                  Presunúť sem
                </button>
              </div>
            ))}
          <button disabled={readOnly} onClick={onChooseFolder}>
            Iný priečinok…
          </button>
        </div>
      )}
      <button
        disabled={!aiReady || readOnly || busy}
        title={!aiReady ? aiReason : undefined}
        onClick={() => {
          setBusy(true);
          void onAnalyze().finally(() => setBusy(false));
        }}
      >
        {busy ? "Analyzujú sa série…" : "Navrhnúť slabšie zábery na vymazanie"}
      </button>
      <IncomingGroups
        key={JSON.stringify([folder, refreshToken])}
        aiReady={aiReady}
        readOnly={readOnly}
        selectedIds={ids}
        onShowPhotos={onShowPhotos}
        onMoveGroup={onMoveGroup}
      />
    </section>
  );
}

function IncomingGroups({
  aiReady,
  readOnly,
  selectedIds,
  onShowPhotos,
  onMoveGroup,
}: {
  selectedIds: number[];
} & Pick<Props, "aiReady" | "readOnly" | "onShowPhotos" | "onMoveGroup">) {
  const [result, setResult] = useState<Groups | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [visibleCount, setVisibleCount] = useState(30);
  const [selected, setSelected] = useState<number | null>(null);

  const load = async () => {
    setBusy(true);
    setError("");
    setSelected(null);
    try {
      setResult(await invoke<Groups>("ai_incoming_groups"));
      setVisibleCount(30);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="incoming-group-tools">
      <button disabled={!aiReady || busy} onClick={() => void load()}>
        {busy ? "Hľadám skupiny…" : "Zoskupiť nové fotky podľa podobnosti"}
      </button>
      {error && <small role="status">{error}</small>}
      {result && (
        <>
          {!result.groups.length && <small>V priečinku nie sú zaindexované fotky.</small>}
          {result.unindexed > 0 && <small>Čaká na AI index: {result.unindexed}</small>}
          {result.omitted > 0 && (
            <small>Spracovaných je posledných 2 000 zaindexovaných položiek. Mimo výsledku: {result.omitted}.</small>
          )}
          {selected === null ? (
            <>
              {result.groups.slice(0, visibleCount).map((ids, i) => (
                <button
                  key={ids[0]}
                  onClick={() => {
                    setSelected(i);
                    onShowPhotos(ids, `Nové fotky · skupina ${i + 1}`);
                  }}
                >
                  Skupina {i + 1} · {ids.length} {ids.length === 1 ? "fotka" : ids.length < 5 ? "fotky" : "fotiek"}
                </button>
              ))}
              {visibleCount < result.groups.length && (
                <button onClick={() => setVisibleCount((n) => n + 30)}>Ďalšie skupiny</button>
              )}
            </>
          ) : (
            <>
              <button onClick={() => setSelected(null)}>Späť na skupiny</button>
              <IncomingGroup
                key={selected}
                ids={result.groups[selected]}
                number={selected + 1}
                readOnly={readOnly}
                selectedIds={selectedIds}
                onShowPhotos={onShowPhotos}
                onMoveGroup={onMoveGroup}
              />
            </>
          )}
        </>
      )}
    </div>
  );
}

function IncomingGroup({
  ids,
  number,
  readOnly,
  selectedIds,
  onShowPhotos,
  onMoveGroup,
}: {
  ids: number[];
  number: number;
  selectedIds: number[];
} & Pick<Props, "readOnly" | "onShowPhotos" | "onMoveGroup">) {
  const [folders, setFolders] = useState<FolderSuggestion[] | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void invoke<Suggestions>("suggest_incoming_folders", { assetIds: ids, includeExamples: true })
      .then((response) => {
        if (active) setFolders(response.folders);
      })
      .catch((cause) => {
        if (active) setError(String(cause));
      });
    return () => {
      active = false;
    };
  }, [ids]);
  const selected = new Set(selectedIds);
  const selectedNew = ids.filter((id) => selected.has(id));
  return (
    <>
      <button onClick={() => onShowPhotos(ids, `Nové fotky · skupina ${number}`)}>Zobraziť nové · {ids.length}</button>
      {error ? (
        <small role="status">{error}</small>
      ) : folders === null ? (
        <small>Hľadám cieľ a podobné fotky v katalógu…</small>
      ) : folders.length === 0 ? (
        <small>Pre túto skupinu nemáme návrh priečinka.</small>
      ) : (
        folders.map((item) => (
          <div className="incoming-group-destination" key={item.folder}>
            <strong title={item.folder}>{item.folder.split(/[\\/]/).filter(Boolean).slice(-2).join(" › ")}</strong>
            {!!item.exampleIds.length && (
              <button
                onClick={() =>
                  onShowPhotos([...ids, ...item.exampleIds], "#1 Nové · #2 Už v katalógu", [
                    ...ids.map(() => 1),
                    ...item.exampleIds.map(() => 2),
                  ])
                }
              >
                Porovnať s katalógom · {item.exampleIds.length}
              </button>
            )}
            <button disabled={readOnly} onClick={() => onMoveGroup(ids, item.folder)}>
              Presunúť celú skupinu sem
            </button>
            {selectedNew.length > 0 && selectedNew.length < ids.length && (
              <button disabled={readOnly} onClick={() => onMoveGroup(selectedNew, item.folder)}>
                Presunúť vybrané nové sem · {selectedNew.length}
              </button>
            )}
          </div>
        ))
      )}
    </>
  );
}

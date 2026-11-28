import { formatBytes } from "./AssetGrid";
import ThumbnailInfoDialog from "./ThumbnailInfoDialog";
import SettingsDialog from "./SettingsDialog";
import QuarantineReview from "./QuarantineReview";
import HelpDialog from "./HelpDialog";
import FileOperationDialog from "./FileOperationDialog";
import FileNameDialog from "./FileNameDialog";
import IptcWriteDialog, { IptcIssues } from "./IptcWriteDialog";
import { normalizeThumbnailInfo, type ThumbnailInfoConfig } from "../thumbnailInfo";
import type { Dispatch, SetStateAction } from "react";
import type { useHelpShortcut } from "../hooks/useKeyboardShortcuts";
import type { AppInfo, useNavigation } from "../hooks/useNavigation";
import type { useIncomingFolder, useQuarantineReview, useQuarantine } from "../hooks/useQuarantine";
import type { useFileOperations } from "../hooks/useFileOperations";
import type { useAiSettings, useAi } from "../hooks/useAi";
import type { useIptc } from "../hooks/useIptc";

type WindowDialogsProps = {
  slovakItems: (count: number) => string;
  appInfo: AppInfo | null;
  thumbnailInfo: ThumbnailInfoConfig;
  setThumbnailInfo: Dispatch<SetStateAction<ThumbnailInfoConfig>>;
  thumbnailInfoOpen: boolean;
  setThumbnailInfoOpen: Dispatch<SetStateAction<boolean>>;
  showOtherFiles: boolean;
  setShowOtherFiles: Dispatch<SetStateAction<boolean>>;
  uiScale: number;
  setUiScale: Dispatch<SetStateAction<number>>;
  settingsOpen: boolean;
  setSettingsOpen: Dispatch<SetStateAction<boolean>>;
  toast: string;
  help: Pick<ReturnType<typeof useHelpShortcut>, "helpOpen" | "setHelpOpen">;
  navigation: Pick<ReturnType<typeof useNavigation>, "view">;
  incoming: Pick<ReturnType<typeof useIncomingFolder>, "incomingFolder">;
  files: Pick<
    ReturnType<typeof useFileOperations>,
    | "deleteConfirm"
    | "setDeleteConfirm"
    | "excludeConfirm"
    | "setExcludeConfirm"
    | "removeCatalogConfirm"
    | "setRemoveCatalogConfirm"
    | "fileOp"
    | "setFileOp"
    | "nameOp"
    | "setNameOp"
    | "fileConflict"
    | "setFileConflict"
    | "moveWarning"
    | "setMoveWarning"
    | "folderRenameOp"
    | "setFolderRenameOp"
    | "folderDeleteConfirm"
    | "setFolderDeleteConfirm"
    | "operationProgress"
    | "runFileOperation"
    | "chooseOperationTarget"
    | "submitFolderRename"
    | "confirmFolderDelete"
    | "runDelete"
    | "createFolder"
    | "submitName"
    | "applyFolderRule"
    | "changeFolderRule"
    | "requestRemoveCatalog"
    | "removeCatalogSource"
  >;
  review: Pick<
    ReturnType<typeof useQuarantineReview>,
    | "quarantineSettings"
    | "saveQuarantineSettings"
    | "changeIncomingFolder"
    | "quarantineReview"
    | "setQuarantineReview"
    | "reviewInGrid"
    | "quarantineBusy"
    | "quarantineConfirm"
    | "setQuarantineConfirm"
    | "quarantineFailures"
    | "setQuarantineFailures"
  >;
  aiSettings: Pick<
    ReturnType<typeof useAiSettings>,
    "aiStatus" | "aiEnabled" | "aiLoadModelAtStart" | "setAiLoadModelAtStart"
  >;
  iptc: Pick<
    ReturnType<typeof useIptc>,
    "iptcIds" | "setIptcIds" | "iptcEditIssues" | "setIptcEditIssues" | "finishIptcWrite"
  >;
  quarantine: Pick<ReturnType<typeof useQuarantine>, "runQuarantine" | "inspectQuarantineReview">;
  ai: Pick<ReturnType<typeof useAi>, "aiChipText" | "toggleAi">;
};

export function WindowDialogs({
  slovakItems,
  appInfo,
  thumbnailInfo,
  setThumbnailInfo,
  thumbnailInfoOpen,
  setThumbnailInfoOpen,
  showOtherFiles,
  setShowOtherFiles,
  uiScale,
  setUiScale,
  settingsOpen,
  setSettingsOpen,
  toast,
  help,
  navigation,
  incoming,
  files,
  review,
  aiSettings,
  iptc,
  quarantine,
  ai,
}: WindowDialogsProps) {
  const { helpOpen, setHelpOpen } = help;
  const { view } = navigation;
  const { incomingFolder } = incoming;
  const {
    deleteConfirm,
    setDeleteConfirm,
    excludeConfirm,
    setExcludeConfirm,
    removeCatalogConfirm,
    setRemoveCatalogConfirm,
    fileOp,
    setFileOp,
    nameOp,
    setNameOp,
    fileConflict,
    setFileConflict,
    moveWarning,
    setMoveWarning,
    folderRenameOp,
    setFolderRenameOp,
    folderDeleteConfirm,
    setFolderDeleteConfirm,
    operationProgress,
    runFileOperation,
    chooseOperationTarget,
    submitFolderRename,
    confirmFolderDelete,
    runDelete,
    createFolder,
    submitName,
    applyFolderRule,
    changeFolderRule,
    requestRemoveCatalog,
    removeCatalogSource,
  } = files;
  const {
    quarantineSettings,
    saveQuarantineSettings,
    changeIncomingFolder,
    quarantineReview,
    setQuarantineReview,
    reviewInGrid,
    quarantineBusy,
    quarantineConfirm,
    setQuarantineConfirm,
    quarantineFailures,
    setQuarantineFailures,
  } = review;
  const { aiStatus, aiEnabled, aiLoadModelAtStart, setAiLoadModelAtStart } = aiSettings;
  const { iptcIds, setIptcIds, iptcEditIssues, setIptcEditIssues, finishIptcWrite } = iptc;
  const { runQuarantine, inspectQuarantineReview } = quarantine;
  const { aiChipText, toggleAi } = ai;
  return (
    <>
      {iptcIds && (
        <IptcWriteDialog
          ids={iptcIds}
          progress={operationProgress}
          onClose={() => setIptcIds(null)}
          onDone={finishIptcWrite}
        />
      )}
      {iptcEditIssues && (
        <div className="modal-backdrop">
          <section
            className="confirm-dialog iptc-write-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Výsledok úpravy IPTC"
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                setIptcEditIssues(null);
              }
            }}
          >
            <b>Výsledok úpravy IPTC</b>
            <p>
              Upravené súbory: {iptcEditIssues.written} · bez zmeny: {iptcEditIssues.unchanged}
            </p>
            <IptcIssues title="Chyby" items={iptcEditIssues.errors} defaultOpen />
            <IptcIssues title="Upozornenia" items={iptcEditIssues.warnings} defaultOpen />
            <div className="confirm-actions">
              <button autoFocus onClick={() => setIptcEditIssues(null)}>
                Zavrieť
              </button>
            </div>
          </section>
        </div>
      )}
      {thumbnailInfoOpen && (
        <ThumbnailInfoDialog
          value={thumbnailInfo}
          onSave={(value) => {
            setThumbnailInfo(normalizeThumbnailInfo(value));
            setThumbnailInfoOpen(false);
          }}
          onClose={() => setThumbnailInfoOpen(false)}
        />
      )}
      {fileOp && (
        <FileOperationDialog
          mode={fileOp.mode}
          initialPath={view.kind === "folder" ? (view.folderPath ?? "") : ""}
          onChoose={(path) => void chooseOperationTarget(path)}
          onClose={() => setFileOp(null)}
          onCreate={createFolder}
        />
      )}
      {moveWarning && (
        <div className="modal-backdrop">
          <div
            className="confirm-dialog"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="move-uncategorized-title"
            aria-describedby="move-uncategorized-description"
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                setMoveWarning(null);
              }
            }}
          >
            <b id="move-uncategorized-title">Presunúť fotky bez kategórie?</b>
            <p id="move-uncategorized-description">
              {moveWarning.count === 1
                ? "Fotka, ktorú presúvaš, nie je v žiadnej kategórii."
                : `Počet presúvaných fotiek bez kategórie: ${moveWarning.count}.`}
            </p>
            <div className="confirm-actions">
              <button autoFocus onClick={() => setMoveWarning(null)}>
                Zrušiť
              </button>
              <button
                onClick={() => {
                  const { ids, path, conflict } = moveWarning;
                  setMoveWarning(null);
                  void runFileOperation(ids, "move", path, conflict, true);
                }}
              >
                Presunúť aj tak
              </button>
            </div>
          </div>
        </div>
      )}
      {fileConflict && (
        <div className="modal-backdrop">
          <section className="file-op-dialog conflict-dialog">
            <header>Konflikt názvov</header>
            <p>
              V cieli už existuje {fileConflict.count}{" "}
              {fileConflict.count === 1 ? "súbor" : fileConflict.count < 5 ? "súbory" : "súborov"} s rovnakým názvom.
            </p>
            <footer>
              <button
                onClick={() => {
                  void runFileOperation(fileConflict.ids, fileConflict.mode, fileConflict.path, "skip");
                  setFileConflict(null);
                }}
              >
                Preskočiť
              </button>
              <button
                onClick={() => {
                  void runFileOperation(fileConflict.ids, fileConflict.mode, fileConflict.path, "keep_both");
                  setFileConflict(null);
                }}
              >
                Ponechať oba (pridať číslo)
              </button>
              <button onClick={() => setFileConflict(null)}>Zrušiť</button>
            </footer>
          </section>
        </div>
      )}
      {folderRenameOp && (
        <FileNameDialog
          title="Premenovať priečinok"
          label="Nový názov priečinka"
          initialName={folderRenameOp.name}
          onSubmit={submitFolderRename}
          onClose={() => setFolderRenameOp(null)}
        />
      )}
      {nameOp && (
        <FileNameDialog
          title={nameOp.kind === "rename" ? "Premenovať súbor" : "Nový priečinok"}
          label={nameOp.kind === "rename" ? "Nový názov" : "Názov priečinka"}
          initialName={nameOp.kind === "rename" ? nameOp.name : ""}
          extension={nameOp.kind === "rename" ? nameOp.extension : undefined}
          onSubmit={submitName}
          onClose={() => setNameOp(null)}
        />
      )}
      {toast && <div className="app-toast">{toast}</div>}
      {quarantineReview && (
        <QuarantineReview
          review={quarantineReview}
          hidden={reviewInGrid}
          busy={quarantineBusy}
          progress={operationProgress}
          onShow={inspectQuarantineReview}
          onClose={() => setQuarantineReview(null)}
          onExecute={(selection, confirmedNoBinDrives) =>
            void runQuarantine("execute_quarantine_review", { selection, confirmedNoBinDrives })
          }
        />
      )}
      {quarantineConfirm && (
        <div className="modal-backdrop">
          <div
            className="confirm-dialog"
            role="alertdialog"
            aria-modal="true"
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                setQuarantineConfirm(null);
              }
            }}
          >
            <b>
              {quarantineConfirm.action === "restore" ? "Vrátiť fotky z karantény?" : "Presunúť fotky do karantény?"}
            </b>
            <p>{slovakItems(quarantineConfirm.ids.length)} vrátane príslušných XMP.</p>
            <p className="confirm-path">
              {quarantineConfirm.action === "restore"
                ? "Do pôvodných priečinkov. Pri kolízii názvov zostanú oba súbory."
                : quarantineSettings?.quarantineFolder || "Priečinok karantény vyberiete v nastaveniach."}
            </p>
            <div className="confirm-actions">
              <button autoFocus onClick={() => setQuarantineConfirm(null)}>
                Zrušiť
              </button>
              {quarantineConfirm.action === "restore" ? (
                <button
                  onClick={() => {
                    setQuarantineConfirm(null);
                    void runQuarantine("restore_from_quarantine", { assetIds: quarantineConfirm.ids });
                  }}
                >
                  Vrátiť späť
                </button>
              ) : (
                (
                  [
                    ["skip", "Presunúť, kolízie preskočiť"],
                    ["keep_both", "Presunúť, ponechať oba"],
                  ] as const
                ).map(([conflict, label]) => (
                  <button
                    key={conflict}
                    onClick={() => {
                      setQuarantineConfirm(null);
                      void runQuarantine("move_to_quarantine", {
                        assetIds: quarantineConfirm.ids,
                        conflict,
                        suggestionsOnly: !!quarantineConfirm.suggestionsOnly,
                      });
                    }}
                  >
                    {label}
                  </button>
                ))
              )}
            </div>
          </div>
        </div>
      )}
      {!!quarantineFailures.length && (
        <div className="modal-backdrop">
          <div className="confirm-dialog" role="alertdialog" aria-modal="true">
            <b>Nedokončené operácie karantény</b>
            <ul className="quarantine-failures">
              {quarantineFailures.map((failure, index) => (
                <li key={index}>{failure}</li>
              ))}
            </ul>
            <div className="confirm-actions">
              <button autoFocus onClick={() => setQuarantineFailures([])}>
                Zavrieť
              </button>
            </div>
          </div>
        </div>
      )}
      {helpOpen && <HelpDialog onClose={() => setHelpOpen(false)} />}
      {settingsOpen && (
        <SettingsDialog
          incomingFolder={incomingFolder ?? ""}
          readOnly={!!appInfo?.readonly}
          quarantineSettings={quarantineSettings}
          onQuarantineSettingsChange={saveQuarantineSettings}
          onIncomingFolderChange={changeIncomingFolder}
          onClose={() => setSettingsOpen(false)}
          onRule={(path, mode) => void changeFolderRule(path, mode)}
          showOtherFiles={showOtherFiles}
          onShowOtherFiles={setShowOtherFiles}
          aiEnabled={aiEnabled}
          aiLoadModelAtStart={aiLoadModelAtStart}
          onAiLoadModelAtStart={setAiLoadModelAtStart}
          aiAvailable={aiStatus.available}
          aiStatusText={aiChipText}
          onToggleAi={() => void toggleAi()}
          uiScale={uiScale}
          onUiScale={setUiScale}
          onRemoveSource={(path) => void requestRemoveCatalog(path)}
        />
      )}
      {removeCatalogConfirm && (
        <div className="modal-backdrop">
          <div className="confirm-dialog" role="alertdialog">
            <b>Odstrániť zdroj z katalógu?</b>
            <p>
              Z katalógu sa odstráni <b>{removeCatalogConfirm.info.assets.toLocaleString("sk-SK")}</b> záznamov o
              súboroch a <b>{removeCatalogConfirm.info.folders.toLocaleString("sk-SK")}</b> priečinkov z „
              {removeCatalogConfirm.path}“.
            </p>
            <p>
              Súbory sa nemažú –{" "}
              {removeCatalogConfirm.info.offline ? "disk nie je pripojený" : "priečinok už neexistuje"}. Kategórie,
              hodnotenia, kľúčové slová a osoby týchto fotiek sa z katalógu stratia. Pred odstránením sa vytvorí záloha
              katalógu.
            </p>
            <div className="confirm-actions">
              <button onClick={() => setRemoveCatalogConfirm(null)}>Zrušiť</button>
              <button className="danger" autoFocus onClick={() => void removeCatalogSource()}>
                Odstrániť z katalógu
              </button>
            </div>
          </div>
        </div>
      )}
      {excludeConfirm && (
        <div className="modal-backdrop">
          <div className="confirm-dialog" role="alertdialog">
            <b>Vylúčiť priečinok z katalógu?</b>
            <p className="confirm-path">{excludeConfirm.path}</p>
            <p>
              V katalógu je pod ním <b>{excludeConfirm.impact.files}</b>{" "}
              {excludeConfirm.impact.files === 1 ? "súbor" : excludeConfirm.impact.files < 5 ? "súbory" : "súborov"}: s
              kategóriami {excludeConfirm.impact.withCategories}, s kľúčovými slovami{" "}
              {excludeConfirm.impact.withKeywords}, s hodnotením/štítkom/označením{" "}
              {excludeConfirm.impact.withRatingLabelTagged}, s opravou farieb {excludeConfirm.impact.withColorOverrides}
              .
            </p>
            <p>Tieto údaje sa z katalógu odstránia. Súbory na disku ostanú a priečinok sa dá ďalej prehliadať.</p>
            <div className="confirm-actions">
              <button onClick={() => setExcludeConfirm(null)}>Zrušiť</button>
              <button
                className="danger"
                autoFocus
                onClick={() => {
                  const target = excludeConfirm.path;
                  setExcludeConfirm(null);
                  void applyFolderRule(target, "exclude");
                }}
              >
                Vylúčiť a odstrániť z katalógu
              </button>
            </div>
          </div>
        </div>
      )}
      {folderDeleteConfirm && (
        <div className="modal-backdrop">
          <div className="confirm-dialog" role="alertdialog">
            <b>Presunúť priečinok „{folderDeleteConfirm.path.split(/[\\/]/).filter(Boolean).pop()}“ do Koša?</b>
            <p>
              Obsahuje {folderDeleteConfirm.info.files.toLocaleString("sk-SK")} súborov v{" "}
              {folderDeleteConfirm.info.folders.toLocaleString("sk-SK")} priečinkoch (
              {formatBytes(folderDeleteConfirm.info.bytes)}), z toho{" "}
              {folderDeleteConfirm.info.catalogued.toLocaleString("sk-SK")} fotiek v katalógu. Údaje o fotkách sa
              odstránia z katalógu.
            </p>
            <div className="confirm-actions">
              <button onClick={() => setFolderDeleteConfirm(null)}>Zrušiť</button>
              <button className="danger" onClick={() => void confirmFolderDelete()}>
                Presunúť do Koša
              </button>
            </div>
          </div>
        </div>
      )}
      {deleteConfirm &&
        (() => {
          const info = deleteConfirm.info;
          const n = info?.count ?? deleteConfirm.ids.length;
          const perm = info?.permanentCount ?? 0;
          const files = (k: number) => (k === 1 ? "súbor" : k < 5 ? "súbory" : "súborov");
          const allPerm = perm >= n;
          return (
            <div className="modal-backdrop">
              <div
                className="confirm-dialog"
                role="alertdialog"
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.stopPropagation();
                    setDeleteConfirm(null);
                  }
                }}
              >
                <b>
                  {allPerm
                    ? n === 1
                      ? "Zmazať súbor natrvalo?"
                      : `Zmazať ${n} ${files(n)} natrvalo?`
                    : n === 1
                      ? "Presunúť súbor do Koša?"
                      : `Presunúť ${n} ${files(n)} do Koša?`}
                </b>
                {(info?.names ?? (deleteConfirm.name ? [deleteConfirm.name] : [])).length > 0 && (
                  <p className="confirm-path">
                    {(info?.names ?? [deleteConfirm.name]).join(", ")}
                    {n > (info?.names.length ?? 1) ? ` a ďalších ${n - (info?.names.length ?? 1)}` : ""}
                  </p>
                )}
                {perm > 0 && (
                  <p className="delete-warning">
                    {allPerm ? "Súbory sa zmažú natrvalo." : `${perm} z ${n} sa zmaže natrvalo, ostatné pôjdu do Koša.`}
                    {info && info.noBinDrives.length > 0
                      ? ` Disk ${info.noBinDrives.join(", ")} nemá Kôš (USB alebo sieťový disk).`
                      : deleteConfirm.permanent
                        ? " (Shift+Delete)"
                        : ""}
                  </p>
                )}
                <p>
                  {info && info.sidecars > 0
                    ? `Spolu s nimi sa zmaže ${info.sidecars} ${files(info.sidecars)} .xmp. `
                    : "Sprievodné súbory (.xmp) sa zmažú s nimi. "}
                  Ich údaje sa odstránia z katalógu.
                </p>
                <div className="confirm-actions">
                  <button autoFocus={allPerm} onClick={() => setDeleteConfirm(null)}>
                    Zrušiť
                  </button>
                  <button
                    className="danger"
                    autoFocus={!allPerm}
                    onClick={() => {
                      if (deleteConfirm.quarantine) {
                        setDeleteConfirm(null);
                        void runQuarantine("delete_from_quarantine", {
                          assetIds: deleteConfirm.ids,
                          confirmedNoBinDrives: info?.noBinDrives ?? [],
                        });
                      } else void runDelete(deleteConfirm.ids, deleteConfirm.permanent, info?.noBinDrives ?? []);
                    }}
                  >
                    {allPerm ? "Zmazať natrvalo" : perm > 0 ? "Zmazať" : "Presunúť do Koša"}
                  </button>
                </div>
              </div>
            </div>
          );
        })()}
    </>
  );
}

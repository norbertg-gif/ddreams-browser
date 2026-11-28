import { labelText } from "./AssetGrid";
import { DockLayoutMenu } from "./Dock";
import type { ViewFilters } from "../types";
import { categoryAssignmentLabels, keywordAssignmentLabels } from "./WindowToolbar";
import type { Dispatch, SetStateAction, RefObject } from "react";
import type { DockLayout } from "../dockLayout";
import type { useHelpShortcut } from "../hooks/useKeyboardShortcuts";
import type { AppInfo, useListingRefresh, useNavigation } from "../hooks/useNavigation";
import type { useAiSettings, useAi } from "../hooks/useAi";

type WindowTopBarProps = {
  slovakItems: (count: number) => string;
  appInfo: AppInfo | null;
  catalogTooltip: string;
  query: string;
  setQuery: Dispatch<SetStateAction<string>>;
  dockLayout: DockLayout;
  setDockLayout: Dispatch<SetStateAction<DockLayout>>;
  filters: ViewFilters;
  setFilters: Dispatch<SetStateAction<ViewFilters>>;
  setSettingsOpen: Dispatch<SetStateAction<boolean>>;
  total: number;
  historyOpen: boolean;
  setHistoryOpen: Dispatch<SetStateAction<boolean>>;
  historyRef: RefObject<HTMLDivElement | null>;
  help: Pick<ReturnType<typeof useHelpShortcut>, "setHelpOpen">;
  listing: Pick<ReturnType<typeof useListingRefresh>, "recentChanges" | "loadRecentChanges">;
  aiSettings: Pick<ReturnType<typeof useAiSettings>, "aiStatus" | "aiEnabled" | "aiOpen" | "setAiOpen" | "aiRef">;
  navigation: Pick<ReturnType<typeof useNavigation>, "crumbs" | "openCrumb">;
  ai: Pick<ReturnType<typeof useAi>, "aiChipText" | "toggleAi" | "updateAiStatus">;
};

export function WindowTopBar({
  slovakItems,
  appInfo,
  catalogTooltip,
  query,
  setQuery,
  dockLayout,
  setDockLayout,
  filters,
  setFilters,
  setSettingsOpen,
  total,
  historyOpen,
  setHistoryOpen,
  historyRef,
  help,
  listing,
  aiSettings,
  navigation,
  ai,
}: WindowTopBarProps) {
  const { setHelpOpen } = help;
  const { recentChanges, loadRecentChanges } = listing;
  const { aiStatus, aiEnabled, aiOpen, setAiOpen, aiRef } = aiSettings;
  const { crumbs, openCrumb } = navigation;
  const { aiChipText, toggleAi, updateAiStatus } = ai;
  return (
    <header className="topbar">
      <div className="top-left">
        <div className="brand-mark">D</div>
        <div className="brand-title">
          DDreams <b>browser</b>
        </div>
        <nav className="top-crumbs">
          {crumbs.map((crumb, i) => (
            <span key={`${crumb}-${i}`} className="crumb-item">
              {i > 0 && <i>›</i>}
              <button onClick={() => void openCrumb(i)}>{crumb}</button>
            </span>
          ))}
          {Object.values(filters).some(Boolean) && (
            <span className="active-filters">
              {filters.ratingMode && `Hodnotenie ${filters.ratingMode} ${filters.ratingValue ?? ""}`}{" "}
              {filters.labelFilter && `Farebný štítok: ${labelText(filters.labelFilter)}`}{" "}
              {filters.taggedOnly && "Označené"}{" "}
              {filters.categoryFilter && categoryAssignmentLabels[filters.categoryFilter]}{" "}
              {filters.keywordFilter && keywordAssignmentLabels[filters.keywordFilter]}{" "}
              {filters.fileGroup && `Typ: ${filters.fileGroup === "all" ? "všetky súbory" : filters.fileGroup}`}
              <button onClick={() => setFilters({})}>×</button>
            </span>
          )}
        </nav>
        <span className="count-pill">{slovakItems(total)}</span>
      </div>
      <label className="search-box">
        <span>⌕</span>
        <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Filtrovať v zobrazení…" />
        <kbd>Ctrl+F</kbd>
      </label>
      <div className="top-right">
        <DockLayoutMenu layout={dockLayout} setLayout={setDockLayout} />
        <div className="ai-anchor" ref={aiRef}>
          <button
            className={`ai-chip ai-${aiStatus.state}`}
            title={
              !aiStatus.available
                ? (aiStatus.reason ?? "AI modul nie je nainštalovaný")
                : (aiStatus.lastError ?? "AI index a vyhľadávanie")
            }
            onClick={() => setAiOpen((v) => !v)}
          >
            {aiChipText}
          </button>
          {aiOpen && (
            <div className="ai-popover">
              <b>AI katalóg</b>
              <div>
                <span>Zaindexované</span>
                <strong>{aiStatus.indexed.toLocaleString("sk-SK")}</strong>
              </div>
              <div>
                <span>Vo fronte</span>
                <strong>{aiStatus.queued.toLocaleString("sk-SK")}</strong>
              </div>
              <div>
                <span>Čaká na spracovanie</span>
                <strong>{aiStatus.waiting.toLocaleString("sk-SK")}</strong>
              </div>
              <div>
                <span>AI model</span>
                <strong>
                  {(
                    { unloaded: "nenačítaný", loading: "načítava sa", ready: "pripravený", error: "chyba" } as Record<
                      string,
                      string
                    >
                  )[aiStatus.modelState] ?? aiStatus.modelState}
                </strong>
              </div>
              {aiStatus.modelState === "unloaded" && (
                <button
                  disabled={!aiStatus.available || !aiEnabled}
                  onClick={() => void updateAiStatus("ai_load_model")}
                >
                  Spracovať čakajúce fotografie <small>vyžaduje AI model</small>
                </button>
              )}
              <div>
                <span>Offline</span>
                <strong>{aiStatus.offline.toLocaleString("sk-SK")}</strong>
              </div>
              <div>
                <span>Chyby</span>
                <strong>{aiStatus.failed.toLocaleString("sk-SK")}</strong>
              </div>
              {aiStatus.lastError && <small>{aiStatus.lastError}</small>}
              <button
                disabled={!aiStatus.available || !aiEnabled || aiStatus.state === "loading"}
                onClick={() => void updateAiStatus(aiStatus.state === "paused" ? "ai_resume" : "ai_pause")}
              >
                {aiStatus.state === "paused" ? "Pokračovať" : "Pozastaviť"}
              </button>
              <button disabled={!aiStatus.available || !aiEnabled} onClick={() => void updateAiStatus("ai_reindex")}>
                Aktualizovať index
              </button>
              <button onClick={() => void toggleAi()}>{aiEnabled ? "Vypnúť AI" : "Zapnúť AI"}</button>
            </div>
          )}
        </div>
        <div className="history-anchor" ref={historyRef}>
          <button
            className="catalog-label"
            title={catalogTooltip}
            onClick={() => {
              setHistoryOpen((v) => !v);
              loadRecentChanges();
            }}
          >
            <span className="status-light" />
            {appInfo?.readonly ? "Katalóg iba na čítanie" : "Katalóg"}
          </button>
          {historyOpen && (
            <div className="change-history">
              <div className="change-history-head">
                <b>História zmien</b>
              </div>
              {recentChanges.length ? (
                recentChanges.map((change) => (
                  <div key={change.id}>
                    <time>{new Date(change.ts).toLocaleString("sk-SK")}</time>
                    <span>{change.summary}</span>
                  </div>
                ))
              ) : (
                <small>Žiadne zmeny</small>
              )}
            </div>
          )}
        </div>
        <button className="settings-button" title="Pomocník (F1)" onClick={() => setHelpOpen(true)}>
          ?
        </button>
        <button className="settings-button" title="Nastavenia" onClick={() => setSettingsOpen(true)}>
          ⚙
        </button>
      </div>
    </header>
  );
}

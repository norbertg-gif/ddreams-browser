import { createPortal } from "react-dom";
import { labelText } from "./AssetGrid";
import type { useTransientDetails } from "../uiState";
import type { ViewFilters, ValueCount } from "../types";
import type { SeriesPreset, useSeriesSearchSettings, useAiSettings, useAi } from "../hooks/useAi";
import type { Dispatch, SetStateAction } from "react";
import type { useHelpShortcut } from "../hooks/useKeyboardShortcuts";
import type { useNavigation, useListingRefresh } from "../hooks/useNavigation";

export const categoryAssignmentLabels = { with: "Zaradené do kategórie", without: "Bez kategórie" };
export const keywordAssignmentLabels = { with: "S kľúčovými slovami", without: "Bez kľúčových slov" };

type WindowToolbarProps = {
  seriesMenu: ReturnType<typeof useTransientDetails>;
  filtersMenu: ReturnType<typeof useTransientDetails>;
  displayMenu: ReturnType<typeof useTransientDetails>;
  zoom: number;
  setZoom: Dispatch<SetStateAction<number>>;
  centerMode: "map" | "grid";
  setCenterMode: Dispatch<SetStateAction<"map" | "grid">>;
  overlayMode: 0 | 2;
  setOverlayMode: Dispatch<SetStateAction<0 | 2>>;
  hoverMode: "off" | "shift" | "hover";
  setHoverMode: Dispatch<SetStateAction<"off" | "shift" | "hover">>;
  setThumbnailInfoOpen: Dispatch<SetStateAction<boolean>>;
  filters: ViewFilters;
  setFilters: Dispatch<SetStateAction<ViewFilters>>;
  availableLabels: ValueCount[];
  sort: string;
  descending: boolean;
  editPendingView: boolean;
  changeSort: (nextSort: string, nextDescending: boolean) => void;
  help: Pick<ReturnType<typeof useHelpShortcut>, "setHelpOpen">;
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "history" | "historyIndex" | "travel" | "up">;
  series: Pick<ReturnType<typeof useSeriesSearchSettings>, "seriesPreset" | "setSeriesPreset" | "seriesBusy">;
  aiSettings: Pick<
    ReturnType<typeof useAiSettings>,
    "aiStatus" | "aiQuery" | "setAiQuery" | "aiSearchBusy" | "aiSearchTarget" | "aiReady"
  >;
  listing: Pick<ReturnType<typeof useListingRefresh>, "refresh">;
  ai: Pick<ReturnType<typeof useAi>, "aiSearch" | "findSeries">;
};

export function WindowToolbar({
  seriesMenu,
  filtersMenu,
  displayMenu,
  zoom,
  setZoom,
  centerMode,
  setCenterMode,
  overlayMode,
  setOverlayMode,
  hoverMode,
  setHoverMode,
  setThumbnailInfoOpen,
  filters,
  setFilters,
  availableLabels,
  sort,
  descending,
  editPendingView,
  changeSort,
  help,
  navigation,
  series,
  aiSettings,
  listing,
  ai,
}: WindowToolbarProps) {
  const { setHelpOpen } = help;
  const { view, history, historyIndex, travel, up } = navigation;
  const { seriesPreset, setSeriesPreset, seriesBusy } = series;
  const { aiStatus, aiQuery, setAiQuery, aiSearchBusy, aiSearchTarget, aiReady } = aiSettings;
  const { refresh } = listing;
  const { aiSearch, findSeries } = ai;
  return (
    <>
      <div className="toolbar">
        <div className="toolbar-left">
          <button className="nav-btn" disabled={historyIndex <= 0} onClick={() => travel(historyIndex - 1)}>
            ←
          </button>
          <button
            className="nav-btn"
            disabled={historyIndex >= history.length - 1}
            onClick={() => travel(historyIndex + 1)}
          >
            →
          </button>
          <button className="nav-btn" disabled={view.kind !== "folder"} onClick={() => void up()}>
            ↑
          </button>
          <button className="nav-btn" onClick={refresh}>
            ↻
          </button>
          <div className="type-pills">
            {(
              [
                ["", "Všetko"],
                ["images", "Fotografie"],
                ["raw", "RAW"],
                ["video", "Video"],
              ] as const
            ).map(([v, l]) => (
              <button
                key={v}
                className={`pill ${(filters.fileGroup ?? "") === v ? "active" : ""}`}
                onClick={() => setFilters((f) => ({ ...f, fileGroup: (v || undefined) as ViewFilters["fileGroup"] }))}
              >
                {l}
              </button>
            ))}
          </div>
        </div>
        <div className="toolbar-right">
          {view.kind === "ai" || view.kind === "series" || view.kind === "people" ? (
            <select
              disabled
              value="relevance"
              title={
                view.kind === "people" ? "Fotky osoby sú zoradené podľa dátumu" : "Výsledky AI sú zoradené podľa zhody"
              }
            >
              <option value="relevance">{view.kind === "people" ? "Podľa dátumu" : "Podľa zhody"}</option>
            </select>
          ) : (
            <>
              <select
                disabled={editPendingView}
                value={editPendingView ? "editPending" : sort}
                onChange={(e) => changeSort(e.target.value, descending)}
              >
                {editPendingView && <option value="editPending">Najdlhšie čakajúce</option>}
                {[
                  ["name", "Názov"],
                  ["dateTaken", "Dátum nasnímania"],
                  ["modified", "Dátum zmeny"],
                  ["size", "Veľkosť"],
                  ["type", "Typ súboru"],
                  ["dimensions", "Rozmery"],
                  ["rating", "Hodnotenie"],
                  ["label", "Farebný štítok"],
                  ["tagged", "Označené"],
                  ["caption", "Popis"],
                ].map(([v, l]) => (
                  <option value={v} key={v}>
                    {l}
                  </option>
                ))}
                {aiReady && <option value="similarity">Podľa podobnosti</option>}
              </select>
              <button
                className="direction-button"
                disabled={editPendingView}
                title={!editPendingView && descending ? "Zostupne" : "Vzostupne"}
                onClick={() => changeSort(sort, !descending)}
              >
                {!editPendingView && descending ? "↓" : "↑"}
              </button>
            </>
          )}
          {view.kind !== "series" && (
            <details
              {...seriesMenu}
              className="toolbar-menu"
              title={!aiReady ? (aiStatus.reason ?? "AI nie je pripravená") : undefined}
            >
              <summary>Série</summary>
              <div className="toolbar-pop">
                <select
                  value={seriesPreset}
                  disabled={!aiReady || seriesBusy}
                  onChange={(e) => setSeriesPreset(e.target.value as SeriesPreset)}
                >
                  <option value="strict">Prísne</option>
                  <option value="normal">Stredné</option>
                  <option value="loose">Voľné</option>
                </select>
                <button
                  disabled={!aiReady || seriesBusy}
                  title={!aiReady ? (aiStatus.reason ?? "AI nie je pripravená") : undefined}
                  onClick={() => void findSeries()}
                >
                  {seriesBusy ? "Hľadám série…" : "Nájsť série v tomto zobrazení"}
                </button>
              </div>
            </details>
          )}
          <details {...filtersMenu} className="toolbar-menu">
            <summary>Filtre</summary>
            <div className="toolbar-pop">
              <select
                value={filters.ratingMode ?? ""}
                onChange={(e) =>
                  setFilters((f) => ({
                    ...f,
                    ratingMode: (e.target.value || undefined) as ViewFilters["ratingMode"],
                    ratingValue: e.target.value && e.target.value !== "unrated" ? (f.ratingValue ?? 1) : undefined,
                  }))
                }
              >
                <option value="">Hodnotenie: všetky</option>
                <option value="equal">Rovné</option>
                <option value="atLeast">Aspoň</option>
                <option value="atMost">Najviac</option>
                <option value="unrated">Bez hodnotenia</option>
              </select>
              {filters.ratingMode && filters.ratingMode !== "unrated" && (
                <select
                  value={filters.ratingValue ?? 1}
                  onChange={(e) => setFilters((f) => ({ ...f, ratingValue: Number(e.target.value) }))}
                >
                  {[1, 2, 3, 4, 5].map((v) => (
                    <option value={v} key={v}>
                      {v}
                    </option>
                  ))}
                </select>
              )}
              <select
                value={filters.labelFilter ?? ""}
                onChange={(e) => setFilters((f) => ({ ...f, labelFilter: e.target.value || undefined }))}
              >
                <option value="">Farebný štítok: všetky</option>
                {availableLabels
                  .filter((v) => v.value)
                  .map((v) => (
                    <option value={v.value} key={v.value}>
                      {labelText(v.value)}
                    </option>
                  ))}
              </select>
              <label>
                <input
                  type="checkbox"
                  checked={!!filters.taggedOnly}
                  onChange={(e) => setFilters((f) => ({ ...f, taggedOnly: e.target.checked || undefined }))}
                />{" "}
                Len označené
              </label>
              <select
                aria-label="Priradenie kategórií"
                value={filters.categoryFilter ?? ""}
                onChange={(e) =>
                  setFilters((f) => ({
                    ...f,
                    categoryFilter: (e.target.value || undefined) as ViewFilters["categoryFilter"],
                  }))
                }
              >
                <option value="">Kategórie: všetky</option>
                <option value="with">{categoryAssignmentLabels.with}</option>
                <option value="without">{categoryAssignmentLabels.without}</option>
              </select>
              <select
                aria-label="Priradenie kľúčových slov"
                value={filters.keywordFilter ?? ""}
                onChange={(e) =>
                  setFilters((f) => ({
                    ...f,
                    keywordFilter: (e.target.value || undefined) as ViewFilters["keywordFilter"],
                  }))
                }
              >
                <option value="">Kľúčové slová: všetky</option>
                <option value="with">{keywordAssignmentLabels.with}</option>
                <option value="without">{keywordAssignmentLabels.without}</option>
              </select>
              <select
                value={filters.fileGroup ?? ""}
                onChange={(e) =>
                  setFilters((f) => ({
                    ...f,
                    fileGroup: (e.target.value || undefined) as ViewFilters["fileGroup"],
                  }))
                }
              >
                <option value="">Typ: fotky a videá</option>
                {[
                  ["images", "Fotografie"],
                  ["raw", "RAW"],
                  ["video", "Video"],
                  ["other", "Ostatné"],
                  ["all", "Všetky súbory"],
                ].map(([v, l]) => (
                  <option value={v} key={v}>
                    {l}
                  </option>
                ))}
              </select>
              <button onClick={() => setFilters({})}>Vymazať filtre</button>
            </div>
          </details>
          <details {...displayMenu} className="toolbar-menu">
            <summary>Zobrazenie</summary>
            <div className="toolbar-pop">
              <select
                value={hoverMode}
                onChange={(e) => setHoverMode(e.target.value as typeof hoverMode)}
                title="Náhľad po prejdení myšou"
              >
                <option value="off">Náhľad vypnutý</option>
                <option value="hover">Náhľad pri prejdení</option>
                <option value="shift">Náhľad so Shift</option>
              </select>
              <button onClick={() => setOverlayMode((v) => (v === 0 ? 2 : 0))} title="Ikonky na náhľadoch, kláves ]">
                Ikonky {overlayMode === 0 ? "vypnuté" : "zapnuté"}
              </button>
              <button onClick={() => setThumbnailInfoOpen(true)}>&#218;daje pod n&#225;h&#318;admi&#8230;</button>
              <button onClick={() => setHelpOpen(true)}>Pomocník (F1)</button>
            </div>
          </details>
          <div className="center-mode-pills" title="Zobrazenie stredu: mriežka náhľadov alebo mapa katalógu">
            <button className={centerMode === "grid" ? "active" : ""} onClick={() => setCenterMode("grid")}>
              Mriežka
            </button>
            <button className={centerMode === "map" ? "active" : ""} onClick={() => setCenterMode("map")}>
              Mapa
            </button>
          </div>
          <div className="zoom-control">
            <span>−</span>
            <input
              type="range"
              min="80"
              max="400"
              step="8"
              value={zoom}
              onChange={(e) => setZoom(Number(e.target.value))}
            />
            <span>＋</span>
          </div>
        </div>
      </div>
      {aiSearchTarget &&
        createPortal(
          <label className="ai-search-box" title={!aiReady ? (aiStatus.reason ?? "AI modul nie je nainštalovaný") : ""}>
            <span>AI</span>
            <small className="ai-model-hint">vyžaduje AI model</small>
            <input
              value={aiQuery}
              disabled={!aiReady || aiSearchBusy}
              onChange={(e) => setAiQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !aiSearchBusy) void aiSearch();
              }}
              placeholder="Hľadať obsahom (AI)…"
            />
            {aiSearchBusy && <i className="ai-search-spinner" aria-label="Vyh?ad?vam" />}
          </label>,
          aiSearchTarget,
        )}
    </>
  );
}

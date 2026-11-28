import type { buildAssetFilter } from "../assetFilter";
import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Asset } from "../types";
import { readStored, writeStored } from "../storage";
import type { useNavigation } from "./useNavigation";
import type { useAssetValues } from "./useAssetValues";

type AiStatus = {
  available: boolean;
  reason: string | null;
  state: string;
  modelState: string;
  waiting: number;
  indexed: number;
  queued: number;
  done: number;
  failed: number;
  offline: number;
  total: number;
  ratePerS: number;
  etaS: number | null;
  lastError: string | null;
};
type AiSearchPage = { ids: number[]; searched: number };
export type SeriesPreset = "strict" | "normal" | "loose";
type AiSeriesResponse = { groups: { ids: number[]; best: number }[]; analyzed: number; truncated: boolean };

export function useSeriesSearchSettings() {
  const [seriesPreset, setSeriesPreset] = useState<SeriesPreset>(() => {
    const saved = readStored<string>("ddreams.seriesPreset", "normal");
    return saved === "strict" || saved === "loose" ? saved : "normal";
  });
  const [seriesBusy, setSeriesBusy] = useState(false);
  useEffect(() => writeStored("ddreams.seriesPreset", seriesPreset), [seriesPreset]);
  return { seriesPreset, setSeriesPreset, seriesBusy, setSeriesBusy };
}

export function useAiSettings() {
  const [aiStatus, setAiStatus] = useState<AiStatus>({
    available: false,
    reason: "AI modul nie je nainštalovaný",
    state: "off",
    modelState: "unloaded",
    waiting: 0,
    indexed: 0,
    queued: 0,
    done: 0,
    failed: 0,
    offline: 0,
    total: 0,
    ratePerS: 0,
    etaS: null,
    lastError: null,
  });
  const [aiEnabled, setAiEnabled] = useState(() => readStored("ddreams.aiEnabled", true));
  const [aiLoadModelAtStart, setAiLoadModelAtStart] = useState(() => readStored("ddreams.aiLoadModelAtStart", true));
  const [aiOpen, setAiOpen] = useState(false);

  const [aiQuery, setAiQuery] = useState("");
  const [aiSearchBusy, setAiSearchBusy] = useState(false);
  const aiRef = useRef<HTMLDivElement>(null);
  const [aiSearchTarget, setAiSearchTarget] = useState<HTMLElement | null>(null);
  const aiReady =
    aiStatus.available &&
    aiEnabled &&
    (aiStatus.state === "ready" || aiStatus.state === "indexing" || aiStatus.state === "paused");
  useEffect(() => writeStored("ddreams.aiEnabled", aiEnabled), [aiEnabled]);
  useEffect(() => writeStored("ddreams.aiLoadModelAtStart", aiLoadModelAtStart), [aiLoadModelAtStart]);
  const [aiTick, setAiTick] = useState(0);
  return {
    aiStatus,
    setAiStatus,
    aiEnabled,
    setAiEnabled,
    aiLoadModelAtStart,
    setAiLoadModelAtStart,
    aiOpen,
    setAiOpen,
    aiQuery,
    setAiQuery,
    aiSearchBusy,
    setAiSearchBusy,
    aiRef,
    aiSearchTarget,
    setAiSearchTarget,
    aiReady,
    aiTick,
    setAiTick,
  };
}

type AiStatusOptions = {
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "setView">;
  aiSettings: Pick<ReturnType<typeof useAiSettings>, "aiStatus" | "setAiStatus" | "setAiSearchTarget" | "setAiTick">;
};

export function useAiStatus({ navigation, aiSettings }: AiStatusOptions) {
  const { view, setView } = navigation;
  const { aiStatus, setAiStatus, setAiSearchTarget, setAiTick } = aiSettings;
  useEffect(() => {
    if (aiStatus.state !== "indexing") return;
    const timer = window.setInterval(() => setAiTick((v) => v + 1), 20000);
    return () => window.clearInterval(timer);
  }, [aiStatus.state]);
  useEffect(() => {
    if (view.kind !== "ai" || !view.title.startsWith("AI:")) return;
    const base = view.title.replace(/ · prehľadané [\d\s\u00a0]+ z [\d\s\u00a0]+$/u, "");
    const suffix =
      aiStatus.indexed < aiStatus.total
        ? ` · prehľadané ${aiStatus.indexed.toLocaleString("sk-SK")} z ${aiStatus.total.toLocaleString("sk-SK")}`
        : "";
    const title = base + suffix;
    if (title !== view.title) setView((current) => (current === view ? { ...current, title } : current));
  }, [aiStatus.indexed, aiStatus.total, view]);
  useEffect(() => setAiSearchTarget(document.querySelector<HTMLElement>(".toolbar-left")), []);
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<AiStatus>("ai-status", (event) => {
      if (active) setAiStatus(event.payload);
    }).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });
    void invoke<AiStatus>("ai_status")
      .then((status) => {
        if (!active) return;
        setAiStatus(status);
      })
      .catch(() => {});
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);
}

type AiStartupOptions = {
  ready: boolean;
  aiSettings: Pick<ReturnType<typeof useAiSettings>, "setAiStatus" | "aiEnabled" | "aiLoadModelAtStart">;
};

export function useAiStartup({ ready, aiSettings }: AiStartupOptions) {
  const { setAiStatus, aiEnabled, aiLoadModelAtStart } = aiSettings;
  useEffect(() => {
    if (!ready || !aiEnabled) return;
    const timer = window.setTimeout(() => {
      void invoke<AiStatus>("ai_status")
        .then((status) => {
          if (status.available && status.state === "off")
            void invoke<AiStatus>("ai_start", { lazyModel: !aiLoadModelAtStart })
              .then(setAiStatus)
              .catch((error) => setAiStatus((s) => ({ ...s, state: "error", lastError: String(error) })));
        })
        .catch(() => {});
    }, 4000);
    return () => window.clearTimeout(timer);
  }, [ready]);
}

type AiSuggestionOptions = {
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "navigate">;
  assetValues: Pick<ReturnType<typeof useAssetValues>, "setAssetCategories" | "setAssetKeywords">;
};

export function useAiSuggestion({ navigation, assetValues }: AiSuggestionOptions) {
  const { view, navigate } = navigation;
  const { setAssetCategories, setAssetKeywords } = assetValues;
  const [applyingSuggestion, setApplyingSuggestion] = useState(false);
  const currentView = useRef(view);
  currentView.current = view;
  const applySuggestion = async (ids: number[]) => {
    const suggestion = view.suggestion;
    if (!suggestion || !ids.length || applyingSuggestion) return;
    setApplyingSuggestion(true);
    try {
      const saved =
        suggestion.kind === "category"
          ? await setAssetCategories(ids, suggestion.labelId, true)
          : await setAssetKeywords(ids, suggestion.labelId, true);
      if (saved && currentView.current === view) {
        const applied = new Set(ids);
        navigate({ ...view, ids: (view.ids ?? []).filter((id) => !applied.has(id)) });
      }
    } finally {
      setApplyingSuggestion(false);
    }
  };
  return { applyingSuggestion, applySuggestion };
}

type AiOptions = {
  setToast: Dispatch<SetStateAction<string>>;
  assetFilter: ReturnType<typeof buildAssetFilter>;
  navigation: Pick<ReturnType<typeof useNavigation>, "view" | "navigate">;
  series: Pick<ReturnType<typeof useSeriesSearchSettings>, "seriesPreset" | "seriesBusy" | "setSeriesBusy">;
  aiSettings: Pick<
    ReturnType<typeof useAiSettings>,
    | "aiStatus"
    | "setAiStatus"
    | "aiEnabled"
    | "setAiEnabled"
    | "aiLoadModelAtStart"
    | "aiOpen"
    | "setAiOpen"
    | "aiQuery"
    | "aiSearchBusy"
    | "setAiSearchBusy"
    | "aiRef"
    | "aiReady"
  >;
};

export function useAi({ setToast, assetFilter, navigation, series, aiSettings }: AiOptions) {
  const { view, navigate } = navigation;
  const { seriesPreset, seriesBusy, setSeriesBusy } = series;
  const {
    aiStatus,
    setAiStatus,
    aiEnabled,
    setAiEnabled,
    aiLoadModelAtStart,
    aiOpen,
    setAiOpen,
    aiQuery,
    aiSearchBusy,
    setAiSearchBusy,
    aiRef,
    aiReady,
  } = aiSettings;
  useEffect(() => {
    if (!aiOpen) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !aiRef.current?.contains(event.target)) setAiOpen(false);
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setAiOpen(false);
      }
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", escape);
    };
  }, [aiOpen]);
  const aiChipText =
    aiStatus.modelState === "error"
      ? "Chyba AI modelu"
      : aiStatus.state === "loading"
        ? "AI sa načítava…"
        : aiStatus.state === "indexing"
          ? `AI indexuje ${aiStatus.indexed.toLocaleString("sk-SK")} / ${aiStatus.total.toLocaleString("sk-SK")}${aiStatus.etaS != null ? ` · ~${Math.max(1, Math.round(aiStatus.etaS / 3600))} h` : ""}`
          : aiStatus.state === "ready"
            ? "AI pripravené"
            : aiStatus.state === "paused"
              ? "AI pozastavené"
              : aiStatus.state === "error"
                ? "AI chyba"
                : "AI vypnuté";
  const aiSearch = async () => {
    const text = aiQuery.trim();
    if (!text || !aiReady || aiSearchBusy) return;
    setAiSearchBusy(true);
    try {
      const result = await invoke<AiSearchPage>("ai_search_text", { query: text, limit: 300 });
      if (result.ids.length) {
        const suffix =
          result.searched < aiStatus.total
            ? ` · prehľadané ${result.searched.toLocaleString("sk-SK")} z ${aiStatus.total.toLocaleString("sk-SK")}`
            : "";
        navigate({ kind: "ai", ids: result.ids, title: `AI: „${text}“${suffix}` });
      } else {
        setToast(
          `AI nenašlo nič (prehľadané ${result.searched.toLocaleString("sk-SK")} z ${aiStatus.total.toLocaleString("sk-SK")})`,
        );
        window.setTimeout(() => setToast(""), 3500);
      }
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 3500);
    } finally {
      setAiSearchBusy(false);
    }
  };
  const findSimilar = async (asset: Asset) => {
    try {
      const result = await invoke<AiSearchPage>("ai_similar", { assetId: asset.id, limit: 300 });
      if (result.ids.length) navigate({ kind: "ai", ids: result.ids, title: `Podobné ako ${asset.name}` });
      else {
        setToast("AI nenašlo podobné fotky");
        window.setTimeout(() => setToast(""), 2500);
      }
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 3500);
    }
  };
  const findSeries = async () => {
    if (!aiReady || seriesBusy) return;
    setSeriesBusy(true);
    try {
      const filter = { ...assetFilter, offset: 0, limit: 500 };
      const result = await invoke<AiSeriesResponse>("ai_series", { filter, preset: seriesPreset });
      const ids: number[] = [],
        groups: number[] = [],
        bestIds: number[] = [];
      result.groups.forEach((group, index) => {
        for (const id of group.ids) {
          ids.push(id);
          groups.push(index);
        }
        bestIds.push(group.best);
      });
      navigate({
        kind: "series",
        ids,
        groups,
        bestIds,
        seriesInfo: { count: result.groups.length, analyzed: result.analyzed, truncated: result.truncated },
        title: `Série: ${view.title}`,
      });
      const notices = [];
      if (!result.groups.length) notices.push("V tomto zobrazení som nenašiel žiadne série");
      if (result.truncated) notices.push("Analyzovaných bolo prvých 20 000 fotiek");
      if (notices.length) {
        setToast(notices.join(" · "));
        window.setTimeout(() => setToast(""), 5000);
      }
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 4000);
    } finally {
      setSeriesBusy(false);
    }
  };
  const toggleAi = async () => {
    const enabled = !aiEnabled;
    setAiEnabled(enabled);
    try {
      setAiStatus(
        await (enabled
          ? invoke<AiStatus>("ai_start", { lazyModel: !aiLoadModelAtStart })
          : invoke<AiStatus>("ai_stop")),
      );
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 3500);
    }
  };
  const updateAiStatus = async (command: "ai_pause" | "ai_resume" | "ai_reindex" | "ai_load_model") => {
    try {
      setAiStatus(await invoke<AiStatus>(command));
    } catch (error) {
      setToast(String(error));
      window.setTimeout(() => setToast(""), 3500);
    }
  };
  return { aiChipText, aiSearch, findSimilar, findSeries, toggleAi, updateAiStatus };
}

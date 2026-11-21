import { useEffect, useState, type Dispatch, type SetStateAction, type ToggleEvent } from "react";
import { readStored, writeStored } from "./storage.ts";
import type { Folder } from "./types";

type UiKey = `ddreams.ui.${string}`;

export function readUiState<T>(key: UiKey, fallback: T, legacyKey?: string): T {
  const value = readStored<unknown>(key, undefined)
    ?? (legacyKey ? readStored<unknown>(legacyKey, undefined) : undefined);
  return value != null && typeof value === typeof fallback && Array.isArray(value) === Array.isArray(fallback)
    ? value as T
    : fallback;
}

export function usePersistentState<T>(key: UiKey, fallback: T, legacyKey?: string) {
  const [value, setValue] = useState(() => readUiState(key, fallback, legacyKey));
  useEffect(() => writeStored(key, value), [key, value]);
  return [value, setValue] as const;
}

/** Same shape as usePersistentDetails, but not remembered: drop-down menus must start closed. */
export function useTransientDetails() {
  const [open, setOpen] = useState(false);
  return {
    open,
    onToggle: (event: ToggleEvent<HTMLDetailsElement>) => {
      if (event.target === event.currentTarget) setOpen(event.currentTarget.open);
    },
  };
}

export function usePersistentDetails(key: UiKey) {
  const [open, setOpen] = usePersistentState(key, false);
  return {
    open,
    onToggle: (event: ToggleEvent<HTMLDetailsElement>) => {
      if (event.target === event.currentTarget) setOpen(event.currentTarget.open);
    },
  };
}

// Keep the live tree intact. Only the most recently opened nodes are restored.
export function recentTreeKeys<T extends string | number>(keys: Iterable<T>, limit = 300): T[] {
  return [...new Set(keys)].slice(-limit);
}

export function usePersistentSet<T extends string | number>(
  key: UiKey, sample: T, limit = 300,
): [Set<T>, Dispatch<SetStateAction<Set<T>>>] {
  const [open, setOpen] = useState(() => new Set(recentTreeKeys(
    readUiState<T[]>(key, []).filter(item => typeof item === typeof sample), limit,
  )));
  useEffect(() => writeStored(key, recentTreeKeys(open, limit)), [key, open, limit]);
  return [open, setOpen];
}

export function toggleSetItem<T>(items: Set<T>, item: T): Set<T> {
  const next = new Set(items);
  if (!next.delete(item)) next.add(item);
  return next;
}

export function folderTreeKey(folder: Folder): string {
  if (folder.virtualNode) return `virtual:${folder.virtualNode}`;
  const path = folder.path?.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return path ? `path:${path}` : `id:${folder.id}`;
}

export function* expandedNodes<T>(
  nodes: readonly T[], isOpen: (node: T) => boolean, children: (node: T) => readonly T[],
): Generator<T> {
  for (const node of nodes) {
    if (!isOpen(node)) continue;
    yield node;
    yield* expandedNodes(children(node), isOpen, children);
  }
}

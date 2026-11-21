import assert from "node:assert/strict";
import { test } from "node:test";
import { expandedNodes, folderTreeKey, readUiState, recentTreeKeys } from "../src/uiState.ts";
import { writeStored } from "../src/storage.ts";

function mockStorage(t, getStorage) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { get localStorage() { return getStorage(); } },
  });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else delete globalThis.window;
  });
}

test("UI state survives a fresh read and falls back to legacy settings", t => {
  const stored = new Map();
  mockStorage(t, () => ({
    getItem: key => stored.get(key) ?? null,
    setItem: (key, value) => stored.set(key, value),
  }));
  const key = "ddreams.ui.catalog.sections";
  const legacy = "ddreams.catalogCollapsed";
  writeStored(legacy, { categories: true });
  assert.deepEqual(readUiState(key, {}, legacy), { categories: true });
  writeStored(key, { categories: false });
  assert.deepEqual(readUiState(key, {}, legacy), { categories: false });
  assert.deepEqual(readUiState(legacy, {}), { categories: true });
  stored.set(key, "{");
  assert.deepEqual(readUiState(key, {}, legacy), { categories: true });
  stored.set(key, "[]");
  assert.deepEqual(readUiState(key, {}), {});
});

test("disabled and full localStorage fail silently", t => {
  const key = "ddreams.ui.catalog.editSelection";
  let enabled = false;
  mockStorage(t, () => {
    if (!enabled) throw new Error("Storage blocked");
    return {
      getItem: () => "true",
      setItem: () => { throw new Error("Quota exceeded"); },
    };
  });
  assert.equal(readUiState(key, false), false);
  assert.doesNotThrow(() => writeStored(key, true));
  enabled = true;
  assert.equal(readUiState(key, false), true);
  assert.doesNotThrow(() => writeStored(key, false));
});

test("tree state caps recent nodes and restores only reachable branches", () => {
  const ids = Array.from({ length: 350 }, (_, id) => id);
  assert.deepEqual(recentTreeKeys(new Set(ids)), ids.slice(50));
  const root = { path: "F:/Photos/", id: -8 };
  assert.equal(folderTreeKey(root), folderTreeKey({ path: "f:\\photos", id: -47 }));
  const children = new Map([[1, [2, 3]], [3, [4]], [5, [6]]]);
  const open = new Set([1, 3, 4, 6]);
  assert.deepEqual([...expandedNodes([1, 5], id => open.has(id), id => children.get(id) ?? [])],
    [1, 3, 4]);
});

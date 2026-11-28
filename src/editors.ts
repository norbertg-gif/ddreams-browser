import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { readStored } from "./storage";

export type Editor = import("./bindings/Editor").Editor;
const KEY = "ddreams.editors";
const EVENT = "ddreams-editors-changed";
let editors: Editor[] = [];
let suggested = false;
let initialized: Promise<void> | null = null;
let pending = Promise.resolve();
let revision = 0;
const notify = () => window.dispatchEvent(new Event(EVENT));
function initialize() {
  if (!initialized)
    initialized = (async () => {
      const startRevision = revision;
      let disk = await invoke<Editor[]>("get_editors");
      const legacy = readStored<Editor[]>(KEY, []);
      if (!disk.length && Array.isArray(legacy) && legacy.length) {
        await invoke("set_editors", { editors: legacy });
        disk = legacy;
      }
      try {
        window.localStorage.removeItem(KEY);
      } catch {
        /* storage may be unavailable */
      }
      if (!disk.length) {
        const detected = await invoke<Editor | null>("detect_photoshop");
        if (detected) {
          disk = [detected];
          if (revision === startRevision) suggested = true;
        }
      }
      if (revision === startRevision) {
        editors = disk;
        notify();
      }
    })().catch((error) => {
      initialized = null;
      throw error;
    });
  return initialized;
}
export function saveEditors(next: Editor[]) {
  revision++;
  editors = next;
  suggested = false;
  notify();
  pending = pending
    .then(async () => {
      await initialize();
      await invoke("set_editors", { editors: next });
    })
    .catch((error) => {
      window.alert(`Nastavenia editorov sa nepodarilo uložiť: ${String(error)}`);
    });
}
export async function launchEditor(editor: Editor, assetIds: number[]) {
  await pending;
  await invoke("open_in_editor", { assetIds, exe: editor.path });
}

export function useEditors(): Editor[] {
  const [value, setValue] = useState(editors);
  useEffect(() => {
    const refresh = () => setValue(editors);
    window.addEventListener(EVENT, refresh);
    void initialize().catch((error) => console.error("Editor settings failed to load", error));
    return () => window.removeEventListener(EVENT, refresh);
  }, []);
  return value;
}
export function editorsSuggested() {
  return suggested;
}

import { useEffect, useState } from "react";
import { readStored, writeStored } from "./storage";

/** External program for opening photos. The first one in the list is the default. */
export type Editor = { name: string; path: string; multi: boolean };

const KEY = "ddreams.editors";
const EVENT = "ddreams-editors-changed";

export function loadEditors(): Editor[] {
  const value = readStored<Editor[]>(KEY, []);
  return Array.isArray(value) ? value.filter(e => e && typeof e.path === "string" && e.path) : [];
}

export function saveEditors(editors: Editor[]) {
  writeStored(KEY, editors);
  window.dispatchEvent(new Event(EVENT));
}

/** Live list of configured editors; updates when Settings change it. */
export function useEditors(): Editor[] {
  const [editors, setEditors] = useState(loadEditors);
  useEffect(() => {
    const refresh = () => setEditors(loadEditors());
    window.addEventListener(EVENT, refresh);
    return () => window.removeEventListener(EVENT, refresh);
  }, []);
  return editors;
}

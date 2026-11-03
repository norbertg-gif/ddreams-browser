export type ThumbnailField = { id: string; label: string; group: string };
export type ThumbnailFieldChoice = { id: string; lines: 1 | 2 | 3 };
export type ThumbnailInfoConfig = { fields: ThumbnailFieldChoice[] };

export const THUMBNAIL_FIELD_GROUPS: { name: string; fields: ThumbnailField[] }[] = [
  { name: "Súbor", fields: [
    { id: "name", label: "Názov s príponou", group: "Súbor" },
    { id: "baseName", label: "Názov bez prípony", group: "Súbor" },
    { id: "type", label: "Typ", group: "Súbor" },
    { id: "sizeKb", label: "Veľkosť (KB)", group: "Súbor" },
    { id: "sizeAuto", label: "Veľkosť (auto, ako dnes)", group: "Súbor" },
    { id: "folder", label: "Priečinok", group: "Súbor" },
    { id: "modified", label: "Dátum zmeny", group: "Súbor" },
  ] },
  { name: "Obrázok", fields: [
    { id: "resolution", label: "Rozlíšenie", group: "Obrázok" },
  ] },
  { name: "Katalóg", fields: [
    { id: "dateTaken", label: "Dátum snímania", group: "Katalóg" },
    { id: "rating", label: "Hodnotenie", group: "Katalóg" },
    { id: "caption", label: "Popis", group: "Katalóg" },
    { id: "author", label: "Autor", group: "Katalóg" },
    { id: "categories", label: "Kategórie", group: "Katalóg" },
    { id: "keywords", label: "Kľúčové slová", group: "Katalóg" },
  ] },
  { name: "EXIF", fields: [
    { id: "iso", label: "ISO", group: "EXIF" },
    { id: "aperture", label: "Clona", group: "EXIF" },
    { id: "shutter", label: "Čas uzávierky", group: "EXIF" },
    { id: "focalLength", label: "Ohnisková vzdialenosť", group: "EXIF" },
    { id: "lens", label: "Objektív", group: "EXIF" },
    { id: "camera", label: "Fotoaparát", group: "EXIF" },
  ] },
];

export const DEFAULT_THUMBNAIL_INFO: ThumbnailInfoConfig = { fields: [{ id: "name", lines: 1 }] };
export const THUMBNAIL_FIELD_BY_ID = new Map(THUMBNAIL_FIELD_GROUPS.flatMap(group => group.fields.map(field => [field.id, field] as const)));
export const EXIF_THUMBNAIL_FIELDS = new Set(["iso", "aperture", "shutter", "focalLength", "lens", "camera", "dateTaken"]);

export function normalizeThumbnailInfo(value: unknown): ThumbnailInfoConfig {
  if (!value || typeof value !== "object" || !Array.isArray((value as ThumbnailInfoConfig).fields)) return DEFAULT_THUMBNAIL_INFO;
  const seen = new Set<string>();
  const fields: ThumbnailFieldChoice[] = [];
  for (const raw of (value as ThumbnailInfoConfig).fields) {
    if (!raw || !THUMBNAIL_FIELD_BY_ID.has(raw.id) || seen.has(raw.id)) continue;
    seen.add(raw.id);
    fields.push({ id: raw.id, lines: raw.lines === 2 || raw.lines === 3 ? raw.lines : 1 });
  }
  return { fields };
}

export function formatKilobytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "";
  const kb = Math.ceil(bytes / 1024);
  return `${String(kb).replace(/\B(?=(\d{3})+(?!\d))/g, " ")} KB`;
}

export function formatSlovakDate(value: string | null | undefined): string {
  if (!value) return "";
  const match = value.match(/^(\d{4})[-:](\d{2})[-:](\d{2})[ T](\d{2}:\d{2}:\d{2})(.*)$/);
  if (match) return `${Number(match[3])}. ${Number(match[2])}. ${match[1]} ${match[4]}${match[5]}`;
  return value;
}

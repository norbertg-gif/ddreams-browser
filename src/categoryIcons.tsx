const categoryIcons = [
  ["album", "Album", "M6 3h13v18H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2ZM8 3v18m3-7 2-3 2 2 2-3m-5-3h.01"],
  ["people", "Ľudia", "M15 21v-2a4 4 0 0 0-4-4H7a4 4 0 0 0-4 4v2m18 0v-2a4 4 0 0 0-3-4m-3-12a4 4 0 0 1 0 8M9 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z"],
  ["places", "Miesta", "M20 10c0 5-8 11-8 11S4 15 4 10a8 8 0 1 1 16 0Zm-8-3a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z"],
  ["various", "Rôzne", "M3 3h7v7H3zm11 0h7v7h-7zM3 14h7v7H3zm14.5 0 3.5 3.5-3.5 3.5-3.5-3.5z"],
  ["event", "Udalosti", "M3 5h18v16H3zm5-3v6m8-6v6M3 10h18m-9 3v5m-2.5-2.5h5"],
  ["travel", "Cestovanie", "m22 2-7 20-4-9-9-4 20-7ZM11 13 22 2"],
  ["nature", "Príroda", "M20 4c-8 0-14 3-14 9a5 5 0 0 0 5 5c6 0 9-6 9-14ZM4 21l10-10"],
  ["animal", "Zvieratá", "M7 17c0-2 3-5 5-5s5 3 5 5c0 3-3 3-5 2-2 1-5 1-5-2ZM4 8a1.5 2 0 1 0 0 4 1.5 2 0 0 0 0-4Zm5-4a1.5 2 0 1 0 0 4 1.5 2 0 0 0 0-4Zm6 0a1.5 2 0 1 0 0 4 1.5 2 0 0 0 0-4Zm5 4a1.5 2 0 1 0 0 4 1.5 2 0 0 0 0-4Z"],
  ["home", "Domov", "m2 11 10-8 10 8M5 10v11h14V10m-9 11v-7h4v7"],
  ["building", "Budovy", "M5 21V3h10v18m0-12h4v12M3 21h18M9 7h2m-2 4h2m-2 4h2"],
  ["car", "Autá", "m4 10 2-5h12l2 5M3 17v-5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v5H3Zm2 0v3m14-3v3M6 14h2m8 0h2"],
  ["food", "Jedlo", "M4 3v6a3 3 0 0 0 6 0V3M7 3v18m12 0V3c-3 2-4 5-4 9h4"],
  ["sport", "Šport", "M8 3h8v7a4 4 0 0 1-8 0V3Zm0 2H4v2a4 4 0 0 0 4 4m8-6h4v2a4 4 0 0 1-4 4m-4 3v5m-4 2h8"],
  ["music", "Hudba", "M9 18V5l11-2v13M9 9l11-2M6 16c-2 0-3 1-3 2s1 2 3 2 3-1 3-2-1-2-3-2Zm11-2c-2 0-3 1-3 2s1 2 3 2 3-1 3-2-1-2-3-2Z"],
  ["work", "Práca", "M3 7h18v13H3zm5 0V4h8v3M3 12h18m-9-1v3"],
  ["school", "Škola", "m2 9 10-5 10 5-10 5-10-5Zm4 2v6c4 3 8 3 12 0v-6m4-2v7"],
  ["star", "Hviezda", "m12 2 3 6 7 1-5 5 1 7-6-3-6 3 1-7-5-5 7-1 3-6Z"],
  ["heart", "Srdce", "M12 21 4 13a5 5 0 0 1 8-6 5 5 0 0 1 8 6l-8 8Z"],
  ["camera", "Fotoaparát", "M3 7h4l2-3h6l2 3h4v13H3zm9 3a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z"],
  ["folder", "Priečinok", "M3 5h7l2 3h9v12H3z"],
] as const;

function IconSvg({ path }: { path: string }) {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d={path} />
  </svg>;
}

export function CategoryIcon({ iconKey, root = false }: { iconKey?: string | null; root?: boolean }) {
  const icon = categoryIcons.find(([key]) => key === iconKey);
  return <span className="category-icon" aria-hidden="true">
    {icon ? <IconSvg path={icon[2]} /> : root ? "◈" : "▤"}
  </span>;
}

export function CategoryIconPicker({ value, onChange, disabled }: {
  value: string | null;
  onChange: (value: string | null) => void;
  disabled: boolean;
}) {
  return <fieldset className="category-icon-picker" disabled={disabled}>
    <legend>Ikona</legend>
    <div className="category-icon-grid">
      {categoryIcons.map(([key, label, path]) => <button key={key} type="button"
        title={label} aria-label={label} aria-pressed={value === key} onClick={() => onChange(key)}>
        <IconSvg path={path} />
      </button>)}
    </div>
    <button type="button" aria-pressed={value == null} onClick={() => onChange(null)}>
      Zdediť od nadradenej
    </button>
  </fieldset>;
}

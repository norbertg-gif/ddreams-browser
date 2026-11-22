import { useState } from "react";
import { formatBytes } from "./AssetGrid";

export type QuarantineSettings = {
  folder: string;
  incomingFolder: string;
  quarantineFolder: string;
  moveAfterDays: number;
  deleteAfterDays: number;
  editAfterDays: number;
  manageFrom: string;
};

type ReviewList = { ids: number[]; bytes: number };
export type Review = { moves: ReviewList; deletes: ReviewList; staleEdits: ReviewList; editAfterDays: number; noBinDrives: string[] };
type ReviewSelection = Record<"moves" | "deletes" | "staleEdits", boolean>;
export type QuarantineOutcome = {
  moved: number;
  restored: number;
  deleted: number;
  failures: string[];
  affectedFolderPaths: string[];
};

export default function QuarantineReview({ review, onShow, onClose, onExecute, busy, progress, hidden }: {
  review: Review;
  onShow: (ids: number[], title: string) => void;
  onClose: () => void;
  onExecute: (selection: ReviewSelection, confirmedDrives: string[]) => void;
  busy: boolean;
  progress: string;
  hidden: boolean;
}) {
  const [confirmed, setConfirmed] = useState(false);
  const [selection, setSelection] = useState<ReviewSelection>({ moves: true, deletes: true, staleEdits: true });
  if (hidden) return null;
  const rows = [
    ["moves", "Na presun do karantény"],
    ["staleEdits", `Na úpravu dlhšie ako ${review.editAfterDays} ${review.editAfterDays === 1 ? "deň" : "dní"}`],
    ["deletes", "Na odstránenie z karantény"],
  ] as const;
  const selected = rows.filter(([key]) => selection[key] && review[key].ids.length);
  const needsConfirmation = selection.deletes && review.noBinDrives.length > 0;
  return <div className="modal-backdrop">
    <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="quarantine-review-title"
      onKeyDown={event => { if (event.key === "Escape" && !busy) { event.stopPropagation(); onClose(); } }}>
      <b id="quarantine-review-title">Kontrola importu a karantény</b>
      {rows.map(([key, title]) => {
        const list = review[key];
        if (key === "staleEdits" && !review.editAfterDays) return null;
        return <p key={key}><label>
          <input type="checkbox" checked={selection[key] && list.ids.length > 0} disabled={busy || !list.ids.length}
            onChange={event => setSelection({ ...selection, [key]: event.target.checked })} />
          {title} · {list.ids.length.toLocaleString("sk-SK")} · {formatBytes(list.bytes)}
        </label>{" "}<button className="quarantine-review-link" disabled={busy || !list.ids.length} onClick={() => onShow(list.ids, title)}>Zobraziť</button>
        </p>;
      })}
      <p>Spolu {formatBytes(selected.reduce((bytes, [key]) => bytes + review[key].bytes, 0))} vrátane XMP. Pri kolízii názvov zostanú oba súbory.</p>
      {needsConfirmation && <label className="delete-warning">
        <input type="checkbox" checked={confirmed} disabled={busy} onChange={event => setConfirmed(event.target.checked)} />
        Súhlasím s trvalým zmazaním uvedených súborov z karantény na diskoch bez Koša ({review.noBinDrives.join(", ")}).
      </label>}
      {busy && <p role="status">{progress || "Spracúvam karanténu…"}</p>}
      <div className="confirm-actions">
        <button autoFocus disabled={busy} onClick={onClose}>Teraz nie</button>
        <button disabled={busy || !selected.length || (needsConfirmation && !confirmed)}
          onClick={() => onExecute(selection, needsConfirmation ? review.noBinDrives : [])}>Vykonať</button>
      </div>
    </div>
  </div>;
}

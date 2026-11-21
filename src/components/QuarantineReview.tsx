import { useState } from "react";
import { formatBytes } from "./AssetGrid";

export type QuarantineSettings = {
  folder: string;
  incomingFolder: string;
  quarantineFolder: string;
  moveAfterDays: number;
  deleteAfterDays: number;
  manageFrom: string;
};

type ReviewList = { ids: number[]; bytes: number };
export type Review = { moves: ReviewList; deletes: ReviewList; noBinDrives: string[] };
export type QuarantineOutcome = {
  moved: number;
  restored: number;
  deleted: number;
  failures: string[];
  affectedFolderPaths: string[];
};

export default function QuarantineReview({ review, onShow, onClose, onExecute, busy, progress }: {
  review: Review;
  onShow: (ids: number[], title: string) => void;
  onClose: () => void;
  onExecute: (confirmedDrives: string[]) => void;
  busy: boolean;
  progress: string;
}) {
  const [confirmed, setConfirmed] = useState(false);
  const needsConfirmation = review.noBinDrives.length > 0;
  return <div className="modal-backdrop">
    <div className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="quarantine-review-title"
      onKeyDown={event => { if (event.key === "Escape" && !busy) { event.stopPropagation(); onClose(); } }}>
      <b id="quarantine-review-title">Kontrola importu a karantény</b>
      {([[review.moves, "Na presun do karantény"], [review.deletes, "Na odstránenie z karantény"]] as const)
        .map(([list, title]) => <p key={title}>{title} · {list.ids.length.toLocaleString("sk-SK")} · {formatBytes(list.bytes)}
          {" "}<button className="quarantine-review-link" disabled={busy || !list.ids.length} onClick={() => onShow(list.ids, title)}>Zobraziť</button>
        </p>)}
      <p>Spolu {formatBytes(review.moves.bytes + review.deletes.bytes)} vrátane XMP. Pri kolízii názvov zostanú oba súbory.</p>
      {needsConfirmation && <label className="delete-warning">
        <input type="checkbox" checked={confirmed} disabled={busy} onChange={event => setConfirmed(event.target.checked)} />
        Súhlasím s trvalým zmazaním uvedených súborov z karantény na diskoch bez Koša ({review.noBinDrives.join(", ")}).
      </label>}
      {busy && <p role="status">{progress || "Spracúvam karanténu…"}</p>}
      <div className="confirm-actions">
        <button autoFocus disabled={busy} onClick={onClose}>Teraz nie</button>
        <button disabled={busy || (needsConfirmation && !confirmed)} onClick={() => onExecute(review.noBinDrives)}>Vykonať</button>
      </div>
    </div>
  </div>;
}

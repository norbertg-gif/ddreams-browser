import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { IptcIssue, IptcSummary } from "../types";

type Plan = import("../bindings/PlanSummary").PlanSummary;

export function IptcIssues({
  title,
  items,
  defaultOpen = false,
}: {
  title: string;
  items: IptcIssue[];
  defaultOpen?: boolean;
}) {
  if (!items.length) return null;
  return (
    <details className="iptc-issues" open={defaultOpen || undefined}>
      <summary>
        {title} ({items.length})
      </summary>
      <ul>
        {items.map(({ path, message }, index) => (
          <li key={index}>
            <b>{path}</b>
            <br />
            {message}
          </li>
        ))}
      </ul>
    </details>
  );
}

export default function IptcWriteDialog({
  ids,
  progress,
  onClose,
  onDone,
}: {
  ids: number[];
  progress: string;
  onClose: () => void;
  onDone: (result: IptcSummary) => void;
}) {
  const [plan, setPlan] = useState<Plan>();
  const [result, setResult] = useState<IptcSummary>();
  const [error, setError] = useState("");
  const [running, setRunning] = useState(false);
  const cancelRequested = useRef(false);
  const [cancelling, setCancelling] = useState(false);
  const busy = useRef(false);

  useEffect(() => {
    let current = true;
    let planId: number | undefined;
    void invoke<Plan>("plan_iptc_write", { assetIds: ids })
      .then((value) => {
        planId = value.id;
        if (current) setPlan(value);
        else void invoke("cancel_iptc_write", { planId }).catch(() => {});
      })
      .catch((reason) => {
        if (current) setError(String(reason));
      });
    return () => {
      current = false;
      if (planId !== undefined) void invoke("cancel_iptc_write", { planId }).catch(() => {});
    };
  }, [ids]);

  const start = async () => {
    if (!plan || busy.current) return;
    busy.current = true;
    setRunning(true);
    try {
      const value = await invoke<IptcSummary>("execute_iptc_write", { planId: plan.id });
      setResult(value);
      onDone(value);
    } catch (reason) {
      setError(String(reason));
    } finally {
      busy.current = false;
      setRunning(false);
    }
  };

  const cancel = () => {
    if (!plan || !busy.current) {
      onClose();
      return;
    }
    if (cancelRequested.current) return;
    cancelRequested.current = true;
    setCancelling(true);
    void invoke("cancel_iptc_write", { planId: plan.id }).catch((reason) => {
      cancelRequested.current = false;
      setCancelling(false);
      setError(String(reason));
    });
  };

  return (
    <div className="modal-backdrop">
      <section
        className="confirm-dialog iptc-write-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Zapísať IPTC do súborov"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            cancel();
          }
        }}
      >
        <b>Zapísať IPTC do súborov?</b>
        {!plan && !error && <p role="status">Kontrolujem súbory a metadáta…</p>}
        {plan && !result && (
          <>
            <p>
              Súbory: {plan.files} · vložené metadáta: {plan.embedded} · XMP sidecar: {plan.sidecar} · bez zmeny:{" "}
              {plan.unchanged}
            </p>
            {!!plan.sidecar && (
              <p>
                Nový sidecar: <code>&lt;názov&gt;.xmp</code>. Použije sa aj existujúci{" "}
                <code>&lt;názov&gt;.&lt;prípona&gt;.xmp</code>.
              </p>
            )}
            <p>
              Doplní sa popis: {plan.description} · autor: {plan.creator} · nové kľúčové slová: {plan.keywords}
            </p>
            <p className="iptc-write-warning">
              Zapisuje sa priamo do súborov, bez zálohy. Existujúce hodnoty v súboroch sa neprepíšu, len doplnia.
            </p>
            <IptcIssues title="Chyby pri kontrole" items={plan.errors} defaultOpen />
            <IptcIssues title="Upozornenia" items={plan.warnings} />
          </>
        )}
        {running && (
          <p role="status">{cancelling ? "Zastavím po dokončení aktuálneho súboru…" : progress || "Zapisujem IPTC…"}</p>
        )}
        {result && (
          <>
            <p role="status">
              Zapísané: {result.written}, bez zmeny: {result.unchanged}, chyby: {result.errors.length}
            </p>
            {!!result.cancelled && <p>Zrušené pred zápisom: {result.cancelled}</p>}
            <IptcIssues title="Chyby" items={result.errors} defaultOpen />
            <IptcIssues title="Upozornenia" items={result.warnings} />
          </>
        )}
        {error && <p role="alert">{error}</p>}
        <div className="confirm-actions">
          <button autoFocus onClick={cancel} disabled={running && cancelling}>
            {result ? "Zavrieť" : "Zrušiť"}
          </button>
          {plan && !result && (
            <button
              className="danger"
              disabled={running || !!error || plan.files <= plan.unchanged + plan.errors.length}
              onClick={() => void start()}
            >
              Zapísať
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

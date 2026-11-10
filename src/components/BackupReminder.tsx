import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type BackupStatus = { lastBackupAt: number | null; backupsDir: string; due: boolean };

export default function BackupReminder() {
  const [status, setStatus] = useState<BackupStatus | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const close = async (days: number) => {
    try { await invoke("snooze_backup_reminder", { days }); setStatus(null); }
    catch (e) { setError(String(e)); }
  };
  useEffect(() => {
    const timer = window.setTimeout(() => { void invoke<BackupStatus>("get_backup_status").then(value => { if (value.due) setStatus(value); }).catch(e => setError(String(e))); }, 3000);
    return () => window.clearTimeout(timer);
  }, []);
  useEffect(() => {
    if (!status) return;
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); void close(1); } };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [status]);
  const backupNow = async () => {
    setBusy(true); setError("");
    try { await invoke("backup_catalog_now"); setDone(true); window.setTimeout(() => setStatus(null), 1000); }
    catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  if (!status) return error ? <div className="backup-toast" role="status">Zálohu katalógu sa nepodarilo načítať: {error}</div> : null;
  return <div className="modal-backdrop" onPointerDown={e => { if (e.target === e.currentTarget) void close(1); }}>
    <div className="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="backup-reminder-title">
      <h3 id="backup-reminder-title">Zálohovať katalóg?</h3>
      <p>Posledná záloha: {status.lastBackupAt ? new Date(status.lastBackupAt * 1000).toLocaleDateString("sk-SK") : "zatiaľ žiadna"}. Záloha uloží kópiu katalógu (kategórie, kľúčové slová, hodnotenia) do <span className="confirm-path">{status.backupsDir}</span>.</p>
      {error && <small className="settings-error">{error}</small>}
      <div className="confirm-actions">
        <button className="backup-primary" disabled={busy || done} onClick={() => void backupNow()}>{busy ? "Zálohujem…" : done ? "Záloha hotová" : "Zálohovať teraz"}</button>
        <button disabled={busy} onClick={() => void close(7)}>Pripomenúť o týždeň</button>
        <button disabled={busy} onClick={() => void close(1)}>Teraz nie</button>
      </div>
    </div>
  </div>;
}

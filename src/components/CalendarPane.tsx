import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { readStored, writeStored } from "../storage";
import type { View } from "../types";

type Mode = "events" | "year" | "month" | "day";
type Source = "taken" | "database" | "modified";
type Hour = { hour: number; count: number };
type Day = { day: number; date: string; count: number; hours: Hour[] };
type Month = { month: number; count: number; days: Day[] };
type Year = { year: number; count: number; months: Month[] };
type CalendarData = { dateSource: Source; datedCount: number; undatedCount: number; years: Year[] };

const MODE_KEY = "ddreams.calendar.mode";
const SOURCE_KEY = "ddreams.calendar.source";
const EXPANDED_KEY = "ddreams.calendar.expanded";
const dateAt = (year: number, month: number, day = 1) => new Date(Date.UTC(year, month - 1, day));
const isoDate = (date: Date) => `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, "0")}-${String(date.getUTCDate()).padStart(2, "0")}`;
const monthName = (year: number, month: number) => dateAt(year, month).toLocaleDateString("sk-SK", { month: "long", timeZone: "UTC" });
const dayTitle = (day: Day) => `${day.day}. ${Number(day.date.slice(5, 7))}. (${dateAt(Number(day.date.slice(0, 4)), Number(day.date.slice(5, 7)), day.day).toLocaleDateString("sk-SK", { weekday: "short", timeZone: "UTC" })})`;
const countText = (count: number) => count.toLocaleString("sk-SK");
const readMode = (): Mode => { const value = readStored<Mode>(MODE_KEY, "month"); return ["events", "year", "month", "day"].includes(value) ? value : "month"; };
const readSource = (): Source => { const value = readStored<Source>(SOURCE_KEY, "taken"); return ["taken", "database", "modified"].includes(value) ? value : "taken"; };

export default function CalendarPane({ onNavigate, activeView, refreshToken = 0 }: { onNavigate: (view: View) => void; activeView?: View; refreshToken?: number }) {
  const [mode, setMode] = useState<Mode>(readMode);
  const [source, setSource] = useState<Source>(readSource);
  const [expanded, setExpanded] = useState<Record<string, boolean>>(() => readStored(EXPANDED_KEY, {}));
  const [cursor, setCursor] = useState(() => new Date());
  const [data, setData] = useState<CalendarData | null>(null);
  const [error, setError] = useState("");
  const request = useRef(0);

  useEffect(() => writeStored(MODE_KEY, mode), [mode]);
  useEffect(() => writeStored(SOURCE_KEY, source), [source]);
  useEffect(() => writeStored(EXPANDED_KEY, expanded), [expanded]);
  useEffect(() => {
    const id = ++request.current;
    setError("");
    void invoke<CalendarData>("get_calendar_aggregate", { source }).then(value => { if (id === request.current) setData(value); }).catch(reason => { if (id === request.current) setError(String(reason)); });
    return () => { request.current++; };
  }, [source, refreshToken]);
  useEffect(() => {
    if (activeView?.kind !== "date" || !activeView.dateFrom) return;
    const [year, month, day] = activeView.dateFrom.split("-").map(Number);
    if (year && month) setCursor(dateAt(year, month, day || 1));
  }, [activeView?.kind, activeView?.dateFrom]);

  const cursorPlaced = useRef(false);
  useEffect(() => {
    if (!data || cursorPlaced.current) return;
    cursorPlaced.current = true;
    if (activeView?.kind === "date" && activeView.dateFrom) return;
    // Start on the newest month that has photos instead of an empty current month.
    const newest = [...data.years].sort((a, b) => b.year - a.year).flatMap(year => [...year.months].sort((a, b) => b.month - a.month).filter(item => item.count > 0).map(item => ({ year: year.year, month: item.month })))[0];
    if (newest) setCursor(dateAt(newest.year, newest.month, 1));
  }, [data]);

  const years = useMemo(() => [...(data?.years ?? [])].sort((a, b) => b.year - a.year), [data]);
  const selected = (view: View) => activeView?.kind === "date" && activeView.dateSource === source && activeView.dateFrom === view.dateFrom && activeView.dateTo === view.dateTo && !!activeView.dateNoDate === !!view.dateNoDate;
  const selectedYear = (year: number) => selected({ kind: "date", dateFrom: `${year}-01-01`, dateTo: `${year}-12-31`, dateSource: source, title: "" });
  const openDay = (day: Day) => onNavigate({ kind: "date", dateFrom: day.date, dateTo: day.date, dateSource: source, title: `Kalendár: ${day.day}. ${Number(day.date.slice(5, 7))}. ${day.date.slice(0, 4)}` });
  const openMonth = (year: number, month: number) => onNavigate({ kind: "date", dateFrom: isoDate(dateAt(year, month)), dateTo: isoDate(dateAt(year, month + 1, 0)), dateSource: source, title: `Kalendár: ${monthName(year, month)} ${year}` });
  const openYear = (year: number) => onNavigate({ kind: "date", dateFrom: `${year}-01-01`, dateTo: `${year}-12-31`, dateSource: source, title: `Kalendár: ${year}` });
  const openUndated = () => onNavigate({ kind: "date", dateSource: source, dateNoDate: true, title: "Kalendár: Bez dátumu" });
  const month = data?.years.find(year => year.year === cursor.getUTCFullYear())?.months.find(item => item.month === cursor.getUTCMonth() + 1);
  const day = data?.years.find(year => year.year === cursor.getUTCFullYear())?.months.find(item => item.month === cursor.getUTCMonth() + 1)?.days.find(item => item.day === cursor.getUTCDate());

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!event.shiftKey || !["ArrowLeft", "ArrowRight"].includes(event.key)) return;
    const target = event.target as HTMLElement;
    if (["INPUT", "SELECT", "TEXTAREA"].includes(target.tagName)) return;
    event.preventDefault();
    const direction = event.key === "ArrowLeft" ? -1 : 1;
    if (mode === "month" || mode === "day") {
      const all = (data?.years ?? []).flatMap(year => year.months.flatMap(value => value.days)).sort((a, b) => a.date.localeCompare(b.date));
      const current = activeView?.kind === "date" ? activeView.dateFrom ?? isoDate(cursor) : isoDate(cursor);
      const targetDay = direction < 0 ? [...all].reverse().find(value => value.date < current) : all.find(value => value.date > current);
      if (targetDay) openDay(targetDay);
    } else {
      const all = (data?.years ?? []).flatMap(year => year.months.map(value => ({ year: year.year, month: value.month }))).sort((a, b) => a.year - b.year || a.month - b.month);
      const current = activeView?.kind === "date" && activeView.dateFrom ? activeView.dateFrom.slice(0, 7) : `${cursor.getUTCFullYear()}-${String(cursor.getUTCMonth() + 1).padStart(2, "0")}`;
      const targetMonth = direction < 0 ? [...all].reverse().find(value => `${value.year}-${String(value.month).padStart(2, "0")}` < current) : all.find(value => `${value.year}-${String(value.month).padStart(2, "0")}` > current);
      if (targetMonth) openMonth(targetMonth.year, targetMonth.month);
    }
  };
  const toggleExpanded = (key: string, value: boolean) => setExpanded(current => ({ ...current, [key]: value }));

  return <div className="calendar-pane" tabIndex={0} onKeyDown={onKeyDown}>
    <div className="calendar-controls">
      <select aria-label="Pohľad kalendára" value={mode} onChange={event => setMode(event.target.value as Mode)}><option value="events">Udalosti</option><option value="year">Rok</option><option value="month">Mesiac</option><option value="day">Deň</option></select>
      <select aria-label="Zdroj dátumu" value={source} onChange={event => setSource(event.target.value as Source)}><option value="taken">Dátum snímania</option><option value="database">Dátum v databáze</option><option value="modified">Dátum zmeny</option></select>
    </div>
    <button className={`calendar-undated ${activeView?.kind === "date" && activeView.dateNoDate && activeView.dateSource === source ? "selected" : ""}`} disabled={!data?.undatedCount} onClick={openUndated}><span>Bez dátumu</span><small>{countText(data?.undatedCount ?? 0)}</small></button>
    {error && <div className="inline-error">Kalendár sa nepodarilo načítať<button onClick={() => setError("")}>×</button></div>}
    {!data && !error && <div className="calendar-loading">Načítavam…</div>}
    {data && mode === "events" && <div className="calendar-content">
      {years.map(year => <details key={year.year} open={expanded[`year:${source}:${year.year}`] ?? false} onToggle={event => toggleExpanded(`year:${source}:${year.year}`, event.currentTarget.open)}>
        <summary className={`calendar-year-summary ${selectedYear(year.year) ? "selected" : ""}`}><span>{year.year}</span><small>{countText(year.count)}</small></summary>
        <div className="calendar-event-months">{[...year.months].sort((a, b) => b.month - a.month).map(item => {
          const key = `month:${source}:${year.year}:${item.month}`;
          const selectedMonth = activeView?.kind === "date" && activeView.dateSource === source && activeView.dateFrom === isoDate(dateAt(year.year, item.month)) && activeView.dateTo === isoDate(dateAt(year.year, item.month + 1, 0));
          return <details key={item.month} open={expanded[key] ?? false} onToggle={event => toggleExpanded(key, event.currentTarget.open)}>
            <summary className={`calendar-month-summary ${selectedMonth ? "selected" : ""}`}><span>{monthName(year.year, item.month)} {year.year} · {item.days.length} {item.days.length === 1 ? "deň" : item.days.length < 5 ? "dni" : "dní"} · {countText(item.count)}</span><button onClick={event => { event.preventDefault(); event.stopPropagation(); openMonth(year.year, item.month); }}>Otvoriť</button></summary>
            {item.days.slice().reverse().map(value => <button key={value.date} className={`calendar-event-day ${selected({ kind: "date", dateFrom: value.date, dateTo: value.date, dateSource: source, title: "" }) ? "selected" : ""}`} onClick={() => openDay(value)}>{dayTitle(value)} · {countText(value.count)}</button>)}
          </details>;
        })}</div>
      </details>)}
    </div>}
    {data && mode === "year" && <div className="calendar-content calendar-years">{years.map(value => <section key={value.year} className="calendar-year-card"><button className={`calendar-year-heading ${selectedYear(value.year) ? "selected" : ""}`} onClick={() => openYear(value.year)}><b>{value.year}</b><small>{countText(value.count)}</small></button><div className="calendar-mini-months">{Array.from({ length: 12 }, (_, index) => { const number = index + 1; const item = value.months.find(entry => entry.month === number); const selectedMonth = selected({ kind: "date", dateFrom: isoDate(dateAt(value.year, number)), dateTo: isoDate(dateAt(value.year, number + 1, 0)), dateSource: source, title: "" }); return <button key={number} disabled={!item} className={`calendar-mini-month ${item ? "has-count" : ""} ${selectedMonth ? "selected" : ""}`} onClick={() => item && openMonth(value.year, number)} title={item ? `${monthName(value.year, number)}: ${countText(item.count)}` : monthName(value.year, number)}><span>{monthName(value.year, number).slice(0, 3)}</span>{item && <small>{countText(item.count)}</small>}</button>; })}</div></section>)}</div>}
    {data && mode === "month" && <div className="calendar-content calendar-month-view"><div className="calendar-month-heading"><button aria-label="Predchádzajúci mesiac" onClick={() => setCursor(dateAt(cursor.getUTCFullYear(), cursor.getUTCMonth(), 1))}>‹</button><button className={`calendar-open-heading ${month && selected({ kind: "date", dateFrom: isoDate(dateAt(cursor.getUTCFullYear(), cursor.getUTCMonth() + 1)), dateTo: isoDate(dateAt(cursor.getUTCFullYear(), cursor.getUTCMonth() + 2, 0)), dateSource: source, title: "" }) ? "selected" : ""}`} onClick={() => openMonth(cursor.getUTCFullYear(), cursor.getUTCMonth() + 1)}>{monthName(cursor.getUTCFullYear(), cursor.getUTCMonth() + 1)} {cursor.getUTCFullYear()} · {countText(month?.count ?? 0)}</button><button aria-label="Nasledujúci mesiac" onClick={() => setCursor(dateAt(cursor.getUTCFullYear(), cursor.getUTCMonth() + 2, 1))}>›</button></div><div className="calendar-weekdays">{["Po", "Ut", "St", "Št", "Pi", "So", "Ne"].map(value => <span key={value}>{value}</span>)}</div><div className="calendar-day-grid">{(() => { const year = cursor.getUTCFullYear(), number = cursor.getUTCMonth() + 1, firstOffset = (dateAt(year, number).getUTCDay() + 6) % 7, dayCount = dateAt(year, number + 1, 0).getUTCDate(), cells = Math.ceil((firstOffset + dayCount) / 7) * 7; return Array.from({ length: cells }, (_, index) => { const value = index - firstOffset + 1; if (value < 1 || value > dayCount) return <span key={index} />; const item = month?.days.find(entry => entry.day === value); const date = `${year}-${String(number).padStart(2, "0")}-${String(value).padStart(2, "0")}`; const active = activeView?.kind === "date" && activeView.dateSource === source && activeView.dateFrom === date && activeView.dateTo === date; return <button key={date} disabled={!item} className={`${item ? "has-photos" : ""} ${active ? "selected" : ""}`} onClick={() => item && openDay(item)}><span>{value}</span>{item && <small>{countText(item.count)}</small>}</button>; }); })()}</div></div>}
    {data && mode === "day" && <div className="calendar-content calendar-day-view"><div className="calendar-month-heading"><button aria-label="Predchádzajúci deň" onClick={() => setCursor(new Date(Date.UTC(cursor.getUTCFullYear(), cursor.getUTCMonth(), cursor.getUTCDate() - 1)))}>‹</button><button className={`calendar-open-heading ${day && selected({ kind: "date", dateFrom: day.date, dateTo: day.date, dateSource: source, title: "" }) ? "selected" : ""}`} onClick={() => day && openDay(day)}>{cursor.toLocaleDateString("sk-SK", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" })} · {countText(day?.count ?? 0)}</button><button aria-label="Nasledujúci deň" onClick={() => setCursor(new Date(Date.UTC(cursor.getUTCFullYear(), cursor.getUTCMonth(), cursor.getUTCDate() + 1)))}>›</button></div>{day?.hours.map(hour => <div key={hour.hour} className="calendar-hour-row"><span>{String(hour.hour).padStart(2, "0")}:00–{String(hour.hour).padStart(2, "0")}:59</span><b>{countText(hour.count)}</b></div>) ?? <div className="calendar-empty-day">V tento deň nie sú fotografie</div>}</div>}
  </div>;
}

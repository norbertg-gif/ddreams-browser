import { useRef, type PointerEvent as ReactPointerEvent } from "react";

type Props = {
  direction: "x" | "y";
  value: number | (() => number);
  min: number;
  max: number;
  invert?: boolean;
  title?: string;
  onChange: (value: number) => void;
  onCommit?: (value: number) => void;
  onReset: () => void;
};

/** Ťahadlo medzi panelmi. Ťahaním mení rozmer suseda, dvojklikom ho vráti na predvolený. */
export default function Splitter({ direction, value, min, max, invert = false, title, onChange, onCommit, onReset }: Props) {
  const start = useRef<{ pos: number; value: number; latest?: number } | null>(null);
  const clamp = (v: number) => Math.round(Math.min(Math.max(v, min), Math.max(min, max)));
  const position = (e: ReactPointerEvent) => direction === "x" ? e.clientX : e.clientY;
  return <div
    className={`splitter splitter-${direction}`}
    role="separator"
    aria-orientation={direction === "x" ? "vertical" : "horizontal"}
    title={title ?? "Ťahaním zmeníš veľkosť, dvojklikom vrátiš predvolenú"}
    onPointerDown={e => { if (e.button !== 0) return; e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); start.current = { pos: position(e), value: typeof value === "function" ? value() : value }; document.body.classList.add(`resizing-${direction}`); }}
    onPointerMove={e => {
      const s = start.current;
      if (!s) return;
      const delta = (position(e) - s.pos) * (invert ? -1 : 1);
      s.latest = clamp(s.value + delta);
      onChange(s.latest);
    }}
    onPointerUp={e => {
      const latest = start.current?.latest;
      start.current = null;
      if (latest !== undefined) onCommit?.(latest);
      if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
      document.body.classList.remove(`resizing-${direction}`);
    }}
    onPointerCancel={() => {
      if (onCommit && start.current) onChange(start.current.value);
      start.current = null;
      document.body.classList.remove(`resizing-${direction}`);
    }}
    onDoubleClick={onReset}
  />;
}

import { useCallback, useEffect, useRef, type MutableRefObject } from "react";

const DRAG_TYPES = [
  "application/x-ddreams-asset-ids",
  "application/x-ddreams-category-id",
  "application/x-ddreams-keyword-id",
];

export function useSpringOpen(isOpen: boolean, hasChildren: boolean, open: () => void) {
  const timer = useRef<number | null>(null);
  const cancel = useCallback(() => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
  }, []);
  const enter = useCallback((event: React.DragEvent<HTMLElement>) => startSpringOpen(timer, event, isOpen, hasChildren, open), [hasChildren, isOpen, open]);
  const leave = useCallback((event: React.DragEvent<HTMLElement>) => {
    const next = event.relatedTarget;
    if (next instanceof Node && event.currentTarget.contains(next)) return;
    cancel();
  }, [cancel]);
  useEffect(() => { if (isOpen) cancel(); return cancel; }, [cancel, isOpen]);
  return { onDragEnter: enter, onDragOver: enter, onDragLeave: leave, onDrop: cancel };
}

export function springOpenHandlers(timer: MutableRefObject<number | null>, isOpen: boolean, hasChildren: boolean, open: () => void) {
  const cancel = () => { if (timer.current !== null) window.clearTimeout(timer.current); timer.current = null; };
  const enter = (event: React.DragEvent<HTMLElement>) => startSpringOpen(timer, event, isOpen, hasChildren, open);
  return { onDragEnter: enter, onDragOver: enter, onDragLeave: (event: React.DragEvent<HTMLElement>) => { const next=event.relatedTarget;if(next instanceof Node&&event.currentTarget.contains(next))return;cancel(); }, onDrop: cancel };
}

function startSpringOpen(timer: MutableRefObject<number | null>, event: React.DragEvent<HTMLElement>, isOpen: boolean, hasChildren: boolean, open: () => void) {
  if (isOpen || !hasChildren || timer.current !== null || !DRAG_TYPES.some(type => event.dataTransfer.types.includes(type))) return;
  timer.current = window.setTimeout(() => { timer.current = null; open(); }, 700);
}

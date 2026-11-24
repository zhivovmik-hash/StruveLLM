import {PanelLeftClose, PanelLeftOpen} from 'lucide-react';
import {useLayoutEffect, useRef, useState, type ReactNode, type PointerEvent} from 'react';
import './navigation-sidebar.css';

const storageKey = 'navigation-sidebar';
const minWidth = 180;
const maxWidth = 620;
const minWorkspace = 400;

function preferences(): {width?: number; collapsed: boolean} {
  try {
    const value = JSON.parse(localStorage.getItem(storageKey) || '{}');
    return {
      width: Number.isFinite(value?.width) ? value.width : undefined,
      collapsed: value?.collapsed === true,
    };
  } catch {
    return {collapsed: false};
  }
}

export default function NavigationSidebar({children, english}: {children: ReactNode; english: boolean}) {
  const [saved, setSaved] = useState(preferences);
  const [available, setAvailable] = useState(window.innerWidth);
  const [dragWidth, setDragWidth] = useState<number>();
  const dock = useRef<HTMLDivElement>(null);
  const drag = useRef<{pointer: number; origin: number; width: number} | null>(null);
  const limit = Math.max(minWidth, Math.min(maxWidth, available - minWorkspace));
  const clamp = (value: number) => Math.max(minWidth, Math.min(limit, Math.round(value)));
  const width = clamp(dragWidth ?? saved.width ?? (available < 920 ? 200 : 252));
  const toggleLabel = saved.collapsed
    ? (english ? 'Show sidebar' : 'Показать левую панель')
    : (english ? 'Collapse sidebar' : 'Свернуть левую панель');
  const resizeLabel = english ? 'Sidebar width' : 'Ширина левой панели';

  useLayoutEffect(() => {
    const layout = dock.current!.parentElement!;
    const measure = () => setAvailable(layout.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(layout);
    return () => observer.disconnect();
  }, []);

  function save(next: typeof saved) {
    setSaved(next);
    localStorage.setItem(storageKey, JSON.stringify(next));
  }

  function pointerWidth(event: PointerEvent<HTMLDivElement>) {
    const start = drag.current;
    if (start?.pointer === event.pointerId) return clamp(start.width + event.clientX - start.origin);
  }

  function cancelDrag() {
    drag.current = null;
    setDragWidth(undefined);
  }

  return <div ref={dock} className="navigation-dock" style={{width: saved.collapsed ? 0 : width}}>
    <button type="button" className="navigation-toggle" aria-controls="project-navigation"
      aria-expanded={!saved.collapsed} aria-label={toggleLabel} title={toggleLabel}
      onClick={() => save({...saved, collapsed: !saved.collapsed})}>
      {saved.collapsed ? <PanelLeftOpen size={18}/> : <PanelLeftClose size={18}/>}
    </button>
    <aside className="sidebar" id="project-navigation" hidden={saved.collapsed}>
      {children}
      <div role="separator" tabIndex={0} aria-controls="project-navigation" aria-orientation="vertical"
        aria-label={resizeLabel} title={resizeLabel} aria-valuemin={minWidth} aria-valuemax={limit}
        aria-valuenow={width} className="navigation-resizer"
        onPointerDown={event => {
          if (!event.isPrimary || event.button !== 0) return;
          event.currentTarget.focus({preventScroll: true});
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = {pointer: event.pointerId, origin: event.clientX, width};
          event.preventDefault();
        }}
        onPointerMove={event => {
          const next = pointerWidth(event);
          if (next !== undefined) setDragWidth(next);
        }}
        onPointerUp={event => {
          const next = pointerWidth(event);
          if (next === undefined) return;
          save({...saved, width: next});
          cancelDrag();
        }}
        onLostPointerCapture={cancelDrag}
        onKeyDown={event => {
          const widths: Record<string, number> = {
            ArrowLeft: width - 16, ArrowRight: width + 16, Home: minWidth, End: limit,
          };
          if (!(event.key in widths)) return;
          event.preventDefault();
          save({...saved, width: clamp(widths[event.key])});
        }}
      />
    </aside>
  </div>;
}

import { useCallback, useEffect, useState, type CSSProperties } from 'react';
import type { TxnDTO } from '../api.ts';

/** Register columns: resizable (drag a header edge) and sortable (click a header). */
export type ColId = 'account' | 'date' | 'payee' | 'category' | 'memo' | 'outflow' | 'inflow' | 'cleared' | 'balance';

interface Col {
  id: ColId;
  label: string;
  /** Fixed width in px, or the minimum when the column flexes. */
  width: number;
  /** Share of the leftover space until the column is resized by hand. */
  flex?: number;
  min: number;
  align?: 'num' | 'center';
}

const SELECT_COL = 32;
const GAP = 8;
const PAD = 40;

const COLS: Col[] = [
  { id: 'account', label: 'Account', width: 90, flex: 0.8, min: 60 },
  { id: 'date', label: 'Date', width: 100, min: 70 },
  { id: 'payee', label: 'Payee', width: 140, flex: 1.3, min: 60 },
  { id: 'category', label: 'Category', width: 140, flex: 1.3, min: 60 },
  { id: 'memo', label: 'Memo', width: 100, flex: 1, min: 50 },
  { id: 'outflow', label: 'Outflow', width: 100, min: 60, align: 'num' },
  { id: 'inflow', label: 'Inflow', width: 100, min: 60, align: 'num' },
  { id: 'cleared', label: 'C', width: 40, min: 32, align: 'center' },
  { id: 'balance', label: 'Balance', width: 120, min: 90, align: 'num' },
];

export interface Sort {
  col: ColId;
  desc: boolean;
}

const DEFAULT_SORT: Sort = { col: 'date', desc: true };
const SORT_KEY = 'budget4.register.sort';
const WIDTH_KEY = 'budget4.register.widths';

function load<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? { ...fallback, ...JSON.parse(v) } : fallback;
  } catch {
    return fallback;
  }
}

function save(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* not saved; fine */
  }
}

/** Column widths and sort order, remembered between sessions. */
export function useRegisterLayout(showAccount: boolean) {
  const [sort, setSortState] = useState<Sort>(() => load(SORT_KEY, DEFAULT_SORT));
  const [widths, setWidths] = useState<Partial<Record<ColId, number>>>(() => load(WIDTH_KEY, {}));
  useEffect(() => save(SORT_KEY, sort), [sort]);
  useEffect(() => save(WIDTH_KEY, widths), [widths]);

  const cols = COLS.filter((c) => (c.id !== 'account' || showAccount));
  const track = (c: Col) => {
    const w = widths[c.id];
    if (w) return `${w}px`;
    return c.flex ? `minmax(${c.width}px, ${c.flex}fr)` : `${c.width}px`;
  };
  const minWidth = PAD + SELECT_COL + cols.reduce((s, c) => s + (widths[c.id] ?? c.width) + GAP, 0);
  const style = { '--reg-cols': [`${SELECT_COL}px`, ...cols.map(track)].join(' ') } as CSSProperties;

  const setSort = useCallback((col: ColId) => {
    // A new column starts ascending (date starts newest first); clicking again flips it.
    setSortState((s) => (s.col === col ? { col, desc: !s.desc } : { col, desc: col === 'date' }));
  }, []);
  const setWidth = useCallback((id: ColId, w: number | null) => {
    setWidths((cur) => {
      const next = { ...cur };
      if (w === null) delete next[id];
      else next[id] = w;
      return next;
    });
  }, []);
  return { cols, sort, setSort, widths, setWidth, style, minWidth };
}

const CLEARED_ORDER: Record<string, number> = { Uncleared: 0, Cleared: 1, Reconciled: 2 };

/** Sorts register rows. `rows` come in register (running-balance) order, oldest first. */
export function sortRows(rows: TxnDTO[], sort: Sort, label: { payee: (t: TxnDTO) => string; category: (t: TxnDTO) => string; account: (t: TxnDTO) => string }): TxnDTO[] {
  const dir = sort.desc ? -1 : 1;
  const text = (f: (t: TxnDTO) => string) => {
    const keys = new Map(rows.map((t) => [t, f(t).toLocaleLowerCase()]));
    return (a: TxnDTO, b: TxnDTO) => {
      const x = keys.get(a)!;
      const y = keys.get(b)!;
      // Blanks go last either way.
      if (!x !== !y) return x ? -1 : 1;
      return dir * x.localeCompare(y);
    };
  };
  // Outflow/inflow: rows without one go last either way.
  const side = (sign: number) => (a: TxnDTO, b: TxnDTO) => {
    const x = sign * a.amount > 0 ? sign * a.amount : null;
    const y = sign * b.amount > 0 ? sign * b.amount : null;
    if ((x === null) !== (y === null)) return x === null ? 1 : -1;
    return dir * ((x ?? 0) - (y ?? 0));
  };
  const num = (f: (t: TxnDTO) => number) => (a: TxnDTO, b: TxnDTO) => dir * (f(a) - f(b));
  const primary: (a: TxnDTO, b: TxnDTO) => number = {
    date: (a: TxnDTO, b: TxnDTO) => dir * a.date.localeCompare(b.date),
    account: text(label.account),
    payee: text(label.payee),
    category: text(label.category),
    memo: text((t) => t.memo ?? ''),
    outflow: side(-1),
    inflow: side(1),
    cleared: num((t) => CLEARED_ORDER[t.cleared] ?? 0),
    balance: num((t) => t.runningBalance ?? 0),
  }[sort.col];
  // Ties: by date, then register order. A date sort keeps its own direction, so same-day rows
  // stay in running-balance order; other columns fall back to newest first.
  const tie = sort.col === 'date' ? dir : -1;
  const index = new Map(rows.map((t, i) => [t, i]));
  return [...rows].sort((a, b) => primary(a, b) || tie * a.date.localeCompare(b.date) || tie * (index.get(a)! - index.get(b)!));
}

export function RegisterHead({
  cols,
  sort,
  setSort,
  setWidth,
  balanceLabel,
}: {
  cols: Col[];
  sort: Sort;
  setSort: (c: ColId) => void;
  setWidth: (c: ColId, w: number | null) => void;
  /** "Balance" in one account's register; nothing (and not sortable) in All accounts. */
  balanceLabel: boolean;
}) {
  const startResize = (e: React.PointerEvent<HTMLSpanElement>, c: Col) => {
    e.preventDefault();
    e.stopPropagation();
    const cell = e.currentTarget.parentElement!;
    const startX = e.clientX;
    const startW = cell.getBoundingClientRect().width;
    const move = (ev: PointerEvent) => setWidth(c.id, Math.max(c.min, Math.round(startW + ev.clientX - startX)));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('col-resizing');
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    document.body.classList.add('col-resizing');
  };
  return (
    <div className="reg-head" role="row">
      <span />
      {cols.map((c) => {
        const sortable = c.id !== 'balance' || balanceLabel;
        const active = sortable && sort.col === c.id;
        const label = c.id === 'balance' && !balanceLabel ? '' : c.label;
        return (
          <div
            key={c.id}
            role="columnheader"
            aria-sort={active ? (sort.desc ? 'descending' : 'ascending') : undefined}
            className={`reg-th ${c.align ?? ''} ${sortable ? 'sortable' : ''} ${active ? 'active' : ''}`}
            title={sortable ? `Sort by ${c.label.toLowerCase() === 'c' ? 'cleared' : c.label.toLowerCase()}` : undefined}
            onClick={sortable ? () => setSort(c.id) : undefined}
          >
            <span className="ellipsis">{label}</span>
            {active && <span className="sort-arrow">{sort.desc ? '▼' : '▲'}</span>}
            <span
              className="col-resize"
              title="Drag to resize · double-click to reset"
              onPointerDown={(e) => startResize(e, c)}
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => {
                e.stopPropagation();
                setWidth(c.id, null);
              }}
            />
          </div>
        );
      })}
    </div>
  );
}

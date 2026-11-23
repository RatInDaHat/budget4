import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { api, type AccountDTO, type MasterDTO, type PayeeDTO, type StateDTO } from './api.ts';
import { configureFormat } from './format.ts';

export type View = { kind: 'budget' } | { kind: 'account'; accountId: string | null } | { kind: 'scheduled' } | { kind: 'settings' };

interface Ctx {
  state: StateDTO | null;
  /** Bumped whenever budget data changes (locally or from another device). */
  version: number;
  view: View;
  setView: (v: View) => void;
  refresh: () => Promise<void>;
  /** Runs an edit, then refreshes; errors surface as a toast. */
  run: <T>(fn: () => Promise<T>) => Promise<T | undefined>;
  /** Undoes (or redoes) the latest edit and says what it was. */
  undo: (redo?: boolean) => Promise<void>;
  toast: string | null;
  setToast: (t: string | null) => void;
  onClosed: () => void;
  lookups: Lookups;
}

export interface CategoryOption {
  id: string;
  name: string;
  master: string;
  hidden: boolean;
}

export interface Lookups {
  accounts: Map<string, AccountDTO>;
  payees: Map<string, PayeeDTO>;
  categories: Map<string, CategoryOption>;
  categoryOptions: CategoryOption[];
  masters: MasterDTO[];
  categoryLabel: (id: string | null) => string;
  payeeLabel: (id: string | null) => string;
}

const StateCtx = createContext<Ctx | null>(null);

export function useApp(): Ctx {
  const c = useContext(StateCtx);
  if (!c) throw new Error('useApp outside provider');
  return c;
}

const INCOME_OPTIONS: CategoryOption[] = [
  { id: 'Category/__ImmediateIncome__', name: 'Income for this month', master: 'Income', hidden: false },
  { id: 'Category/__DeferredIncome__', name: 'Income for next month', master: 'Income', hidden: false },
];

function buildLookups(state: StateDTO | null): Lookups {
  const accounts = new Map((state?.accounts ?? []).map((a) => [a.id, a]));
  const payees = new Map((state?.payees ?? []).map((p) => [p.id, p]));
  const categories = new Map<string, CategoryOption>();
  const categoryOptions: CategoryOption[] = [...INCOME_OPTIONS];
  for (const o of INCOME_OPTIONS) categories.set(o.id, o);
  for (const m of state?.masters ?? []) {
    const hidden = m.id === 'MasterCategory/__Hidden__';
    for (const s of m.subs) {
      const name = hidden ? s.name.split(' ` ').slice(0, 2).join(': ') : s.name;
      const o = { id: s.id, name, master: hidden ? 'Hidden' : m.name, hidden };
      categories.set(s.id, o);
      categoryOptions.push(o);
    }
  }
  categories.set('Category/__Split__', { id: 'Category/__Split__', name: 'Split (multiple categories)', master: '', hidden: false });
  return {
    accounts,
    payees,
    categories,
    categoryOptions,
    masters: state?.masters ?? [],
    categoryLabel: (id) => {
      if (!id) return '';
      const c = categories.get(id);
      return c ? (c.master && c.master !== 'Income' ? `${c.master}: ${c.name}` : c.name) : '(deleted category)';
    },
    payeeLabel: (id) => (id ? payees.get(id)?.name ?? '' : ''),
  };
}

export function AppProvider({ initial, onClosed, children }: { initial: StateDTO; onClosed: () => void; children: ReactNode }) {
  const [state, setState] = useState<StateDTO | null>(initial);
  const [version, setVersion] = useState(0);
  const [view, setView] = useState<View>({ kind: 'budget' });
  const [toast, setToast] = useState<string | null>(null);
  const inflight = useRef<Promise<void> | null>(null);

  useEffect(() => {
    configureFormat(initial.budget.locale, initial.budget.currency, initial.budget.decimalDigits);
  }, [initial]);

  const refresh = useCallback(async () => {
    if (inflight.current) return inflight.current;
    inflight.current = (async () => {
      try {
        const s = await api.state();
        setState(s);
        setVersion((v) => v + 1);
      } finally {
        inflight.current = null;
      }
    })();
    return inflight.current;
  }, []);

  const run = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T | undefined> => {
      try {
        const out = await fn();
        await refresh();
        return out;
      } catch (e) {
        setToast((e as Error).message);
        return undefined;
      }
    },
    [refresh],
  );

  const undo = useCallback(
    async (redo = false) => {
      const r = await run(() => (redo ? api.redo() : api.undo()));
      if (r) setToast(`${redo ? 'Redid' : 'Undid'} ${r.label}`);
    },
    [run],
  );

  // Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z), except while typing, where they edit the text.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const k = e.key.toLowerCase();
      const redo = k === 'y' || (k === 'z' && e.shiftKey);
      if (k !== 'z' && k !== 'y') return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName))) return;
      if (document.querySelector('.modal-backdrop')) return;
      e.preventDefault();
      void undo(redo);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [undo]);

  useEffect(() => {
    let timer: number | undefined;
    const schedule = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => void refresh().catch(() => {}), 150);
    };
    // Events from the Rust side's sync loop: "changed", "status", or "closed".
    const unlisten = listen<string>('budget-event', (e) => {
      if (e.payload === 'closed') onClosed();
      else schedule();
    });
    return () => {
      void unlisten.then((f) => f());
      window.clearTimeout(timer);
    };
  }, [refresh, onClosed]);

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 6000);
    return () => window.clearTimeout(t);
  }, [toast]);

  const lookups = useMemo(() => buildLookups(state), [state]);

  return (
    <StateCtx.Provider value={{ state, version, view, setView, refresh, run, undo, toast, setToast, onClosed, lookups }}>{children}</StateCtx.Provider>
  );
}

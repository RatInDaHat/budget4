import { useEffect, useMemo, useRef, useState } from 'react';
import { api, type MasterDTO, type MonthDTO, type SubDTO } from '../api.ts';
import { addMonths, money, monthName, parseAmount, plain, shortMonth } from '../format.ts';
import { useApp } from '../state.tsx';
import { Modal } from './Modal.tsx';

const HIDDEN = 'MasterCategory/__Hidden__';

export function BudgetView() {
  const { state, version, run, setView } = useApp();
  const [month, setMonth] = useState(() => state?.months.current ?? '');
  const [count, setCount] = useState(() => (window.innerWidth > 1400 ? 3 : 1));
  const [months, setMonths] = useState<MonthDTO[]>([]);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set([HIDDEN]));
  const [dialog, setDialog] = useState<null | { kind: 'addMaster' } | { kind: 'addSub'; master: MasterDTO } | { kind: 'rename'; id: string; name: string } | { kind: 'delete'; sub: SubDTO } | { kind: 'note'; id: string; note: string | null }>(null);
  const [menu, setMenu] = useState<null | { month: string; sub: SubDTO; x: number; y: number }>(null);

  useEffect(() => {
    let live = true;
    api.months(month, count).then((m) => live && setMonths(m)).catch(() => {});
    return () => {
      live = false;
    };
  }, [month, count, version]);

  const masters = useMemo(() => (state?.masters ?? []).filter((m) => m.type === 'OUTFLOW' && m.id !== 'MasterCategory/__Internal__'), [state]);
  const visible = masters.filter((m) => m.id !== HIDDEN);
  const hidden = masters.find((m) => m.id === HIDDEN);
  if (!state) return null;

  const toggle = (id: string) => {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setCollapsed(next);
  };

  const prevMonthOf = (m: string) => addMonths(m, -1);

  return (
    <div className="budget">
      <header className="toolbar">
        <div className="month-nav">
          <button className="icon" onClick={() => setMonth(addMonths(month, -1))} disabled={month <= state.months.first} aria-label="Previous month">
            ‹
          </button>
          <h2>{monthName(month)}</h2>
          <button className="icon" onClick={() => setMonth(addMonths(month, 1))} disabled={addMonths(month, count - 1) >= state.months.last} aria-label="Next month">
            ›
          </button>
          {month !== state.months.current && (
            <button className="link small" onClick={() => setMonth(state.months.current)}>
              Today
            </button>
          )}
        </div>
        <div className="row">
          <div className="seg" role="group" aria-label="Months shown">
            {[1, 3].map((n) => (
              <button key={n} className={count === n ? 'on' : ''} onClick={() => setCount(n)}>
                {n} month{n > 1 ? 's' : ''}
              </button>
            ))}
          </div>
          <button onClick={() => setDialog({ kind: 'addMaster' })}>+ Category group</button>
        </div>
      </header>

      <div className="budget-scroll">
        <div className="budget-grid" style={{ gridTemplateColumns: `minmax(200px, 280px) repeat(${months.length}, minmax(330px, 1fr))` }}>
          <div className="cell head-spacer" />
          {months.map((m) => (
            <MonthHeader key={m.month} m={m} prev={prevMonthOf(m.month)} />
          ))}

          <div className="cell colhead sticky-left">Category</div>
          {months.map((m) => (
            <div key={m.month} className="cell colhead triple">
              <span>Budgeted</span>
              <span>Outflows</span>
              <span>Balance</span>
            </div>
          ))}

          {visible.map((master) => (
            <MasterBlock
              key={master.id}
              master={master}
              months={months}
              collapsed={collapsed.has(master.id)}
              onToggle={() => toggle(master.id)}
              onAddSub={() => setDialog({ kind: 'addSub', master })}
              onRename={(id, name) => setDialog({ kind: 'rename', id, name })}
              onMenu={(month, sub, x, y) => setMenu({ month, sub, x, y })}
            />
          ))}
          {hidden && hidden.subs.length > 0 && (
            <MasterBlock
              master={{ ...hidden, name: `Hidden categories (${hidden.subs.length})` }}
              months={months}
              collapsed={collapsed.has(HIDDEN)}
              onToggle={() => toggle(HIDDEN)}
              onMenu={(month, sub, x, y) => setMenu({ month, sub, x, y })}
              isHidden
            />
          )}
        </div>
      </div>

      {menu && (
        <CategoryMenu
          {...menu}
          months={months}
          onClose={() => setMenu(null)}
          onRename={() => setDialog({ kind: 'rename', id: menu.sub.id, name: menu.sub.name })}
          onDelete={() => setDialog({ kind: 'delete', sub: menu.sub })}
          onNote={() => setDialog({ kind: 'note', id: menu.sub.id, note: menu.sub.note })}
          onShowActivity={() => setView({ kind: 'account', accountId: null })}
        />
      )}

      {dialog?.kind === 'addMaster' && (
        <NameDialog title="New category group" onClose={() => setDialog(null)} onSave={(n) => run(() => api.createMaster(n))} />
      )}
      {dialog?.kind === 'addSub' && (
        <NameDialog title={`New category in ${dialog.master.name}`} onClose={() => setDialog(null)} onSave={(n) => run(() => api.createCategory(dialog.master.id, n))} />
      )}
      {dialog?.kind === 'rename' && (
        <NameDialog title="Rename" initial={dialog.name} onClose={() => setDialog(null)} onSave={(n) => run(() => api.updateCategory(dialog.id, { name: n }))} />
      )}
      {dialog?.kind === 'note' && (
        <NoteDialog initial={dialog.note ?? ''} onClose={() => setDialog(null)} onSave={(n) => run(() => api.updateCategory(dialog.id, { note: n || null }))} />
      )}
      {dialog?.kind === 'delete' && <DeleteCategoryDialog sub={dialog.sub} masters={masters} onClose={() => setDialog(null)} />}
    </div>
  );
}

function MonthHeader({ m, prev }: { m: MonthDTO; prev: string }) {
  const lines: [string, number][] = [
    [`Not budgeted in ${shortMonth(prev)}`, m.notBudgetedLastMonth],
    [`Overspent in ${shortMonth(prev)}`, m.overspentLastMonth],
    [`Income for ${shortMonth(m.month)}`, m.income],
    [`Budgeted in ${shortMonth(m.month)}`, -m.totalBudgeted],
  ];
  return (
    <div className="cell month-head">
      <div className="month-title">{monthName(m.month)}</div>
      <div className="summary">
        <table>
          <tbody>
            {lines.map(([label, v]) => (
              <tr key={label}>
                <td className={`num ${v < 0 ? 'neg' : ''}`}>{v >= 0 ? '+' : '−'}{plain(Math.abs(v))}</td>
                <td className="muted">{label}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <div className={`available ${m.available < 0 ? 'neg' : ''}`}>
          <span className="big">{money(m.available)}</span>
          <span className="small">Available to budget</span>
        </div>
      </div>
    </div>
  );
}

function MasterBlock({
  master,
  months,
  collapsed,
  onToggle,
  onAddSub,
  onRename,
  onMenu,
  isHidden,
}: {
  master: MasterDTO;
  months: MonthDTO[];
  collapsed: boolean;
  onToggle: () => void;
  onAddSub?: () => void;
  onRename?: (id: string, name: string) => void;
  onMenu: (month: string, sub: SubDTO, x: number, y: number) => void;
  isHidden?: boolean;
}) {
  return (
    <>
      <div className="cell master sticky-left">
        <button className="caret" onClick={onToggle} aria-label={collapsed ? 'Expand' : 'Collapse'}>
          {collapsed ? '▸' : '▾'}
        </button>
        <span className="master-name" onDoubleClick={() => onRename?.(master.id, master.name)}>
          {master.name}
        </span>
        {onAddSub && (
          <button className="link small add-sub" onClick={onAddSub} title="Add a category">
            +
          </button>
        )}
      </div>
      {months.map((m) => {
        const t = m.masters[master.id];
        return (
          <div key={m.month} className="cell master triple">
            <span className="num">{t ? plain(t.budgeted) : ''}</span>
            <span className="num">{t ? plain(-t.spent) : ''}</span>
            <span className="num">{t ? plain(t.balance) : ''}</span>
          </div>
        );
      })}
      {!collapsed &&
        master.subs.map((sub) => (
          <SubRow key={sub.id} sub={sub} months={months} onMenu={onMenu} isHidden={isHidden} />
        ))}
    </>
  );
}

function SubRow({ sub, months, onMenu, isHidden }: { sub: SubDTO; months: MonthDTO[]; onMenu: (month: string, sub: SubDTO, x: number, y: number) => void; isHidden?: boolean }) {
  const label = isHidden ? sub.name.split(' ` ').slice(0, 2).join(': ') : sub.name;
  return (
    <>
      <div className="cell sub sticky-left" title={sub.note ?? undefined}>
        <span className="sub-name">{label}</span>
        {sub.note && <span className="note-dot" title={sub.note} />}
        <button className="icon menu-btn" onClick={(e) => onMenu(months[0]?.month ?? '', sub, e.clientX, e.clientY)} aria-label="Category actions">
          ⋯
        </button>
      </div>
      {months.map((m) => {
        const c = m.categories[sub.id];
        return (
          <div key={m.month} className="cell sub triple">
            <BudgetInput month={m.month} categoryId={sub.id} value={c?.budgeted ?? 0} />
            <span className="num muted">{c ? plain(-c.spent) : ''}</span>
            <span className="num">
              {c && (
                <button className={`pill ${pillClass(c.balance, c.handling)}`} onClick={(e) => onMenu(m.month, sub, e.clientX, e.clientY)} title={c.handling === 'Confined' ? 'Overspending stays in this category next month' : undefined}>
                  {plain(c.balance)}
                  {c.handling === 'Confined' && <span className="arrow">→</span>}
                </button>
              )}
            </span>
          </div>
        );
      })}
    </>
  );
}

function pillClass(balance: number, handling: string): string {
  if (balance < 0) return handling === 'Confined' ? 'confined' : 'negative';
  if (balance > 0) return 'positive';
  return 'zero';
}

function BudgetInput({ month, categoryId, value }: { month: string; categoryId: string; value: number }) {
  const { run } = useApp();
  const [text, setText] = useState(plain(value));
  const [focused, setFocused] = useState(false);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!focused) setText(plain(value));
  }, [value, focused]);
  const commit = () => {
    const v = parseAmount(text);
    if (v === null) {
      setText(plain(value));
      return;
    }
    if (v !== value) void run(() => api.setBudgeted(month, categoryId, v));
  };
  return (
    <input
      ref={ref}
      className={`budget-input num ${value === 0 ? 'muted' : ''}`}
      value={text}
      inputMode="decimal"
      onFocus={(e) => {
        setFocused(true);
        setText(value === 0 ? '' : String(value));
        requestAnimationFrame(() => e.target.select());
      }}
      onBlur={() => {
        setFocused(false);
        commit();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          (e.target as HTMLInputElement).blur();
          moveFocus(ref.current, 1);
        } else if (e.key === 'Escape') {
          setText(plain(value));
          setFocused(false);
          (e.target as HTMLInputElement).blur();
        } else if (e.key === 'ArrowDown') {
          e.preventDefault();
          moveFocus(ref.current, 1);
        } else if (e.key === 'ArrowUp') {
          e.preventDefault();
          moveFocus(ref.current, -1);
        }
      }}
      aria-label="Budgeted"
    />
  );
}

/** Moves to the budgeted input in the same month column, `dir` rows away. */
function moveFocus(from: HTMLInputElement | null, dir: number) {
  if (!from) return;
  const all = [...document.querySelectorAll<HTMLInputElement>('.budget-input')];
  const cols = document.querySelectorAll('.month-head').length || 1;
  const i = all.indexOf(from);
  const next = all[i + dir * cols];
  next?.focus();
}

function CategoryMenu({
  month,
  sub,
  x,
  y,
  months,
  onClose,
  onRename,
  onDelete,
  onNote,
}: {
  month: string;
  sub: SubDTO;
  x: number;
  y: number;
  months: MonthDTO[];
  onClose: () => void;
  onRename: () => void;
  onDelete: () => void;
  onNote: () => void;
  onShowActivity: () => void;
}) {
  const { run } = useApp();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    setTimeout(() => document.addEventListener('mousedown', close));
    document.addEventListener('keydown', key);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', key);
    };
  }, [onClose]);
  const m = months.find((x) => x.month === month);
  const c = m?.categories[sub.id];
  const isHidden = sub.name.includes(' ` ');
  const act = (fn: () => Promise<unknown>) => {
    onClose();
    void run(fn);
  };
  const prevMonth = addMonths(month, -1);
  const style = { left: Math.min(x, window.innerWidth - 280), top: Math.min(y, window.innerHeight - 330) };

  return (
    <div className="popover" style={style} ref={ref} role="menu">
      <div className="popover-title">{isHidden ? sub.name.split(' ` ')[1] : sub.name} · {monthName(month)}</div>
      {c && c.balance < 0 && (
        <>
          <div className="popover-label">When overspent</div>
          <button className={c.handling === 'AffectsBuffer' ? 'checked' : ''} onClick={() => act(() => api.setHandling(month, sub.id, 'AffectsBuffer'))}>
            Take it from next month's Available to Budget
          </button>
          <button className={c.handling === 'Confined' ? 'checked' : ''} onClick={() => act(() => api.setHandling(month, sub.id, 'Confined'))}>
            Keep the negative balance in this category
          </button>
          <hr />
        </>
      )}
      {c && (
        <>
          <button onClick={() => act(() => api.setBudgeted(month, sub.id, Math.max(0, c.budgeted - c.balance)))}>Budget to zero the balance</button>
          <BudgetFromPrevious month={month} prevMonth={prevMonth} categoryId={sub.id} act={act} />
          <hr />
        </>
      )}
      <button onClick={() => (onClose(), onRename())}>Rename…</button>
      <button onClick={() => (onClose(), onNote())}>Note…</button>
      {isHidden ? (
        <button onClick={() => act(() => api.unhideCategory(sub.id))}>Unhide</button>
      ) : (
        <button onClick={() => act(() => api.hideCategory(sub.id))}>Hide</button>
      )}
      <button className="danger" onClick={() => (onClose(), onDelete())}>
        Delete…
      </button>
    </div>
  );
}

function BudgetFromPrevious({ month, prevMonth, categoryId, act }: { month: string; prevMonth: string; categoryId: string; act: (fn: () => Promise<unknown>) => void }) {
  const [prev, setPrev] = useState<MonthDTO | null>(null);
  useEffect(() => {
    api.months(prevMonth, 1).then((m) => setPrev(m[0] ?? null)).catch(() => {});
  }, [prevMonth]);
  const p = prev?.categories[categoryId];
  if (!p) return null;
  return (
    <>
      <button onClick={() => act(() => api.setBudgeted(month, categoryId, p.budgeted))}>Budget last month's amount ({plain(p.budgeted)})</button>
      <button onClick={() => act(() => api.setBudgeted(month, categoryId, Math.max(0, p.spent)))}>Budget last month's outflows ({plain(Math.max(0, p.spent))})</button>
    </>
  );
}

function NameDialog({ title, initial = '', onClose, onSave }: { title: string; initial?: string; onClose: () => void; onSave: (name: string) => unknown }) {
  const [name, setName] = useState(initial);
  return (
    <Modal title={title} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) {
            onSave(name.trim());
            onClose();
          }
        }}
      >
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        <div className="row end">
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" disabled={!name.trim()}>
            Save
          </button>
        </div>
      </form>
    </Modal>
  );
}

function NoteDialog({ initial, onClose, onSave }: { initial: string; onClose: () => void; onSave: (note: string) => unknown }) {
  const [note, setNote] = useState(initial);
  return (
    <Modal title="Category note" onClose={onClose}>
      <div className="form">
        <textarea autoFocus rows={5} value={note} onChange={(e) => setNote(e.target.value)} />
        <div className="row end">
          <button onClick={onClose}>Cancel</button>
          <button
            className="primary"
            onClick={() => {
              onSave(note);
              onClose();
            }}
          >
            Save
          </button>
        </div>
      </div>
    </Modal>
  );
}

function DeleteCategoryDialog({ sub, masters, onClose }: { sub: SubDTO; masters: MasterDTO[]; onClose: () => void }) {
  const { run } = useApp();
  const options = masters.flatMap((m) => m.subs.filter((s) => s.id !== sub.id).map((s) => ({ id: s.id, label: `${m.name === 'Hidden Categories' ? 'Hidden' : m.name}: ${s.name.split(' ` ').slice(0, 2).join(': ')}` })));
  const [target, setTarget] = useState(options[0]?.id ?? '');
  return (
    <Modal title={`Delete "${sub.name.split(' ` ').slice(0, 2).join(': ')}"`} onClose={onClose}>
      <div className="form">
        <p>Its transactions and budgeted amounts move to another category:</p>
        <select value={target} onChange={(e) => setTarget(e.target.value)}>
          {options.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
        <p className="muted small">Hiding keeps the category's history without showing it in the budget.</p>
        <div className="row end">
          <button onClick={onClose}>Cancel</button>
          <button
            className="danger-btn"
            disabled={!target}
            onClick={() => {
              onClose();
              void run(() => api.deleteCategory(sub.id, target));
            }}
          >
            Delete and move
          </button>
        </div>
      </div>
    </Modal>
  );
}

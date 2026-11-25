import { useState } from 'react';
import { api, type AccountDTO } from '../api.ts';
import { money, relativeTime, round } from '../format.ts';
import { useApp } from '../state.tsx';
import { AccountDialog } from './AccountDialog.tsx';

export function Sidebar() {
  const { state, view, setView, run, undo } = useApp();
  const [adding, setAdding] = useState(false);
  const [showClosed, setShowClosed] = useState(false);
  const [drag, setDrag] = useState<DragState | null>(null);
  if (!state) return null;

  const open = state.accounts.filter((a) => !a.hidden);
  const onBudget = open.filter((a) => a.onBudget);
  const offBudget = open.filter((a) => !a.onBudget);
  const closed = state.accounts.filter((a) => a.hidden);
  const total = (list: AccountDTO[]) => round(list.reduce((s, a) => s + (a.balances?.working ?? 0), 0));
  const active = (id: string | null) => view.kind === 'account' && view.accountId === id;
  const sync = state.sync;

  const ctx: RowContext = {
    selectedId: view.kind === 'account' ? view.accountId : null,
    drag,
    setDrag,
    move: (id, beforeId) => {
      if (beforeId !== id) void run(() => api.moveAccount(id, beforeId));
    },
    select: (id) => setView({ kind: 'account', accountId: id }),
  };

  return (
    <aside className="sidebar">
      <div className="brand" title={state.budget.folder}>
        <img src="/favicon.svg" alt="" width={22} height={22} />
        <span>{state.budget.name}</span>
      </div>
      <nav className="nav">
        <button className={`nav-item ${view.kind === 'budget' ? 'active' : ''}`} onClick={() => setView({ kind: 'budget' })}>
          Budget
        </button>
        <button className={`nav-item ${active(null) ? 'active' : ''}`} onClick={() => setView({ kind: 'account', accountId: null })}>
          All accounts
        </button>
        <button className={`nav-item ${view.kind === 'scheduled' ? 'active' : ''}`} onClick={() => setView({ kind: 'scheduled' })}>
          Scheduled
        </button>
        <button className={`nav-item ${view.kind === 'payees' ? 'active' : ''}`} onClick={() => setView({ kind: 'payees' })}>
          Payees
        </button>
      </nav>

      <div className="acct-group">
        <div className="acct-head">
          <span>Budget accounts</span>
          <span className="amount">{money(total(onBudget))}</span>
        </div>
        {onBudget.map((a) => (
          <AccountRow key={a.id} a={a} group={onBudget} ctx={ctx} />
        ))}
      </div>
      {offBudget.length > 0 && (
        <div className="acct-group">
          <div className="acct-head">
            <span>Off-budget</span>
            <span className="amount">{money(total(offBudget))}</span>
          </div>
          {offBudget.map((a) => (
            <AccountRow key={a.id} a={a} group={offBudget} ctx={ctx} />
          ))}
        </div>
      )}
      {closed.length > 0 && (
        <div className="acct-group">
          <button className="acct-head toggle" onClick={() => setShowClosed(!showClosed)}>
            <span>
              {showClosed ? '▾' : '▸'} Closed ({closed.length})
            </span>
          </button>
          {showClosed && closed.map((a) => <AccountRow key={a.id} a={a} group={closed} ctx={ctx} />)}
        </div>
      )}
      <button className="add-account" onClick={() => setAdding(true)}>
        + Add account
      </button>

      <div className="sidebar-foot">
        <div className={`sync ${sync.lastError ? 'bad' : sync.blockedDiffs ? 'warn' : 'ok'}`} title={syncTitle(state)}>
          <span className="dot" />
          {sync.lastError
            ? 'Sync problem'
            : sync.blockedDiffs
              ? `Waiting on ${sync.blockedDiffs} change file(s)`
              : sync.unpublished
                ? 'Saving…'
                : `Synced ${relativeTime(sync.lastSync)}`}
        </div>
        <div className="foot-actions">
          <button
            className="link small"
            disabled={!state.undo.undo}
            title={state.undo.undo ? `Undo ${state.undo.undo} (Ctrl+Z)` : 'Nothing to undo'}
            onClick={() => void undo(false)}
          >
            Undo
          </button>
          <button
            className="link small"
            disabled={!state.undo.redo}
            title={state.undo.redo ? `Redo ${state.undo.redo} (Ctrl+Y)` : 'Nothing to redo'}
            onClick={() => void undo(true)}
          >
            Redo
          </button>
          <button className="link small" onClick={() => run(() => api.sync())}>
            Sync now
          </button>
          <button className={`link small ${view.kind === 'settings' ? 'active' : ''}`} onClick={() => setView({ kind: 'settings' })}>
            Settings
          </button>
          <button className="link small" onClick={() => run(() => api.close())}>
            Close
          </button>
        </div>
      </div>
      {adding && <AccountDialog onClose={() => setAdding(false)} />}
    </aside>
  );
}

interface DragState {
  id: string;
  over: string | null;
  pos: 'before' | 'after';
}

interface RowContext {
  selectedId: string | null;
  drag: DragState | null;
  setDrag: (d: DragState | null) => void;
  move: (id: string, beforeId: string | null) => void;
  select: (id: string) => void;
}

/** One account in the sidebar. Drag it within its own group (budget or off-budget), or use Alt+Up / Alt+Down. */
function AccountRow({ a, group, ctx }: { a: AccountDTO; group: AccountDTO[]; ctx: RowContext }) {
  const { drag, setDrag, move } = ctx;
  const idx = group.findIndex((x) => x.id === a.id);
  const dropPos = drag && drag.over === a.id ? drag.pos : null;
  const sameGroup = !!drag && group.some((x) => x.id === drag.id);
  // The account is placed before `a`, or before the account that follows it in this group.
  const beforeFor = (pos: 'before' | 'after') => (pos === 'before' ? a.id : (group[idx + 1]?.id ?? null));
  return (
    <div
      className={`acct-drag ${dropPos ? `drop-${dropPos}` : ''} ${drag?.id === a.id ? 'dragging' : ''}`}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', a.id);
        setDrag({ id: a.id, over: null, pos: 'before' });
      }}
      onDragEnd={() => setDrag(null)}
      onDragOver={(e) => {
        if (!drag || !sameGroup) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        const r = e.currentTarget.getBoundingClientRect();
        const pos = e.clientY < r.top + r.height / 2 ? 'before' : 'after';
        if (drag.over !== a.id || drag.pos !== pos) setDrag({ id: drag.id, over: a.id, pos });
      }}
      onDrop={(e) => {
        e.preventDefault();
        if (!drag || drag.id === a.id) {
          setDrag(null);
          return;
        }
        move(drag.id, beforeFor(drag.over === a.id ? drag.pos : 'before'));
        setDrag(null);
      }}
    >
      <button
        className={`nav-account ${ctx.selectedId === a.id ? 'active' : ''}`}
        onClick={() => ctx.select(a.id)}
        onKeyDown={(e) => {
          if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
          e.preventDefault();
          if (e.key === 'ArrowUp' && idx > 0) move(a.id, group[idx - 1].id);
          if (e.key === 'ArrowDown' && idx < group.length - 1) move(a.id, group[idx + 2]?.id ?? null);
        }}
        title={`${a.name}. Drag to reorder, or Alt+Up / Alt+Down.`}
      >
        <span className="acct-name">
          {a.name}
          {a.unapproved > 0 && <span className="badge" title={`${a.unapproved} unapproved`}>{a.unapproved}</span>}
        </span>
        <span className={`amount ${a.balances.working < 0 ? 'neg' : ''}`}>{money(a.balances.working)}</span>
      </button>
    </div>
  );
}

function syncTitle(state: NonNullable<ReturnType<typeof useApp>['state']>): string {
  const s = state.sync;
  const lines = [
    `This computer: device ${state.budget.device.id} (${state.budget.device.name})`,
    ...state.budget.otherDevices.map((d) => `Device ${d.id}: ${d.name}`),
    `Last sync: ${relativeTime(s.lastSync)}`,
    `Full save: ${relativeTime(s.lastFullSave)}`,
  ];
  if (s.conflictsResolved) lines.push(`${s.conflictsResolved} conflicting edit(s) resolved`);
  if (s.lastError) lines.push(`Error: ${s.lastError}`);
  return lines.join('\n');
}

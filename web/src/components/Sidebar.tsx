import { useState } from 'react';
import { api, type AccountDTO } from '../api.ts';
import { money, relativeTime, round } from '../format.ts';
import { useApp } from '../state.tsx';
import { AccountDialog } from './AccountDialog.tsx';

export function Sidebar() {
  const { state, view, setView, run, undo } = useApp();
  const [adding, setAdding] = useState(false);
  const [showClosed, setShowClosed] = useState(false);
  if (!state) return null;

  const open = state.accounts.filter((a) => !a.hidden);
  const onBudget = open.filter((a) => a.onBudget);
  const offBudget = open.filter((a) => !a.onBudget);
  const closed = state.accounts.filter((a) => a.hidden);
  const total = (list: AccountDTO[]) => round(list.reduce((s, a) => s + (a.balances?.working ?? 0), 0));
  const active = (id: string | null) => view.kind === 'account' && view.accountId === id;
  const sync = state.sync;

  const AccountRow = ({ a }: { a: AccountDTO }) => (
    <button className={`nav-account ${active(a.id) ? 'active' : ''}`} onClick={() => setView({ kind: 'account', accountId: a.id })} title={a.name}>
      <span className="acct-name">
        {a.name}
        {a.unapproved > 0 && <span className="badge" title={`${a.unapproved} unapproved`}>{a.unapproved}</span>}
      </span>
      <span className={`amount ${a.balances.working < 0 ? 'neg' : ''}`}>{money(a.balances.working)}</span>
    </button>
  );

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
      </nav>

      <div className="acct-group">
        <div className="acct-head">
          <span>Budget accounts</span>
          <span className="amount">{money(total(onBudget))}</span>
        </div>
        {onBudget.map((a) => (
          <AccountRow key={a.id} a={a} />
        ))}
      </div>
      {offBudget.length > 0 && (
        <div className="acct-group">
          <div className="acct-head">
            <span>Off-budget</span>
            <span className="amount">{money(total(offBudget))}</span>
          </div>
          {offBudget.map((a) => (
            <AccountRow key={a.id} a={a} />
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
          {showClosed && closed.map((a) => <AccountRow key={a.id} a={a} />)}
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

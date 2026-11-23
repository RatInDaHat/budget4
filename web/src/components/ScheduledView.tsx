import { useEffect, useMemo, useState } from 'react';
import { api, type ScheduledDTO } from '../api.ts';
import { formatDate, plain } from '../format.ts';
import { useApp } from '../state.tsx';
import { TxnEditor } from './TxnEditor.tsx';

const FREQUENCIES: [string, string][] = [
  ['Once', 'Once'],
  ['Daily', 'Daily'],
  ['Weekly', 'Weekly'],
  ['EveryOtherWeek', 'Every other week'],
  ['TwiceAMonth', 'Twice a month'],
  ['Every4Weeks', 'Every 4 weeks'],
  ['Monthly', 'Monthly'],
  ['EveryOtherMonth', 'Every other month'],
  ['Every3Months', 'Every 3 months'],
  ['Every4Months', 'Every 4 months'],
  ['TwiceAYear', 'Twice a year'],
  ['Yearly', 'Yearly'],
  ['EveryOtherYear', 'Every other year'],
];
const label = (f: string) => FREQUENCIES.find(([v]) => v === f)?.[1] ?? f;
const frequencyRank = (f: string) => {
  const i = FREQUENCIES.findIndex(([v]) => v === f);
  return i < 0 ? FREQUENCIES.length : i;
};

type Col = 'date' | 'frequency' | 'account' | 'payee' | 'category' | 'memo' | 'amount';
interface Sort {
  col: Col;
  desc: boolean;
}
const SORT_KEY = 'budget4.scheduled.sort';

function loadSort(): Sort {
  try {
    const v = localStorage.getItem(SORT_KEY);
    if (v) return { col: 'date', desc: false, ...JSON.parse(v) };
  } catch {
    /* default */
  }
  return { col: 'date', desc: false };
}

/** One scheduled transaction as seen from an account (a transfer looks reversed from its other side). */
interface Row {
  s: ScheduledDTO;
  accountId: string;
  payee: string;
  category: string;
  amount: number;
}

export function ScheduledView() {
  return (
    <div className="register">
      <ScheduledTable accountId={null} />
    </div>
  );
}

/** Scheduled transactions, sortable by any column. With an account, only the ones that touch it. */
export function ScheduledTable({ accountId, onCount }: { accountId: string | null; onCount?: (n: number) => void }) {
  const { version, run, refresh, lookups } = useApp();
  const [all, setAll] = useState<ScheduledDTO[] | null>(null);
  const [editing, setEditing] = useState<ScheduledDTO | 'new' | null>(null);
  const [frequency, setFrequency] = useState('Monthly');
  const [sort, setSort] = useState<Sort>(loadSort);

  useEffect(() => {
    api.scheduled().then(setAll).catch(() => {});
  }, [version]);

  useEffect(() => {
    setFrequency(editing && editing !== 'new' ? editing.frequency : 'Monthly');
  }, [editing]);

  useEffect(() => {
    try {
      localStorage.setItem(SORT_KEY, JSON.stringify(sort));
    } catch {
      /* not remembered; fine */
    }
  }, [sort]);

  const rows = useMemo(() => {
    if (!all) return null;
    const out: Row[] = [];
    for (const s of all) {
      const target = s.targetAccountId ? lookups.accounts.get(s.targetAccountId) : undefined;
      const category = s.isSplit ? `Split (${s.splits.length})` : lookups.categoryLabel(s.categoryId);
      if (!accountId || s.accountId === accountId) {
        out.push({ s, accountId: s.accountId, payee: target ? `Transfer : ${target.name}` : lookups.payeeLabel(s.payeeId), category, amount: s.amount });
      } else if (s.targetAccountId === accountId) {
        // The other side of a scheduled transfer.
        const from = lookups.accounts.get(s.accountId);
        out.push({ s, accountId, payee: `Transfer : ${from?.name ?? ''}`, category: '', amount: -s.amount });
      }
    }
    const dir = sort.desc ? -1 : 1;
    const text = (f: (r: Row) => string) => (a: Row, b: Row) => {
      const x = f(a).toLocaleLowerCase();
      const y = f(b).toLocaleLowerCase();
      if (!x !== !y) return x ? -1 : 1; // blanks last either way
      return dir * x.localeCompare(y);
    };
    const primary = {
      date: (a: Row, b: Row) => dir * a.s.date.localeCompare(b.s.date),
      frequency: (a: Row, b: Row) => dir * (frequencyRank(a.s.frequency) - frequencyRank(b.s.frequency)),
      account: text((r) => lookups.accounts.get(r.accountId)?.name ?? ''),
      payee: text((r) => r.payee),
      category: text((r) => r.category),
      memo: text((r) => r.s.memo ?? ''),
      amount: (a: Row, b: Row) => dir * (a.amount - b.amount),
    }[sort.col];
    return out.sort((a, b) => primary(a, b) || a.s.date.localeCompare(b.s.date) || a.payee.localeCompare(b.payee));
  }, [all, accountId, lookups, sort]);

  const count = rows?.length ?? 0;
  useEffect(() => onCount?.(count), [count, onCount]);

  const th = (col: Col, text: string, cls = '') => {
    const active = sort.col === col;
    return (
      <th
        className={`sortable ${cls} ${active ? 'active' : ''}`}
        aria-sort={active ? (sort.desc ? 'descending' : 'ascending') : undefined}
        title={`Sort by ${text.toLowerCase()}`}
        onClick={() => setSort((s) => (s.col === col ? { col, desc: !s.desc } : { col, desc: false }))}
      >
        {text}
        {active && <span className="sort-arrow">{sort.desc ? ' ▼' : ' ▲'}</span>}
      </th>
    );
  };

  const editor = editing && (
    <div className="editor-wrap">
      <h4>{editing === 'new' ? 'New scheduled transaction' : 'Edit scheduled transaction'}</h4>
      <TxnEditor
        key={editing === 'new' ? 'new' : editing.id}
        txn={editing === 'new' ? null : editing}
        defaultAccountId={accountId}
        onCancel={() => setEditing(null)}
        onSave={async (input) => {
          await api.saveScheduled({ ...input, frequency });
          setEditing(null);
          await refresh();
        }}
        extra={
          <label className="inline">
            Repeats
            <select value={frequency} onChange={(e) => setFrequency(e.target.value)}>
              {FREQUENCIES.map(([v, l]) => (
                <option key={v} value={v}>
                  {l}
                </option>
              ))}
            </select>
          </label>
        }
      />
    </div>
  );

  const table =
    rows === null ? (
      <div className="empty">Loading…</div>
    ) : rows.length === 0 ? (
      <div className="empty">{accountId ? 'Nothing scheduled for this account.' : 'Nothing scheduled.'}</div>
    ) : (
      <table className="table">
        <thead>
          <tr>
            {th('date', 'Next date')}
            {th('frequency', 'Repeats')}
            {!accountId && th('account', 'Account')}
            {th('payee', 'Payee')}
            {th('category', 'Category')}
            {th('memo', 'Memo')}
            {th('amount', 'Amount', 'num')}
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={`${r.s.id}:${r.accountId}`} onDoubleClick={() => setEditing(r.s)}>
              <td>{formatDate(r.s.date)}</td>
              <td>{label(r.s.frequency)}</td>
              {!accountId && <td>{lookups.accounts.get(r.accountId)?.name}</td>}
              <td>{r.payee}</td>
              <td>{r.category}</td>
              <td className="muted ellipsis-cell" title={r.s.memo ?? ''}>
                {r.s.memo}
              </td>
              <td className={`num ${r.amount < 0 ? 'neg' : ''}`}>{plain(r.amount)}</td>
              <td className="actions">
                <button className="small" onClick={() => run(() => api.enterScheduled(r.s.id))} title="Enter the next occurrence now">
                  Enter now
                </button>
                <button className="small" onClick={() => run(() => api.skipScheduled(r.s.id))} title="Skip the next occurrence">
                  Skip
                </button>
                <button className={accountId ? 'icon' : 'small'} onClick={() => setEditing(r.s)} title="Edit" aria-label="Edit">
                  {accountId ? '✎' : 'Edit'}
                </button>
                <button
                  className={accountId ? 'icon danger-icon' : 'small danger-btn'}
                  onClick={() => confirm('Delete this scheduled transaction?') && run(() => api.deleteScheduled(r.s.id))}
                  title="Delete"
                  aria-label="Delete"
                >
                  {accountId ? '×' : 'Delete'}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    );

  if (accountId) {
    return (
      <>
        <div className="sched-panel-actions">
          <button className="small" onClick={() => setEditing('new')}>
            + Scheduled transaction
          </button>
        </div>
        {editor}
        <div className="sched-list">{table}</div>
      </>
    );
  }
  return (
    <>
      <header className="toolbar">
        <div>
          <h2>Scheduled transactions</h2>
          <p className="muted small">Entered automatically on their date, waiting for your approval in the account.</p>
        </div>
        <button className="primary" onClick={() => setEditing('new')}>
          + Scheduled transaction
        </button>
      </header>
      {editor}
      <div className="sched-list">{table}</div>
    </>
  );
}

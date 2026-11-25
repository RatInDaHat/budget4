import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { api, type TxnDTO } from '../api.ts';
import { formatDate, money, parseAmount, plain, today } from '../format.ts';
import { useApp } from '../state.tsx';
import { AccountDialog } from './AccountDialog.tsx';
import { ImportDialog } from './ImportDialog.tsx';
import { InlineRow, type Field } from './InlineRow.tsx';
import { Modal } from './Modal.tsx';
import { RegisterHead, sortRows, useRegisterLayout } from './RegisterColumns.tsx';
import { ScheduledTable } from './ScheduledView.tsx';
import { TxnEditor } from './TxnEditor.tsx';

const ROW = 34;
const HEAD = 32;
/** Pinned above the rows: the column headings and the new-transaction row. */
const LEAD = HEAD + ROW;

type Lookups = ReturnType<typeof useApp>['lookups'];

function payeeText(t: TxnDTO, lookups: Lookups): string {
  const target = t.targetAccountId ? lookups.accounts.get(t.targetAccountId) : undefined;
  return target ? `Transfer : ${target.name}` : lookups.payeeLabel(t.payeeId) || t.importedPayee || '';
}

function categoryText(t: TxnDTO, lookups: Lookups): string {
  if (t.isSplit) return `Split (${t.splits.length})`;
  if (t.categoryId) return lookups.categoryLabel(t.categoryId);
  if (t.targetAccountId) return '';
  return t.amount !== 0 && lookups.accounts.get(t.accountId)?.onBudget ? '⚠ Needs a category' : '';
}

export function Register({ accountId }: { accountId: string | null }) {
  const { state, version, run, refresh, lookups, setView } = useApp();
  const [newKey, setNewKey] = useState(0);
  const [rows, setRows] = useState<TxnDTO[] | null>(null);
  const [editing, setEditingState] = useState<TxnDTO | 'new' | null>(null);
  // The row being edited in place, and the cell that was clicked.
  const [inline, setInline] = useState<{ id: string; focus: Field } | null>(null);
  const setEditing = (e: TxnDTO | 'new' | null) => {
    setEditingState(e);
    if (e) setInline(null);
  };
  /** Leaves in-place editing of `id` (unless another row has started editing since). */
  const finishInline = (id: string) => setInline((cur) => (cur?.id === id ? null : cur));
  // The entry row for a new transaction: closed, or open on a cell. `n` remounts it blank after
  // each add; account and date carry over to the next one.
  const [entry, setEntry] = useState<{ focus: Field; n: number; accountId: string | null; date: string } | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [search, setSearch] = useState('');
  const [hideReconciled, setHideReconciled] = useState(false);
  const [dialog, setDialog] = useState<'reconcile' | 'account' | 'import' | null>(null);
  const account = accountId ? lookups.accounts.get(accountId) : undefined;
  const layout = useRegisterLayout(!accountId);

  useEffect(() => {
    let live = true;
    api
      .transactions(accountId)
      .then((r) => live && setRows(r))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [accountId, version]);

  const filtered = useMemo(() => {
    if (!rows) return [];
    const q = search.trim().toLowerCase();
    const amountQ = q ? parseAmount(q) : null;
    const out = rows.filter((t) => {
      if (hideReconciled && t.cleared === 'Reconciled') return false;
      if (!q) return true;
      if (amountQ !== null && amountQ !== 0 && Math.abs(t.amount) === Math.abs(amountQ)) return true;
      return (
        lookups.payeeLabel(t.payeeId).toLowerCase().includes(q) ||
        lookups.categoryLabel(t.categoryId).toLowerCase().includes(q) ||
        (t.memo ?? '').toLowerCase().includes(q) ||
        t.splits.some((s) => lookups.categoryLabel(s.categoryId).toLowerCase().includes(q) || (s.memo ?? '').toLowerCase().includes(q))
      );
    });
    return sortRows(out, layout.sort, {
      payee: (t) => payeeText(t, lookups),
      category: (t) => categoryText(t, lookups),
      account: (t) => lookups.accounts.get(t.accountId)?.name ?? '',
    });
  }, [rows, search, hideReconciled, lookups, layout.sort]);

  // Sorted by date, the register opens on the newest end. Re-applied when the date sort direction changes.
  const jump = useMemo(() => {
    if (layout.sort.col !== 'date' || filtered.length === 0) return null;
    const desc = layout.sort.desc;
    // Always open on the newest end, so future (italic) rows and the entry row are in view:
    // oldest first shows the bottom, newest first shows the top (where the entry row is pinned).
    if (!desc) return { key: 'date:false', index: filtered.length - 1, align: 'bottom' as const };
    return { key: 'date:true', index: 0, align: 'start' as const };
  }, [filtered, layout.sort]);

  const unapproved = useMemo(() => (rows ?? []).filter((t) => !t.accepted).map((t) => t.id), [rows]);
  const matchedCount = useMemo(() => (rows ?? []).filter((t) => !t.accepted && t.matched).length, [rows]);
  if (!state) return null;
  if (accountId && !account) return <div className="empty">This account no longer exists.</div>;

  const toggleSelect = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };
  const sel = [...selected];

  return (
    <div className="register" style={layout.style}>
      <header className="toolbar">
        <div>
          <h2>
            {account ? account.name : 'All accounts'}
            {account?.hidden && <span className="tag">Closed</span>}
            {account && !account.onBudget && <span className="tag">Off-budget</span>}
          </h2>
          {account && (
            <div className="balances">
              <span>
                Cleared <b className={account.balances.cleared < 0 ? 'neg' : ''}>{money(account.balances.cleared)}</b>
              </span>
              <span>+</span>
              <span>
                Uncleared <b className={account.balances.uncleared < 0 ? 'neg' : ''}>{money(account.balances.uncleared)}</b>
              </span>
              <span>=</span>
              <span>
                Working balance <b className={account.balances.working < 0 ? 'neg' : ''}>{money(account.balances.working)}</b>
              </span>
            </div>
          )}
        </div>
        <div className="row">
          <input className="search" placeholder="Search payee, category, memo or amount" value={search} onChange={(e) => setSearch(e.target.value)} />
          <label className="check small">
            <input type="checkbox" checked={hideReconciled} onChange={(e) => setHideReconciled(e.target.checked)} /> Hide reconciled
          </label>
          {account && (
            <>
              <button onClick={() => setDialog('reconcile')}>Reconcile</button>
              <button onClick={() => setDialog('account')}>Edit account</button>
            </>
          )}
          <button onClick={() => setDialog('import')}>Import…</button>
          <button className="primary" onClick={() => setEditing('new')}>
            + Transaction
          </button>
        </div>
      </header>

      {unapproved.length > 0 && (
        <div className="banner">
          {unapproved.length} transaction{unapproved.length > 1 ? 's' : ''} entered automatically (scheduled or imported) need a look.
          {matchedCount > 0 && ` ${matchedCount} ${matchedCount > 1 ? 'were' : 'was'} matched to transactions you entered.`}
          <button className="link" onClick={() => run(() => api.approve(unapproved))}>
            Approve all
          </button>
        </div>
      )}

      {editing && (
        <div className="editor-wrap">
          <h4>{editing === 'new' ? 'New transaction' : 'Edit transaction'}</h4>
          <TxnEditor
            key={editing === 'new' ? `new-${newKey}` : editing.id}
            txn={editing === 'new' ? null : editing}
            defaultAccountId={accountId}
            onCancel={() => setEditing(null)}
            onSave={async (input) => {
              await api.saveTransaction(input);
              // Keep the form open for the next new transaction; close it after an edit.
              if (editing === 'new') setNewKey((k) => k + 1);
              else setEditing(null);
              await refresh();
            }}
            saveLabel={editing === 'new' ? 'Add' : 'Save'}
            extra={
              editing !== 'new' && (
                <button
                  className="danger-btn"
                  onClick={() => {
                    const id = editing.id;
                    setEditing(null);
                    void run(() => api.deleteTransactions([id]));
                  }}
                >
                  Delete
                </button>
              )
            }
          />
        </div>
      )}

      {sel.length > 0 && (
        <div className="bulk">
          <span>{sel.length} selected</span>
          <button onClick={() => run(() => api.toggleCleared(sel))}>Toggle cleared</button>
          <button onClick={() => run(() => api.approve(sel))}>Approve</button>
          <button
            className="danger-btn"
            onClick={() => {
              if (confirm(`Delete ${sel.length} transaction(s)?`)) {
                setSelected(new Set());
                void run(() => api.deleteTransactions(sel));
              }
            }}
          >
            Delete
          </button>
          <button className="link" onClick={() => setSelected(new Set())}>
            Clear selection
          </button>
        </div>
      )}

      <VirtualRows
        head={<RegisterHead cols={layout.cols} sort={layout.sort} setSort={layout.setSort} setWidth={layout.setWidth} balanceLabel={!!accountId} />}
        minWidth={layout.minWidth}
        empty={rows === null ? 'Loading…' : filtered.length === 0 ? (search ? 'No matching transactions.' : 'No transactions yet.') : null}
        rows={filtered}
        jump={jump}
        entryAtEnd={layout.sort.col === 'date' && !layout.sort.desc}
        entry={
          entry ? (
            <InlineRow
              key={`new-${entry.n}`}
              t={null}
              accountId={entry.accountId}
              date={entry.date}
              focus={entry.focus}
              showAccount={!accountId}
              onDone={() => setEntry(null)}
              onAdded={(acct, date) => setEntry((e) => ({ focus: 'payee', n: (e?.n ?? 0) + 1, accountId: accountId ? accountId : acct, date }))}
              onFullEdit={() => {
                setEntry(null);
                setEditing('new');
              }}
            />
          ) : (
            <EntryPrompt showAccount={!accountId} onOpen={(focus) => setEntry({ focus, n: 0, accountId, date: today() })} />
          )
        }
        render={(t) =>
          inline?.id === t.id ? (
            <InlineRow
              key={t.id}
              t={t}
              focus={inline.focus}
              showAccount={!accountId}
              onDone={() => finishInline(t.id)}
              onFullEdit={() => setEditing(t)}
              onToggleCleared={() => run(() => api.toggleCleared([t.id]))}
            />
          ) : (
            <TxnRow
              key={t.id}
              t={t}
              showAccount={!accountId}
              selected={selected.has(t.id)}
              editing={editing !== 'new' && editing?.id === t.id}
              onSelect={() => toggleSelect(t.id)}
              onEdit={() => setEditing(t)}
              onCellClick={(focus) => setInline({ id: t.id, focus })}
              onToggleCleared={() => run(() => api.toggleCleared([t.id]))}
              onApprove={() => run(() => api.approve([t.id]))}
              onUnmatch={() => run(() => api.unmatch(t.id))}
              onOpenAccount={(id) => setView({ kind: 'account', accountId: id })}
            />
          )
        }
      />

      {accountId && <ScheduledPanel accountId={accountId} />}

      {dialog === 'account' && account && <AccountDialog account={account} onClose={() => setDialog(null)} />}
      {dialog === 'import' && <ImportDialog defaultAccountId={accountId} onClose={() => setDialog(null)} />}
      {dialog === 'reconcile' && account && <ReconcileDialog accountId={account.id} cleared={account.balances.cleared} onClose={() => setDialog(null)} />}
    </div>
  );
}

const PANEL_KEY = 'budget4.register.scheduledOpen';

/** This account's scheduled transactions, above its register; open or closed is remembered. */
function ScheduledPanel({ accountId }: { accountId: string }) {
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(PANEL_KEY) !== '0';
    } catch {
      return true;
    }
  });
  const [count, setCount] = useState<number | null>(null);
  const toggle = () => {
    setOpen(!open);
    try {
      localStorage.setItem(PANEL_KEY, open ? '0' : '1');
    } catch {
      /* not remembered; fine */
    }
  };
  return (
    <section className={`sched-panel ${open ? 'open' : ''}`}>
      <button className="sched-panel-head" onClick={toggle} aria-expanded={open}>
        <span className="caret">{open ? '▾' : '▸'}</span> Scheduled
        {count !== null && ` (${count})`}
      </button>
      <div hidden={!open}>
        <ScheduledTable accountId={accountId} onCount={setCount} />
      </div>
    </section>
  );
}

/** The resting entry row: click a cell to start a new transaction there. */
function EntryPrompt({ showAccount, onOpen }: { showAccount: boolean; onOpen: (focus: Field) => void }) {
  const cell = (field: Field) => ({ onClick: () => onOpen(field) });
  return (
    <div className="reg-group new-entry">
      <div className={`reg-row editable ${showAccount ? 'with-account' : ''}`} title="Add a transaction">
        <span className="center muted">+</span>
        {showAccount && <span {...cell('payee')} />}
        <span className="muted" {...cell('date')}>
          {formatDate(today())}
        </span>
        <span className="muted" {...cell('payee')}>
          Add a transaction…
        </span>
        <span {...cell('category')} />
        <span {...cell('memo')} />
        <span {...cell('outflow')} />
        <span {...cell('inflow')} />
        <span />
        <span />
      </div>
    </div>
  );
}

function VirtualRows({
  rows,
  render,
  head,
  minWidth,
  empty,
  jump,
  entry,
  entryAtEnd,
}: {
  rows: TxnDTO[];
  jump: { key: string; index: number; align: 'start' | 'end' | 'bottom' } | null;
  /** The new-transaction row: pinned under the headings, or after the last row when sorted by date ascending. */
  entry: React.ReactNode;
  /** Put the entry row after the rows (newest last) instead of pinning it under the headings. */
  entryAtEnd: boolean;
  render: (t: TxnDTO) => React.ReactNode;
  head: React.ReactNode;
  minWidth: number;
  empty: string | null;
}) {
  // Rows start below the headings, and below the entry row too when it is pinned.
  const lead = entryAtEnd ? HEAD : LEAD;
  const ref = useRef<HTMLDivElement>(null);
  const [scroll, setScroll] = useState(0);
  const [height, setHeight] = useState(600);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const jumped = useRef<string | null>(null);
  const heights = rows.map((t) => ROW * (1 + (t.isSplit ? t.splits.length : 0)));
  const offsets: number[] = [];
  let total = 0;
  for (const h of heights) {
    offsets.push(total);
    total += h;
  }
  // Scroll to `jump.index` once per jump key (not on every refresh or search).
  useLayoutEffect(() => {
    const el = ref.current;
    if (!jump && rows.length) jumped.current = null; // sorted by another column: jump again on returning to date
    if (!el || !jump || jumped.current === jump.key || jump.index >= offsets.length) return;
    jumped.current = jump.key;
    // Rows start below the pinned headings and entry row, which also cover the top of the view.
    const top = lead + offsets[jump.index];
    el.scrollTop =
      jump.align === 'bottom' ? el.scrollHeight : jump.align === 'start' ? top - lead : top + heights[jump.index] - el.clientHeight;
    setScroll(el.scrollTop);
  });
  const scrollRows = Math.max(0, scroll - lead);
  let start = 0;
  let lo = 0;
  let hi = rows.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (offsets[mid] + heights[mid] < scrollRows) lo = mid + 1;
    else hi = mid - 1;
  }
  start = Math.max(0, lo - 5);
  let end = start;
  while (end < rows.length && offsets[end] < scrollRows + height + ROW * 5) end++;
  return (
    <div className="reg-body" ref={ref} onScroll={(e) => setScroll((e.target as HTMLDivElement).scrollTop)}>
      <div className="reg-scroll" style={{ minWidth }}>
        {head}
        {!entryAtEnd && <div className="reg-entry">{entry}</div>}
        {empty !== null ? (
          <div className="empty">{empty}</div>
        ) : (
          <div style={{ height: total, position: 'relative' }}>
            <div
              style={{
                position: 'absolute',
                top: offsets[start] ?? 0,
                left: 0,
                right: 0,
              }}
            >
              {rows.slice(start, end).map(render)}
            </div>
          </div>
        )}
        {entryAtEnd && <div className="reg-entry-end">{entry}</div>}
      </div>
    </div>
  );
}

function TxnRow({
  t,
  showAccount,
  selected,
  editing,
  onSelect,
  onEdit,
  onCellClick,
  onToggleCleared,
  onApprove,
  onUnmatch,
  onOpenAccount,
}: {
  t: TxnDTO;
  showAccount: boolean;
  selected: boolean;
  editing: boolean;
  onSelect: () => void;
  onEdit: () => void;
  onCellClick: (field: Field) => void;
  onToggleCleared: () => void;
  onApprove: () => void;
  onUnmatch: () => void;
  onOpenAccount: (id: string) => void;
}) {
  const { lookups } = useApp();
  const target = t.targetAccountId ? lookups.accounts.get(t.targetAccountId) : undefined;
  const payee = payeeText(t, lookups);
  const category = categoryText(t, lookups);
  const future = t.date > today();
  const matchTip = t.matched
    ? `Matched: your entry ${lookups.payeeLabel(t.matched.payeeId) || ''} on ${formatDate(t.matched.date)}` +
      (t.matched.bankPayee || t.matched.bankDate
        ? ` · the bank's: ${t.matched.bankPayee ?? ''}${t.matched.bankDate ? ` on ${formatDate(t.matched.bankDate)}` : ''}`
        : '')
    : t.importedPayee
      ? `Imported as: ${t.importedPayee}`
      : payee;
  // A click on a cell edits the row in place (buttons and links inside cells keep their own job).
  const cell = (field: Field) => ({
    onClick: (e: React.MouseEvent) => {
      if ((e.target as HTMLElement).closest('button, a, input')) return;
      onCellClick(field);
    },
  });
  return (
    <div className={`reg-group ${selected ? 'selected' : ''} ${editing ? 'editing' : ''} ${!t.accepted ? 'unapproved' : ''} ${future ? 'future' : ''}`}>
      <div className={`reg-row editable ${showAccount ? 'with-account' : ''}`}>
        <span>
          <input type="checkbox" checked={selected} onChange={onSelect} aria-label="Select" />
        </span>
        {showAccount && <span className="ellipsis">{lookups.accounts.get(t.accountId)?.name}</span>}
        <span {...cell('date')}>{formatDate(t.date)}</span>
        <span className="ellipsis" title={matchTip} {...cell('payee')}>
          {t.matched && <span className="tag match-tag">Matched</span>}
          {target ? (
            <button className="link" onClick={() => onOpenAccount(target.id)}>
              {payee}
            </button>
          ) : (
            payee
          )}
        </span>
        <span className={`ellipsis ${category.startsWith('⚠') ? 'warn-text' : ''}`} title={category} {...(t.isSplit ? { onClick: onEdit } : cell('category'))}>
          {category}
        </span>
        <span className="ellipsis muted" title={t.memo ?? ''} {...cell('memo')}>
          {t.memo}
        </span>
        <span className="num" {...cell('outflow')}>
          {t.amount < 0 ? plain(-t.amount) : ''}
        </span>
        <span className="num" {...cell('inflow')}>
          {t.amount > 0 ? plain(t.amount) : ''}
        </span>
        <span className="center">
          <button className={`cleared-btn ${t.cleared}`} onClick={onToggleCleared} disabled={t.cleared === 'Reconciled'} title={t.cleared}>
            {t.cleared === 'Reconciled' ? 'R' : 'C'}
          </button>
        </span>
        <span className="num row-end">
          {!t.accepted ? (
            <>
              {t.matched && (
                <button className="small" onClick={onUnmatch} title="Split back into your entry and the bank's transaction">
                  Unmatch
                </button>
              )}
              <button className="small approve" onClick={onApprove}>
                Approve
              </button>
            </>
          ) : t.runningBalance !== null && !showAccount ? (
            <span className={t.runningBalance < 0 ? 'neg' : ''}>{plain(t.runningBalance)}</span>
          ) : null}
          <button className="icon edit-btn" onClick={onEdit} aria-label="Edit">
            ✎
          </button>
        </span>
      </div>
      {t.isSplit &&
        t.splits.map((s, i) => {
          const st = s.targetAccountId ? lookups.accounts.get(s.targetAccountId) : undefined;
          return (
            <div key={s.id ?? i} className={`reg-row split ${showAccount ? 'with-account' : ''}`}>
              <span />
              {showAccount && <span />}
              <span />
              <span className="ellipsis muted">{st ? `Transfer : ${st.name}` : ''}</span>
              <span className="ellipsis">{st ? '' : lookups.categoryLabel(s.categoryId)}</span>
              <span className="ellipsis muted">{s.memo}</span>
              <span className="num">{s.amount < 0 ? plain(-s.amount) : ''}</span>
              <span className="num">{s.amount > 0 ? plain(s.amount) : ''}</span>
              <span />
              <span />
            </div>
          );
        })}
    </div>
  );
}

function ReconcileDialog({ accountId, cleared, onClose }: { accountId: string; cleared: number; onClose: () => void }) {
  const { run } = useApp();
  const [balance, setBalance] = useState(plain(cleared));
  const [date, setDate] = useState(today());
  const v = parseAmount(balance);
  const diff = v === null ? null : Math.round((v - cleared) * 100) / 100;
  return (
    <Modal title="Reconcile account" onClose={onClose}>
      <div className="form">
        <p>Enter the balance your bank shows for cleared transactions.</p>
        <div className="row">
          <label className="grow">
            Statement balance
            <input autoFocus value={balance} onChange={(e) => setBalance(e.target.value)} inputMode="decimal" />
          </label>
          <label>
            As of
            <input type="date" value={date} onChange={(e) => setDate(e.target.value)} />
          </label>
        </div>
        <p className="small">
          Cleared balance here: <b>{money(cleared)}</b>
          {diff !== null && diff !== 0 && (
            <>
              {' '}
              · An adjustment of <b className={diff < 0 ? 'neg' : ''}>{money(diff)}</b> will be added.
            </>
          )}
          {diff === 0 && <> · They match.</>}
        </p>
        <p className="muted small">Cleared transactions dated on or before {formatDate(date)} will be locked as reconciled.</p>
        <div className="row end">
          <button onClick={onClose}>Cancel</button>
          <button
            className="primary"
            disabled={v === null}
            onClick={() => {
              onClose();
              void run(() => api.reconcile(accountId, v!, date));
            }}
          >
            {diff ? 'Adjust and reconcile' : 'Reconcile'}
          </button>
        </div>
      </div>
    </Modal>
  );
}

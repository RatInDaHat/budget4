import { useEffect, useRef, useState } from 'react';
import { api, type Cleared, type TransactionInput, type TxnDTO } from '../api.ts';
import { plain, today } from '../format.ts';
import { useApp } from '../state.tsx';
import { Autocomplete } from './Autocomplete.tsx';
import { amountFrom, TRANSFER, useCategoryOptions, usePayeeOptions } from './TxnEditor.tsx';

export type Field = 'date' | 'payee' | 'category' | 'memo' | 'outflow' | 'inflow';

/**
 * A register row edited in place. Enter (or clicking elsewhere) saves, Escape cancels. Split
 * transactions keep their lines; their category and amounts are changed in the full editor.
 *
 * With `t` null it's the register's entry row for a new transaction (in `accountId`, or in an
 * account picked in the row when the register shows all accounts). Enter saves it and calls
 * `onAdded` so the register can offer a fresh row; clicking elsewhere saves and closes it.
 */
export function InlineRow({
  t,
  accountId,
  date: newDate,
  focus,
  showAccount,
  onDone,
  onAdded,
  onFullEdit,
  onToggleCleared,
}: {
  t: TxnDTO | null;
  /** New transactions: the account (null in the all-accounts register). */
  accountId?: string | null;
  /** New transactions: the date to start with. */
  date?: string;
  focus: Field;
  showAccount: boolean;
  onDone: () => void;
  onAdded?: (accountId: string, date: string) => void;
  onFullEdit: () => void;
  onToggleCleared?: () => void;
}) {
  const { state, lookups, refresh, setToast } = useApp();
  const ref = useRef<HTMLDivElement>(null);
  const openAccounts = (state?.accounts ?? []).filter((a) => !a.hidden);
  const [acctId, setAcctId] = useState(t?.accountId ?? accountId ?? openAccounts[0]?.id ?? '');
  const [newCleared, setNewCleared] = useState<Cleared>('Uncleared');
  const cleared = t ? t.cleared : newCleared;
  const initial = t
    ? {
        date: t.date,
        payee: t.targetAccountId ? TRANSFER + t.targetAccountId : t.payeeId,
        payeeText: lookups.payeeLabel(t.payeeId) || t.importedPayee || '',
        categoryId: t.isSplit ? null : t.categoryId,
        memo: t.memo ?? '',
        outflow: t.amount < 0 ? plain(-t.amount) : '',
        inflow: t.amount > 0 ? plain(t.amount) : '',
      }
    : { date: newDate ?? today(), payee: null, payeeText: '', categoryId: null, memo: '', outflow: '', inflow: '' };
  const [f, setF] = useState(initial);
  const set = (patch: Partial<typeof initial>) => setF((cur) => ({ ...cur, ...patch }));
  const saving = useRef(false);
  const isSplit = !!t?.isSplit;

  const account = lookups.accounts.get(acctId);
  const transferTo = f.payee?.startsWith(TRANSFER) ? f.payee.slice(TRANSFER.length) : null;
  const target = transferTo ? lookups.accounts.get(transferTo) : undefined;
  const needsCategory = account?.onBudget && (!transferTo || !target?.onBudget);
  const payeeOptions = usePayeeOptions(acctId, t?.payeeId);
  const categoryOptions = useCategoryOptions(f.categoryId);

  // A new row counts as started once anything besides the date is filled in.
  const dirty = t
    ? (Object.keys(initial) as (keyof typeof initial)[]).some((k) => f[k] !== initial[k])
    : !!(f.payee || f.payeeText.trim() || f.categoryId || f.memo.trim() || f.outflow.trim() || f.inflow.trim());

  // Latest values for handlers registered once (the outside-click listener).
  const live = useRef({ f, dirty, acctId, cleared });
  live.current = { f, dirty, acctId, cleared };

  /** `next`: for a new transaction, offer another blank row afterwards instead of closing. */
  const save = async (next = false) => {
    if (saving.current) return;
    const { f: cur, acctId: acct, cleared: clr } = live.current;
    if (!live.current.dirty) return onDone();
    const amount = t?.isSplit ? t.amount : amountFrom(cur.outflow, cur.inflow);
    if (amount === null) return setToast('Check the amount');
    if (!cur.date) return setToast('Enter a date');
    if (!acct) return setToast('Choose an account');
    const to = cur.payee?.startsWith(TRANSFER) ? cur.payee.slice(TRANSFER.length) : null;
    const toAccount = to ? lookups.accounts.get(to) : undefined;
    const catNeeded = lookups.accounts.get(acct)?.onBudget && (!to || !toAccount?.onBudget);
    const input: TransactionInput = {
      id: t?.id,
      accountId: acct,
      date: cur.date,
      amount,
      payeeId: cur.payee && !to ? cur.payee : null,
      payeeName: !cur.payee && cur.payeeText.trim() ? cur.payeeText.trim() : null,
      transferAccountId: to,
      categoryId: t?.isSplit || !catNeeded ? null : cur.categoryId,
      memo: cur.memo || null,
      cleared: clr,
      accepted: true,
      flag: t?.flag ?? null,
      checkNumber: t?.checkNumber ?? null,
      splits: t?.isSplit
        ? t.splits.map((s) => ({
            id: s.id,
            amount: s.amount,
            categoryId: s.targetAccountId ? null : s.categoryId,
            memo: s.memo,
            transferAccountId: s.targetAccountId,
          }))
        : null,
    };
    saving.current = true;
    try {
      await api.saveTransaction(input);
      await refresh();
      if (!t && next && onAdded) onAdded(acct, cur.date);
      else onDone();
    } catch (e) {
      setToast((e as Error).message);
    } finally {
      saving.current = false;
    }
  };
  const saveRef = useRef(save);
  saveRef.current = save;

  // Clicking anywhere outside the row saves it (or just closes it when nothing changed).
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) void saveRef.current();
    };
    document.addEventListener('mousedown', onDown, true);
    return () => document.removeEventListener('mousedown', onDown, true);
  }, []);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      // First Escape closes an open suggestion list; the next one cancels.
      if (ref.current?.querySelector('.ac-list')) return;
      e.preventDefault();
      onDone();
    } else if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT' && !(e.target as HTMLElement).closest('.ac')) {
      e.preventDefault();
      void save(true);
    }
  };

  const onPayee = (id: string | null, text: string) => {
    const patch: Partial<typeof initial> = { payee: id, payeeText: text };
    const p = id && !id.startsWith(TRANSFER) ? lookups.payees.get(id) : undefined;
    if (p?.autoFillCategoryId && !live.current.f.categoryId && lookups.categories.has(p.autoFillCategoryId)) patch.categoryId = p.autoFillCategoryId;
    set(patch);
  };

  const select = (e: React.FocusEvent<HTMLInputElement>) => e.target.select();

  return (
    <div className={`reg-group editing inline ${t ? '' : 'new-entry'}`} ref={ref} onKeyDown={onKey}>
      <div className={`reg-row inline-edit ${showAccount ? 'with-account' : ''}`}>
        <span />
        {showAccount &&
          (t ? (
            <span className="ellipsis muted">{account?.name}</span>
          ) : (
            <span>
              <select value={acctId} onChange={(e) => setAcctId(e.target.value)} aria-label="Account">
                {openAccounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
              </select>
            </span>
          ))}
        <span>
          <input type="date" value={f.date} onChange={(e) => set({ date: e.target.value })} autoFocus={focus === 'date'} aria-label="Date" />
        </span>
        <span>
          <Autocomplete
            value={f.payee}
            text={f.payeeText}
            options={payeeOptions}
            onChange={onPayee}
            allowNew
            autoFocus={focus === 'payee'}
            ariaLabel="Payee"
            onEnter={() => void save(true)}
          />
        </span>
        <span>
          {t?.isSplit ? (
            <button className="link small" onClick={onFullEdit} title="Change the split lines in the full editor">
              Split ({t.splits.length})…
            </button>
          ) : needsCategory ? (
            <Autocomplete
              value={f.categoryId}
              options={categoryOptions}
              onChange={(id) => set({ categoryId: id })}
              autoFocus={focus === 'category'}
              placeholder="Category"
              ariaLabel="Category"
              onEnter={() => void save(true)}
            />
          ) : (
            <span className="muted small">{transferTo ? 'No category needed' : ''}</span>
          )}
        </span>
        <span>
          <input value={f.memo} onChange={(e) => set({ memo: e.target.value })} autoFocus={focus === 'memo'} onFocus={select} aria-label="Memo" />
        </span>
        {t && isSplit ? (
          <>
            <span className="num">{t.amount < 0 ? plain(-t.amount) : ''}</span>
            <span className="num">{t.amount > 0 ? plain(t.amount) : ''}</span>
          </>
        ) : (
          <>
            <span>
              <input
                className="num"
                value={f.outflow}
                inputMode="decimal"
                onChange={(e) => set(e.target.value ? { outflow: e.target.value, inflow: '' } : { outflow: '' })}
                autoFocus={focus === 'outflow'}
                onFocus={select}
                aria-label="Outflow"
              />
            </span>
            <span>
              <input
                className="num"
                value={f.inflow}
                inputMode="decimal"
                onChange={(e) => set(e.target.value ? { inflow: e.target.value, outflow: '' } : { inflow: '' })}
                autoFocus={focus === 'inflow'}
                onFocus={select}
                aria-label="Inflow"
              />
            </span>
          </>
        )}
        <span className="center">
          <button
            className={`cleared-btn ${cleared}`}
            onClick={t ? onToggleCleared : () => setNewCleared(newCleared === 'Cleared' ? 'Uncleared' : 'Cleared')}
            disabled={cleared === 'Reconciled'}
            title={cleared}
          >
            {cleared === 'Reconciled' ? 'R' : 'C'}
          </button>
        </span>
        <span className="row-end inline-actions">
          <button className="small primary" onClick={() => void save(true)} title="Save (Enter)">
            {t ? 'Save' : 'Add'}
          </button>
          <button className="icon" onClick={onDone} title="Cancel (Esc)" aria-label="Cancel">
            ×
          </button>
          <button className="icon" onClick={onFullEdit} title="Open the full editor (splits, account)" aria-label="Full editor">
            ⋯
          </button>
        </span>
      </div>
    </div>
  );
}

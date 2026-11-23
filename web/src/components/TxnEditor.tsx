import { useMemo, useState } from 'react';
import type { Cleared, TransactionInput, TxnDTO } from '../api.ts';
import { money, parseAmount, plain, round, today } from '../format.ts';
import { useApp } from '../state.tsx';
import { Autocomplete, type Option } from './Autocomplete.tsx';

interface SplitRow {
  id?: string;
  categoryId: string | null;
  transferAccountId: string | null;
  memo: string;
  outflow: string;
  inflow: string;
}

export const TRANSFER = 'transfer:';

/** Payee choices for a transaction in `accountId`: "Transfer : <account>" entries, then payees. */
export function usePayeeOptions(accountId: string, currentPayeeId: string | null | undefined): Option[] {
  const { state } = useApp();
  return useMemo<Option[]>(() => {
    const transfers = (state?.accounts ?? [])
      .filter((a) => !a.hidden && a.id !== accountId)
      .map((a) => ({ id: TRANSFER + a.id, label: `Transfer : ${a.name}` }));
    const regular = (state?.payees ?? [])
      .filter((p) => !p.transferAccountId && (p.enabled || p.id === currentPayeeId))
      .map((p) => ({ id: p.id, label: p.name }))
      .sort((a, b) => a.label.localeCompare(b.label));
    return [...transfers, ...regular];
  }, [state, accountId, currentPayeeId]);
}

/** Category choices (hidden ones only when already chosen). */
export function useCategoryOptions(categoryId: string | null): Option[] {
  const { lookups } = useApp();
  return useMemo<Option[]>(
    () => lookups.categoryOptions.filter((c) => !c.hidden || c.id === categoryId).map((c) => ({ id: c.id, label: c.name, group: c.master })),
    [lookups, categoryId],
  );
}

/** Outflow/inflow text to a signed amount (null if either doesn't read as a number). */
export function amountFrom(out: string, inn: string): number | null {
  const o = out.trim() ? parseAmount(out) : 0;
  const i = inn.trim() ? parseAmount(inn) : 0;
  if (o === null || i === null) return null;
  return round(i - o);
}

/**
 * Editor for one transaction (new or existing). Payee choices include "Transfer : <account>" entries;
 * picking one makes it a transfer. On-budget ↔ on-budget transfers have no category.
 */
export function TxnEditor({
  txn,
  defaultAccountId,
  onSave,
  onCancel,
  extra,
  saveLabel = 'Save',
}: {
  txn: TxnDTO | null;
  defaultAccountId: string | null;
  onSave: (input: TransactionInput) => Promise<unknown>;
  onCancel: () => void;
  extra?: React.ReactNode;
  saveLabel?: string;
}) {
  const { state, lookups } = useApp();
  const accounts = (state?.accounts ?? []).filter((a) => !a.hidden || a.id === txn?.accountId || a.id === txn?.targetAccountId);
  const [accountId, setAccountId] = useState(txn?.accountId ?? defaultAccountId ?? accounts.find((a) => a.onBudget)?.id ?? accounts[0]?.id ?? '');
  const [date, setDate] = useState(txn?.date ?? today());
  const initialPayee = txn?.targetAccountId ? TRANSFER + txn.targetAccountId : txn?.payeeId ?? null;
  const [payee, setPayee] = useState<string | null>(initialPayee);
  const [payeeText, setPayeeText] = useState(txn ? lookups.payeeLabel(txn.payeeId) : '');
  const [categoryId, setCategoryId] = useState<string | null>(txn && !txn.isSplit ? txn.categoryId : null);
  const [memo, setMemo] = useState(txn?.memo ?? '');
  const [outflow, setOutflow] = useState(txn && txn.amount < 0 ? plain(-txn.amount) : '');
  const [inflow, setInflow] = useState(txn && txn.amount > 0 ? plain(txn.amount) : '');
  const [cleared, setCleared] = useState<Cleared>(txn?.cleared ?? 'Uncleared');
  const [splits, setSplits] = useState<SplitRow[] | null>(
    txn?.isSplit
      ? txn.splits.map((s) => ({
          id: s.id,
          categoryId: s.targetAccountId ? null : s.categoryId,
          transferAccountId: s.targetAccountId,
          memo: s.memo ?? '',
          outflow: s.amount < 0 ? plain(-s.amount) : '',
          inflow: s.amount > 0 ? plain(s.amount) : '',
        }))
      : null,
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const account = lookups.accounts.get(accountId);
  const transferTo = payee?.startsWith(TRANSFER) ? payee.slice(TRANSFER.length) : null;
  const target = transferTo ? lookups.accounts.get(transferTo) : undefined;
  const needsCategory = account?.onBudget && (!transferTo || !target?.onBudget);

  const payeeOptions = usePayeeOptions(accountId, txn?.payeeId);
  const categoryOptions = useCategoryOptions(categoryId);
  const amount = amountFrom(outflow, inflow);
  const splitTotal = splits ? round(splits.reduce((s, r) => s + (amountFrom(r.outflow, r.inflow) ?? 0), 0)) : 0;

  const onPayee = (id: string | null, text: string) => {
    setPayee(id);
    setPayeeText(text);
    if (!id || id.startsWith(TRANSFER) || txn) return;
    const p = lookups.payees.get(id);
    if (p?.autoFillCategoryId && !categoryId && lookups.categories.has(p.autoFillCategoryId)) setCategoryId(p.autoFillCategoryId);
    if (p && !outflow && !inflow && p.autoFillAmount) {
      if (p.autoFillAmount < 0) setOutflow(plain(-p.autoFillAmount));
      else setInflow(plain(p.autoFillAmount));
    }
    if (p?.autoFillMemo && !memo) setMemo(p.autoFillMemo);
  };

  const save = async () => {
    setError(null);
    if (!accountId) return setError('Choose an account');
    if (!date) return setError('Enter a date');
    if (amount === null) return setError('Check the amount');
    let splitInputs: TransactionInput['splits'] = null;
    if (splits) {
      splitInputs = [];
      for (const r of splits) {
        const a = amountFrom(r.outflow, r.inflow);
        if (a === null) return setError('Check the split amounts');
        splitInputs.push({ id: r.id, amount: a, categoryId: r.categoryId, memo: r.memo || null, transferAccountId: r.transferAccountId });
      }
      if (round(splitTotal) !== amount) return setError(`Split lines add up to ${money(splitTotal)}, not ${money(amount)}`);
    }
    setSaving(true);
    try {
      await onSave({
        id: txn?.id,
        accountId,
        date,
        amount,
        payeeId: payee && !transferTo ? payee : null,
        payeeName: !payee && payeeText.trim() ? payeeText.trim() : null,
        transferAccountId: transferTo,
        categoryId: splits || !needsCategory ? null : categoryId,
        memo: memo || null,
        cleared,
        accepted: true,
        flag: txn?.flag ?? null,
        checkNumber: txn?.checkNumber ?? null,
        splits: splitInputs,
      });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') onCancel();
    if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT' && !(e.target as HTMLElement).closest('.ac')) {
      e.preventDefault();
      void save();
    }
  };

  return (
    <div className="txn-editor" onKeyDown={onKey}>
      <div className="editor-grid">
        {!defaultAccountId || txn ? (
          <label>
            Account
            <select value={accountId} onChange={(e) => setAccountId(e.target.value)}>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        <label>
          Date
          <input type="date" value={date} onChange={(e) => setDate(e.target.value)} />
        </label>
        <label className="wide">
          Payee
          <Autocomplete value={payee} text={payeeText} options={payeeOptions} onChange={onPayee} allowNew placeholder="Who was it?" autoFocus={!txn} ariaLabel="Payee" onEnter={save} />
        </label>
        <label className="wide">
          Category
          {splits ? (
            <div className="split-note">Split into {splits.length} lines</div>
          ) : needsCategory ? (
            <Autocomplete value={categoryId} options={categoryOptions} onChange={(id) => setCategoryId(id)} placeholder="Choose a category" ariaLabel="Category" onEnter={save} />
          ) : (
            <div className="split-note muted">{transferTo ? 'No category needed for this transfer' : 'Off-budget account'}</div>
          )}
        </label>
        <label className="wide">
          Memo
          <input value={memo} onChange={(e) => setMemo(e.target.value)} />
        </label>
        <label>
          Outflow
          <input className="num" value={outflow} inputMode="decimal" onChange={(e) => (setOutflow(e.target.value), e.target.value && setInflow(''))} />
        </label>
        <label>
          Inflow
          <input className="num" value={inflow} inputMode="decimal" onChange={(e) => (setInflow(e.target.value), e.target.value && setOutflow(''))} />
        </label>
        <label className="cleared-field">
          Cleared
          <button type="button" className={`cleared-btn ${cleared}`} disabled={cleared === 'Reconciled'} onClick={() => setCleared(cleared === 'Cleared' ? 'Uncleared' : 'Cleared')} title={cleared}>
            {cleared === 'Reconciled' ? 'R' : 'C'}
          </button>
        </label>
      </div>

      {splits && (
        <div className="splits">
          {splits.map((r, i) => (
            <div className="split-row" key={r.id ?? i}>
              <Autocomplete
                value={r.transferAccountId ? TRANSFER + r.transferAccountId : r.categoryId}
                options={[
                  ...(state?.accounts ?? []).filter((a) => !a.hidden && a.id !== accountId).map((a) => ({ id: TRANSFER + a.id, label: `Transfer : ${a.name}`, group: 'Transfer' })),
                  ...categoryOptions,
                ]}
                onChange={(id) => {
                  const next = [...splits];
                  next[i] = id?.startsWith(TRANSFER) ? { ...r, transferAccountId: id.slice(TRANSFER.length), categoryId: null } : { ...r, categoryId: id, transferAccountId: null };
                  setSplits(next);
                }}
                placeholder="Category or transfer"
                ariaLabel="Split category"
              />
              <input placeholder="Memo" value={r.memo} onChange={(e) => setSplits(splits.map((x, j) => (j === i ? { ...x, memo: e.target.value } : x)))} />
              <input className="num" placeholder="Outflow" value={r.outflow} onChange={(e) => setSplits(splits.map((x, j) => (j === i ? { ...x, outflow: e.target.value, inflow: '' } : x)))} />
              <input className="num" placeholder="Inflow" value={r.inflow} onChange={(e) => setSplits(splits.map((x, j) => (j === i ? { ...x, inflow: e.target.value, outflow: '' } : x)))} />
              <button className="icon" onClick={() => setSplits(splits.length > 2 ? splits.filter((_, j) => j !== i) : splits)} aria-label="Remove line" disabled={splits.length <= 2}>
                ×
              </button>
            </div>
          ))}
          <div className="row">
            <button className="link small" onClick={() => setSplits([...splits, { categoryId: null, transferAccountId: null, memo: '', outflow: '', inflow: '' }])}>
              + Add line
            </button>
            {amount !== null && (
              <span className={`small ${round(amount - splitTotal) !== 0 ? 'error' : 'muted'}`}>
                {round(amount - splitTotal) !== 0 ? `${money(round(amount - splitTotal))} left to assign` : 'Lines add up'}
              </span>
            )}
          </div>
        </div>
      )}

      {error && <p className="error small">{error}</p>}
      <div className="row editor-actions">
        {!splits ? (
          <button
            className="link small"
            onClick={() =>
              setSplits([
                { categoryId, transferAccountId: null, memo: '', outflow, inflow },
                { categoryId: null, transferAccountId: null, memo: '', outflow: '', inflow: '' },
              ])
            }
          >
            Split…
          </button>
        ) : (
          <button className="link small" onClick={() => setSplits(null)}>
            Remove split
          </button>
        )}
        <span className="grow" />
        {extra}
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" onClick={save} disabled={saving}>
          {saveLabel}
        </button>
      </div>
    </div>
  );
}

import { useState } from 'react';
import { api, type AccountDTO } from '../api.ts';
import { money, parseAmount, today } from '../format.ts';
import { useApp } from '../state.tsx';
import { Modal } from './Modal.tsx';

export const ACCOUNT_TYPES: [string, string, boolean][] = [
  ['Checking', 'Checking', true],
  ['Savings', 'Savings', true],
  ['CreditCard', 'Credit card', true],
  ['Cash', 'Cash', true],
  ['LineOfCredit', 'Line of credit', true],
  ['Paypal', 'PayPal', true],
  ['MerchantAccount', 'Merchant account', true],
  ['InvestmentAccount', 'Investment account', false],
  ['Mortgage', 'Mortgage', false],
  ['OtherAsset', 'Other asset', false],
  ['OtherLiability', 'Other loan or liability', false],
];

export function AccountDialog({ onClose, account }: { onClose: () => void; account?: AccountDTO }) {
  const { run, setView } = useApp();
  const [name, setName] = useState(account?.name ?? '');
  const [type, setType] = useState(account?.type ?? 'Checking');
  const [onBudget, setOnBudget] = useState(account?.onBudget ?? true);
  const [balance, setBalance] = useState('0.00');
  const [date, setDate] = useState(today());
  const [note, setNote] = useState(account?.note ?? '');
  const parsed = parseAmount(balance);

  const submit = async () => {
    if (account) {
      await run(() => api.updateAccount(account.id, { name, note: note || null }));
      onClose();
      return;
    }
    if (parsed === null) return;
    const id = await run(() => api.createAccount({ name, type, onBudget, startingBalance: parsed, startingDate: date, note: note || null }));
    if (id) setView({ kind: 'account', accountId: id });
    onClose();
  };

  return (
    <Modal title={account ? 'Edit account' : 'Add an account'} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label>
          Name
          <input autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        {!account && (
          <>
            <label>
              Type
              <select
                value={type}
                onChange={(e) => {
                  setType(e.target.value);
                  setOnBudget(ACCOUNT_TYPES.find(([v]) => v === e.target.value)?.[2] ?? true);
                }}
              >
                {ACCOUNT_TYPES.map(([v, l]) => (
                  <option key={v} value={v}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
            <fieldset className="radios">
              <label>
                <input type="radio" checked={onBudget} onChange={() => setOnBudget(true)} /> Budget account
                <span className="muted small">Its money is part of your budget.</span>
              </label>
              <label>
                <input type="radio" checked={!onBudget} onChange={() => setOnBudget(false)} /> Off-budget
                <span className="muted small">Tracked for net worth only (loans, investments).</span>
              </label>
            </fieldset>
            <div className="row">
              <label className="grow">
                Current balance
                <input value={balance} onChange={(e) => setBalance(e.target.value)} inputMode="decimal" />
              </label>
              <label>
                As of
                <input type="date" value={date} onChange={(e) => setDate(e.target.value)} />
              </label>
            </div>
            {parsed === null ? <p className="error small">Enter an amount like 1,250.00 or -300.</p> : <p className="muted small">Starting balance {money(parsed)}. Use a negative number for money you owe.</p>}
          </>
        )}
        <label>
          Note
          <textarea rows={2} value={note} onChange={(e) => setNote(e.target.value)} />
        </label>
        <div className="row end">
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" disabled={!name.trim() || (!account && parsed === null)}>
            {account ? 'Save' : 'Add account'}
          </button>
        </div>
      </form>
    </Modal>
  );
}

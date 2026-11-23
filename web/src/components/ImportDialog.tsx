import { useEffect, useState } from 'react';
import {
  api,
  pickBankFile,
  type CsvMapping,
  type DateOrder,
  type ImportPreview,
  type ImportRequest,
  type ImportRow,
  type LoadedFile,
  type OfxStatement,
} from '../api.ts';
import { formatDate, plain } from '../format.ts';
import { useApp } from '../state.tsx';
import { Modal } from './Modal.tsx';

/** Per statement (OFX) or for the whole file (CSV). accountId '' = not chosen yet, 'skip' = don't import. */
interface Choice {
  accountId: string;
  importMemos: boolean;
  flipPayeeMemo: boolean;
}

const ORDER_LABEL: Record<DateOrder, string> = { MDY: 'Month / day / year', DMY: 'Day / month / year', YMD: 'Year / month / day' };

function statementLabel(s: OfxStatement): string {
  if (s.kind === 'creditCard') return 'Credit card';
  if (s.kind === 'investment') return 'Investment account';
  const t = (s.accountType ?? '').toLowerCase();
  return t ? t[0].toUpperCase() + t.slice(1).replace('moneymrkt', 'money market') : 'Bank account';
}

function range(from: string | null, to: string | null): string {
  if (!from || !to) return '';
  return from === to ? formatDate(from) : `${formatDate(from)} – ${formatDate(to)}`;
}

export function ImportDialog({ defaultAccountId, onClose }: { defaultAccountId: string | null; onClose: () => void }) {
  const { state, refresh, setToast, lookups } = useApp();
  const [file, setFile] = useState<LoadedFile | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [choices, setChoices] = useState<Choice[]>([]);
  const [active, setActive] = useState(0);
  const [mapping, setMapping] = useState<CsvMapping | null>(null);
  const [includeBefore, setIncludeBefore] = useState(false);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const accounts = (state?.accounts ?? []).filter((a) => !a.hidden);

  const close = () => {
    void api.importCancel().catch(() => {});
    onClose();
  };

  const [typedPath, setTypedPath] = useState('');

  const choose = async (given?: string) => {
    const path = given ?? (await pickBankFile());
    if (!path) return;
    setBusy('Reading the file…');
    setError(null);
    setPreview(null);
    setIncludeBefore(false);
    try {
      const f = await api.importLoad(path, defaultAccountId);
      setFile(f);
      if (f.kind === 'ofx') {
        const single = f.statements.length === 1;
        setChoices(
          f.statements.map((s) => ({
            accountId: s.mapping?.skip ? 'skip' : (s.mapping?.accountId ?? (single && defaultAccountId ? defaultAccountId : '')),
            importMemos: s.mapping?.importMemos ?? true,
            flipPayeeMemo: s.mapping?.flipPayeeMemo ?? false,
          })),
        );
        setActive(0);
        setMapping(null);
      } else {
        setChoices([{ accountId: defaultAccountId ?? '', importMemos: true, flipPayeeMemo: false }]);
        setMapping(f.mapping);
      }
    } catch (e) {
      setError((e as Error).message);
      setFile(null);
    } finally {
      setBusy(null);
    }
  };

  const request = (i: number): ImportRequest | null => {
    const c = choices[i];
    if (!file || !c || !c.accountId || c.accountId === 'skip') return null;
    if (file.kind === 'csv' && !mapping) return null;
    return {
      statement: file.kind === 'ofx' ? i : undefined,
      mapping: file.kind === 'csv' ? (mapping ?? undefined) : undefined,
      accountId: c.accountId,
      importMemos: c.importMemos,
      flipPayeeMemo: c.flipPayeeMemo,
      includeBeforeStart: includeBefore,
    };
  };

  const shown = file?.kind === 'ofx' ? active : 0;
  const req = request(shown);
  const reqKey = JSON.stringify(req);
  useEffect(() => {
    if (!req) {
      setPreview(null);
      return;
    }
    let live = true;
    const t = window.setTimeout(() => {
      api
        .importPreview(req)
        .then((p) => {
          if (!live) return;
          setPreview(p);
          setError(null);
        })
        .catch((e) => {
          if (!live) return;
          setPreview(null);
          setError((e as Error).message);
        });
    }, 150);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reqKey]);

  const setChoice = (i: number, patch: Partial<Choice>) => setChoices((cs) => cs.map((c, j) => (j === i ? { ...c, ...patch } : c)));

  const ready = !!file && choices.length > 0 && choices.every((c) => c.accountId !== '') && choices.some((c) => c.accountId !== 'skip');

  const commit = async () => {
    if (!file) return;
    setBusy('Importing…');
    setError(null);
    const total = { imported: 0, matched: 0, duplicates: 0, beforeStart: 0 };
    try {
      for (let i = 0; i < choices.length; i++) {
        const c = choices[i];
        if (c.accountId === 'skip') {
          // Remembered, as YNAB 4 does, so this account is skipped next time too.
          await api.importCommit({ statement: i, accountId: 'skip', importMemos: c.importMemos, flipPayeeMemo: c.flipPayeeMemo, includeBeforeStart: false, skip: true });
          continue;
        }
        const r = request(i);
        if (!r) continue;
        const out = await api.importCommit(r);
        total.imported += out.imported;
        total.matched += out.matched;
        total.duplicates += out.duplicates;
        total.beforeStart += out.beforeStart;
      }
      await refresh();
      const parts = [`${total.imported} imported`];
      if (total.matched) parts.push(`${total.matched} matched to transactions you entered`);
      if (total.duplicates) parts.push(`${total.duplicates} already imported`);
      if (total.beforeStart) parts.push(`${total.beforeStart} before the account's start left out`);
      setToast(`Import done: ${parts.join(', ')}. Approve them in the register.`);
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const accountSelect = (i: number, allowSkip: boolean) => (
    <select value={choices[i]?.accountId ?? ''} onChange={(e) => setChoice(i, { accountId: e.target.value })}>
      <option value="">Choose an account…</option>
      {accounts.map((a) => (
        <option key={a.id} value={a.id}>
          {a.name}
        </option>
      ))}
      {allowSkip && <option value="skip">Don't import this account</option>}
    </select>
  );

  return (
    <Modal title="Import transactions" onClose={close} wide>
      <div className="form import">
        {!file ? (
          <>
            <p>
              Import an OFX, QFX or QBO file downloaded from your bank, or a CSV file. Transactions imported before are skipped, and ones you
              entered yourself are matched up instead of doubled.
            </p>
            <div className="row">
              <button className="primary" onClick={() => void choose()} disabled={!!busy}>
                Choose file…
              </button>
              <span className="muted small">or</span>
              <input
                className="grow"
                aria-label="File path"
                placeholder="Paste a file path"
                value={typedPath}
                onChange={(e) => setTypedPath(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && typedPath.trim() && void choose(typedPath.trim().replace(/^"|"$/g, ''))}
              />
              <button disabled={!typedPath.trim() || !!busy} onClick={() => void choose(typedPath.trim().replace(/^"|"$/g, ''))}>
                Open
              </button>
            </div>
            {busy && <p className="muted">{busy}</p>}
            {error && <p className="error">{error}</p>}
            <div className="row end">
              <button onClick={close}>Cancel</button>
            </div>
          </>
        ) : (
          <>
            <div className="row">
              <b className="grow ellipsis" title={file.name}>
                {file.name}
              </b>
              <button className="small" onClick={() => void choose()} disabled={!!busy}>
                Choose another file…
              </button>
            </div>

            {file.kind === 'ofx' ? (
              <div className="stmt-list">
                {file.statements.map((s) => {
                  const i = s.index;
                  return (
                    <div key={i} className={`stmt ${active === i ? 'active' : ''}`} onMouseDown={() => setActive(i)}>
                      <div className="stmt-title">
                        <b>
                          {statementLabel(s)} ••••{s.last4}
                        </b>
                        <span className="muted small">
                          {s.count} transaction{s.count === 1 ? '' : 's'} · {range(s.from, s.to)}
                        </span>
                        {s.mapping && <span className="tag">Remembered</span>}
                      </div>
                      <div className="row">
                        <label className="grow">
                          Import into
                          {accountSelect(i, true)}
                        </label>
                        <label className="check small">
                          <input type="checkbox" checked={choices[i]?.importMemos ?? true} onChange={(e) => setChoice(i, { importMemos: e.target.checked })} /> Import
                          memos
                        </label>
                        <label className="check small">
                          <input type="checkbox" checked={choices[i]?.flipPayeeMemo ?? false} onChange={(e) => setChoice(i, { flipPayeeMemo: e.target.checked })} />{' '}
                          Swap payee and memo
                        </label>
                      </div>
                    </div>
                  );
                })}
                {file.statements.length > 1 && <p className="muted small">Click an account to preview it. All of them are imported together.</p>}
              </div>
            ) : (
              mapping && (
                <CsvSetup
                  file={file}
                  mapping={mapping}
                  setMapping={setMapping}
                  dateOrders={preview?.dateOrders ?? file.dateOrders}
                  accountSelect={accountSelect(0, false)}
                  choice={choices[0]}
                  setChoice={(p) => setChoice(0, p)}
                />
              )
            )}

            {error && <p className="error">{error}</p>}
            {preview && <PreviewTable p={preview} includeBefore={includeBefore} setIncludeBefore={setIncludeBefore} lookups={lookups} />}

            <div className="row end">
              {busy && <span className="muted small grow">{busy}</span>}
              <button onClick={close}>Cancel</button>
              <button className="primary" disabled={!ready || !!busy} onClick={() => void commit()}>
                {file.kind === 'ofx' && file.statements.length > 1 ? 'Import all' : 'Import'}
              </button>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}

function CsvSetup({
  file,
  mapping,
  setMapping,
  dateOrders,
  accountSelect,
  choice,
  setChoice,
}: {
  file: Extract<LoadedFile, { kind: 'csv' }>;
  mapping: CsvMapping;
  setMapping: (m: CsvMapping) => void;
  dateOrders: DateOrder[];
  accountSelect: React.ReactNode;
  choice: Choice | undefined;
  setChoice: (p: Partial<Choice>) => void;
}) {
  const [split, setSplit] = useState(mapping.amount === null && (mapping.outflow !== null || mapping.inflow !== null));
  const header = mapping.hasHeader ? file.sample[0] : undefined;
  const body = mapping.hasHeader ? file.sample.slice(1) : file.sample;
  const name = (i: number) => (header?.[i] ? header[i] : `Column ${i + 1}`);
  const set = (patch: Partial<CsvMapping>) => setMapping({ ...mapping, ...patch });
  const col = (key: 'date' | 'payee' | 'memo' | 'amount' | 'outflow' | 'inflow' | 'checkNumber', label: string) => (
    <label>
      {label}
      <select value={mapping[key] ?? ''} onChange={(e) => set({ [key]: e.target.value === '' ? null : Number(e.target.value) })}>
        <option value="">—</option>
        {Array.from({ length: file.columns }, (_, i) => (
          <option key={i} value={i}>
            {name(i)}
            {body[0]?.[i] ? ` (e.g. ${body[0][i].slice(0, 24)})` : ''}
          </option>
        ))}
      </select>
    </label>
  );

  return (
    <div className="csv-setup">
      <label>
        Import into
        {accountSelect}
      </label>
      <div className="csv-grid">
        {col('date', 'Date')}
        <label>
          Date format
          <select value={mapping.dateOrder} onChange={(e) => set({ dateOrder: e.target.value as DateOrder })}>
            {(['MDY', 'DMY', 'YMD'] as DateOrder[]).map((o) => (
              <option key={o} value={o} disabled={dateOrders.length > 0 && !dateOrders.includes(o)}>
                {ORDER_LABEL[o]}
              </option>
            ))}
          </select>
        </label>
        {col('payee', 'Payee')}
        {col('memo', 'Memo')}
        <label>
          Amounts
          <select
            value={split ? 'two' : 'one'}
            onChange={(e) => {
              const two = e.target.value === 'two';
              setSplit(two);
              set(two ? { amount: null } : { outflow: null, inflow: null });
            }}
          >
            <option value="one">One column (negative = outflow)</option>
            <option value="two">Separate outflow and inflow columns</option>
          </select>
        </label>
        {split ? (
          <>
            {col('outflow', 'Outflow')}
            {col('inflow', 'Inflow')}
          </>
        ) : (
          col('amount', 'Amount')
        )}
        {col('checkNumber', 'Check number')}
      </div>
      <div className="row wrap">
        <label className="check small">
          <input type="checkbox" checked={mapping.hasHeader} onChange={(e) => set({ hasHeader: e.target.checked })} /> First row is column names
        </label>
        <label className="check small">
          <input type="checkbox" checked={mapping.negate} onChange={(e) => set({ negate: e.target.checked })} /> Flip signs (charges are positive in this file)
        </label>
        <label className="check small">
          <input type="checkbox" checked={choice?.flipPayeeMemo ?? false} onChange={(e) => setChoice({ flipPayeeMemo: e.target.checked })} /> Swap payee and memo
        </label>
      </div>
      <p className="muted small">
        {file.rowCount - (mapping.hasHeader ? 1 : 0)} rows. The column layout is remembered for this account.
      </p>
    </div>
  );
}

function statusText(r: ImportRow, lookups: ReturnType<typeof useApp>['lookups']): string {
  switch (r.status) {
    case 'matched':
      return r.matched ? `Matches ${lookups.payeeLabel(r.matched.payeeId) || 'your entry'} on ${formatDate(r.matched.date)}` : 'Matches your entry';
    case 'duplicate':
      return 'Already imported';
    case 'beforeStart':
      return "Before the account's start";
    default:
      return r.categoryId ? `New · ${lookups.categoryLabel(r.categoryId)}` : 'New';
  }
}

function PreviewTable({
  p,
  includeBefore,
  setIncludeBefore,
  lookups,
}: {
  p: ImportPreview;
  includeBefore: boolean;
  setIncludeBefore: (b: boolean) => void;
  lookups: ReturnType<typeof useApp>['lookups'];
}) {
  return (
    <div className="import-preview">
      <div className="import-summary">
        <span>
          <b>{p.imported}</b> new
        </span>
        <span>
          <b>{p.matched}</b> matched to transactions you entered
        </span>
        <span>
          <b>{p.duplicates}</b> already imported
        </span>
        {p.beforeStart > 0 && (
          <span>
            <b>{p.beforeStart}</b> before the account's start
          </span>
        )}
      </div>
      {(p.beforeStart > 0 || includeBefore) && p.startDate && (
        <label className="check small">
          <input type="checkbox" checked={includeBefore} onChange={(e) => setIncludeBefore(e.target.checked)} /> Also import transactions dated before this
          account's starting balance ({formatDate(p.startDate)}). The starting balance moves earlier and changes so today's balance stays the same.
        </label>
      )}
      {p.problems.length > 0 && (
        <p className="warn-text small">
          {p.problems.length} row{p.problems.length === 1 ? '' : 's'} couldn't be read and will be left out: {p.problems.slice(0, 3).join('; ')}
          {p.problems.length > 3 ? '…' : ''}
        </p>
      )}
      <div className="import-table-wrap">
        <table className="table compact">
          <thead>
            <tr>
              <th>Date</th>
              <th>Payee</th>
              <th>Memo</th>
              <th className="num">Amount</th>
              <th>Result</th>
            </tr>
          </thead>
          <tbody>
            {p.rows.map((r, i) => (
              <tr key={i} className={`imp-${r.status}`}>
                <td>{formatDate(r.date)}</td>
                <td title={r.importedPayee ?? ''}>
                  {r.payee ?? ''}
                  {r.importedPayee && r.payee !== r.importedPayee && <div className="muted small">{r.importedPayee}</div>}
                </td>
                <td className="muted">{r.memo}</td>
                <td className={`num ${r.amount < 0 ? 'neg' : ''}`}>{plain(r.amount)}</td>
                <td>{statusText(r, lookups)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

import { useMemo, useRef, useState } from 'react';
import { api } from '../api.ts';
import { useApp } from '../state.tsx';

/** Lists the payees you type in transactions (not transfers, which are named after their account). */
export function PayeesView() {
  const { state, run, setToast } = useApp();
  const [filter, setFilter] = useState('');
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState<string | null>(null);
  const [into, setInto] = useState<string | null>(null);
  const cancelled = useRef(false);

  const payees = useMemo(
    () =>
      (state?.payees ?? [])
        .filter((p) => p.transferAccountId == null)
        .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' })),
    [state],
  );
  if (!state) return null;

  const needle = filter.trim().toLowerCase();
  const shown = needle ? payees.filter((p) => p.name.toLowerCase().includes(needle)) : payees;
  const chosen = payees.filter((p) => picked.has(p.id));
  // The merge target: the picked payee the user chose, or the first one picked.
  const target = chosen.find((p) => p.id === into) ?? chosen[0] ?? null;

  const toggle = (id: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const commitRename = async (id: string, typed: string) => {
    setEditing(null);
    const original = payees.find((p) => p.id === id);
    const name = typed.trim();
    if (!original || !name || name === original.name) return;
    const clash = payees.find((p) => p.id !== id && p.name.trim().toLowerCase() === name.toLowerCase());
    if (clash) {
      setToast(`There's already a payee called "${clash.name}". Select both and merge them instead.`);
      return;
    }
    await run(() => api.renamePayee(id, name));
  };

  const merge = async () => {
    if (!target || chosen.length < 2) return;
    const from = chosen.filter((p) => p.id !== target.id).map((p) => p.id);
    const done = await run(() => api.mergePayees(from, target.id));
    if (done !== undefined) {
      setToast(`Merged ${from.length} payee${from.length === 1 ? '' : 's'} into "${target.name}".`);
      setPicked(new Set());
      setInto(null);
    }
  };

  return (
    <div className="settings">
      <header className="toolbar">
        <h2>Payees</h2>
      </header>
      <section>
        <p className="muted small">
          Click a name to rename it. To combine payees, tick two or more, choose the one to keep, and merge: every transaction and scheduled transaction moves to
          it and the others are removed. Undo can reverse either change.
        </p>
        <input className="search" type="search" placeholder="Filter payees" aria-label="Filter payees" value={filter} onChange={(e) => setFilter(e.target.value)} />
        {chosen.length > 0 && (
          <div className="row">
            <span>{chosen.length} selected</span>
            <select value={target?.id ?? ''} onChange={(e) => setInto(e.target.value)} aria-label="Payee to keep">
              {chosen.map((p) => (
                <option key={p.id} value={p.id}>
                  Keep "{p.name}"
                </option>
              ))}
            </select>
            <button className="primary" disabled={chosen.length < 2} onClick={() => void merge()}>
              Merge
            </button>
            <button className="link" onClick={() => setPicked(new Set())}>
              Clear selection
            </button>
          </div>
        )}
        <table className="table compact">
          <thead>
            <tr>
              <th aria-label="Select" />
              <th>Name</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((p) => (
              <tr key={p.id}>
                <td>
                  <input type="checkbox" checked={picked.has(p.id)} onChange={() => toggle(p.id)} aria-label={`Select ${p.name}`} />
                </td>
                <td>
                  {editing === p.id ? (
                    <input
                      className="payee-rename"
                      autoFocus
                      defaultValue={p.name}
                      aria-label="Payee name"
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') e.currentTarget.blur();
                        if (e.key === 'Escape') {
                          cancelled.current = true;
                          e.currentTarget.blur();
                        }
                      }}
                      onBlur={(e) => {
                        const cancel = cancelled.current;
                        cancelled.current = false;
                        if (cancel) setEditing(null);
                        else void commitRename(p.id, e.currentTarget.value);
                      }}
                    />
                  ) : (
                    <button className="link payee-name" onClick={() => setEditing(p.id)} title="Rename">
                      {p.name || <span className="muted">(no name)</span>}
                    </button>
                  )}
                </td>
              </tr>
            ))}
            {shown.length === 0 && (
              <tr>
                <td colSpan={2} className="muted">
                  {payees.length === 0 ? 'No payees yet.' : 'No payees match.'}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>
    </div>
  );
}

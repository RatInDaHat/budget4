import { useEffect, useState } from 'react';
import { api, pickArchiveFile, pickFolder, type AppInfo, type BudgetListing, type StateDTO } from '../api.ts';

const LOCALES: [string, string][] = [
  ['en_US', 'United States (USD)'],
  ['en_CA', 'Canada (CAD)'],
  ['en_GB', 'United Kingdom (GBP)'],
  ['en_AU', 'Australia (AUD)'],
  ['en_NZ', 'New Zealand (NZD)'],
  ['de_DE', 'Germany (EUR)'],
  ['fr_FR', 'France (EUR)'],
  ['nl_NL', 'Netherlands (EUR)'],
  ['es_ES', 'Spain (EUR)'],
  ['sv_SE', 'Sweden (SEK)'],
  ['ja_JP', 'Japan (JPY)'],
];

export function OpenBudget({ onOpened }: { onOpened: (s: StateDTO) => void }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [budgets, setBudgets] = useState<BudgetListing[]>([]);
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('My Budget');
  const [parent, setParent] = useState('');
  const [locale, setLocale] = useState('en_US');
  const [starter, setStarter] = useState<'personal' | 'business' | 'none'>('personal');

  useEffect(() => {
    api.app().then((i) => {
      setInfo(i);
      setParent(i.budgetFolders[0] ?? '');
    });
    api.budgets().then(setBudgets).catch(() => {});
  }, []);

  const open = async (folder: string) => {
    setBusy(folder);
    setError(null);
    try {
      onOpened(await api.open(folder));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const forget = async (folder: string) => {
    setError(null);
    try {
      await api.forgetBudget(folder);
      setBudgets((list) => list.filter((b) => b.folder !== folder));
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const create = async () => {
    setBusy('create');
    setError(null);
    try {
      onOpened(await api.create(parent, name, locale, starter));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="welcome">
      <div className="welcome-card">
        <h1>
          <img src="/favicon.svg" alt="" width={32} height={32} /> Budget4
        </h1>
        <p className="muted">
          Opens YNAB 4 budget folders and syncs with YNAB 4 through Dropbox, as another device. Your budget stays in its folder; nothing is uploaded
          anywhere.
        </p>

        {budgets.length > 0 && (
          <>
            <h2>Your budgets</h2>
            <ul className="budget-list">
              {budgets.map((b) => (
                <li key={b.folder} className="budget-row">
                  <button className="budget-item" disabled={!!busy} onClick={() => open(b.folder)}>
                    <span className="budget-name">{b.name}</span>
                    <span className="muted small">{b.folder}</span>
                    {busy === b.folder && <span className="small">Opening…</span>}
                  </button>
                  <button
                    className="link small"
                    title="Takes this budget off this list. Its folder and files are not deleted."
                    disabled={!!busy}
                    onClick={() => forget(b.folder)}
                  >
                    Remove from list
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}

        <h2>Open a budget folder</h2>
        <div className="row">
          <button
            disabled={!!busy}
            onClick={async () => {
              const f = await pickFolder('Choose a YNAB 4 budget folder (ends in .ynab4)');
              if (f) void open(f);
            }}
          >
            Browse…
          </button>
          <span className="muted small">or paste its path:</span>
        </div>
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            if (path.trim()) open(path.trim().replace(/^"|"$/g, ''));
          }}
        >
          <input className="grow" placeholder="C:\Users\you\Dropbox\YNAB\My Budget~1A2B3C4D.ynab4" value={path} onChange={(e) => setPath(e.target.value)} />
          <button className="primary" disabled={!!busy || !path.trim()}>
            Open
          </button>
        </form>

        {!creating && (
          <div className="row">
            <button className="link" onClick={() => setCreating(true)}>
              Create a new budget…
            </button>
            <button
              className="link"
              disabled={!!busy}
              onClick={async () => {
                const file = await pickArchiveFile();
                if (!file) return;
                const parent = await pickFolder('Choose the folder to restore the budget into');
                if (!parent) return;
                setBusy('restore');
                setError(null);
                try {
                  onOpened(await api.restoreArchive(file, parent));
                } catch (e) {
                  setError((e as Error).message);
                } finally {
                  setBusy(null);
                }
              }}
            >
              Restore from an archive…
            </button>
          </div>
        )}
        {creating && (
          <div className="create-box">
            <h2>New budget</h2>
            <label>
              Name
              <input value={name} onChange={(e) => setName(e.target.value)} />
            </label>
            <label>
              Folder to create it in
              <div className="row">
                <input className="grow" value={parent} onChange={(e) => setParent(e.target.value)} placeholder="D:\Dropbox\YNAB" />
                <button
                  type="button"
                  onClick={async () => {
                    const f = await pickFolder('Choose where to create the budget');
                    if (f) setParent(f);
                  }}
                >
                  Browse…
                </button>
              </div>
            </label>
            <label>
              Currency and formats
              <select value={locale} onChange={(e) => setLocale(e.target.value)}>
                {LOCALES.map(([v, l]) => (
                  <option key={v} value={v}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Starter categories
              <select value={starter} onChange={(e) => setStarter(e.target.value as typeof starter)}>
                <option value="personal">Personal: Monthly Bills, Everyday Expenses, Rainy Day Funds… (as YNAB 4)</option>
                <option value="business">Business: Marketing, Services, Office, Travel… (as YNAB 4)</option>
                <option value="none">None: I'll add my own</option>
              </select>
            </label>
            <div className="row end">
              <button onClick={() => setCreating(false)}>Cancel</button>
              <button className="primary" disabled={!!busy || !name.trim() || !parent.trim()} onClick={create}>
                Create budget
              </button>
            </div>
          </div>
        )}

        {error && <p className="error">{error}</p>}
        {info && <p className="muted small">This computer syncs as "{info.friendlyName}". Version {info.version}.</p>}
      </div>
    </div>
  );
}

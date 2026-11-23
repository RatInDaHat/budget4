import { useCallback, useEffect, useState } from 'react';
import { api, type StateDTO } from './api.ts';
import { BudgetView } from './components/BudgetView.tsx';
import { LockScreen } from './components/LockScreen.tsx';
import { OpenBudget } from './components/OpenBudget.tsx';
import { Register } from './components/Register.tsx';
import { ScheduledView } from './components/ScheduledView.tsx';
import { Settings } from './components/Settings.tsx';
import { Sidebar } from './components/Sidebar.tsx';
import { AppProvider, useApp } from './state.tsx';

export function App() {
  const [initial, setInitial] = useState<StateDTO | null>(null);
  const [checked, setChecked] = useState(false);
  const [locked, setLocked] = useState(false);

  const load = useCallback(async () => {
    try {
      if ((await api.lockStatus()).locked) {
        setLocked(true);
        return;
      }
      setLocked(false);
      const info = await api.app();
      if (info.open) setInitial(await api.state());
    } catch {
      // Shows the budget picker.
    } finally {
      setChecked(true);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const onClosed = useCallback(() => setInitial(null), []);

  if (!checked) return <div className="splash">Loading…</div>;
  if (locked) return <LockScreen onUnlocked={() => void load()} />;
  if (!initial) return <OpenBudget onOpened={setInitial} />;
  return (
    <AppProvider initial={initial} onClosed={onClosed} key={initial.budget.folder}>
      <Shell />
    </AppProvider>
  );
}

function Shell() {
  const { view, toast, setToast } = useApp();
  return (
    <div className="shell">
      <Sidebar />
      <main className="main">
        {view.kind === 'budget' && <BudgetView />}
        {view.kind === 'account' && <Register accountId={view.accountId} key={view.accountId ?? 'all'} />}
        {view.kind === 'scheduled' && <ScheduledView />}
        {view.kind === 'settings' && <Settings />}
      </main>
      {toast && (
        <div className="toast" role="alert" onClick={() => setToast(null)}>
          {toast}
        </div>
      )}
    </div>
  );
}

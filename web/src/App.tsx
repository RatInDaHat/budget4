import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { api, fileBase64, type StateDTO } from './api.ts';
import { BudgetView } from './components/BudgetView.tsx';
import { ImportDialog, type ImportSource } from './components/ImportDialog.tsx';
import { LockScreen } from './components/LockScreen.tsx';
import { OpenBudget } from './components/OpenBudget.tsx';
import { PayeesView } from './components/PayeesView.tsx';
import { Register } from './components/Register.tsx';
import { ScheduledView } from './components/ScheduledView.tsx';
import { Settings } from './components/Settings.tsx';
import { Sidebar } from './components/Sidebar.tsx';
import { AppProvider, useApp } from './state.tsx';

const BANK_FILE = /\.(qfx|ofx|qbo|csv)$/i;
/** Window event carrying files dropped onto the window (see App). */
const DROPPED = 'budget4-files';

let nextSourceId = 1;

export function App() {
  const [initial, setInitial] = useState<StateDTO | null>(null);
  const [checked, setChecked] = useState(false);
  const [locked, setLocked] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  // Files dropped anywhere on the window are not opened by the webview itself; the budget screen imports them.
  useEffect(() => {
    const hasFiles = (e: DragEvent) => !!e.dataTransfer && Array.from(e.dataTransfer.types).includes('Files');
    const over = (e: DragEvent) => {
      if (hasFiles(e)) e.preventDefault();
    };
    const drop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      window.dispatchEvent(new CustomEvent(DROPPED, { detail: Array.from(e.dataTransfer?.files ?? []) }));
    };
    window.addEventListener('dragover', over);
    window.addEventListener('drop', drop);
    return () => {
      window.removeEventListener('dragover', over);
      window.removeEventListener('drop', drop);
    };
  }, []);

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

  const onUnlocked = (msg?: string) => {
    if (msg) {
      setNotice(msg);
      window.setTimeout(() => setNotice(null), 12000);
    }
    void load();
  };

  let body;
  if (!checked) body = <div className="splash">Loading…</div>;
  else if (locked) body = <LockScreen onUnlocked={onUnlocked} />;
  else if (!initial) body = <OpenBudget onOpened={setInitial} />;
  else
    body = (
      <AppProvider initial={initial} onClosed={onClosed} key={initial.budget.folder}>
        <Shell />
      </AppProvider>
    );
  return (
    <>
      {body}
      {notice && (
        <div className="toast" role="alert" onClick={() => setNotice(null)}>
          {notice}
        </div>
      )}
    </>
  );
}

function Shell() {
  const { view, toast, setToast } = useApp();
  // Bank files waiting to be imported, one dialog at a time.
  const [queue, setQueue] = useState<ImportSource[]>([]);

  useEffect(() => {
    const addAll = (items: ImportSource[]) => {
      if (items.length) setQueue((q) => [...q, ...items]);
    };
    // Files that came in before a budget was open (or while it was) are waiting on the app.
    const take = () =>
      api
        .incoming()
        .then((paths) => addAll(paths.map((path) => ({ id: nextSourceId++, kind: 'path' as const, path }))))
        .catch(() => {});
    void take();
    const unlisten = listen('incoming', () => void take());

    const onDropped = async (e: Event) => {
      const files = (e as CustomEvent<File[]>).detail;
      const bank = files.filter((f) => BANK_FILE.test(f.name));
      if (bank.length === 0) {
        setToast('Drop a bank file to import it: QFX, OFX, QBO or CSV.');
        return;
      }
      const items: ImportSource[] = [];
      for (const f of bank) items.push({ id: nextSourceId++, kind: 'data', name: f.name, data: await fileBase64(f) });
      addAll(items);
    };
    window.addEventListener(DROPPED, onDropped);
    return () => {
      window.removeEventListener(DROPPED, onDropped);
      void unlisten.then((off) => off());
    };
  }, [setToast]);

  const next = queue[0];
  return (
    <div className="shell">
      <Sidebar />
      <main className="main">
        {view.kind === 'budget' && <BudgetView />}
        {view.kind === 'account' && <Register accountId={view.accountId} key={view.accountId ?? 'all'} />}
        {view.kind === 'scheduled' && <ScheduledView />}
        {view.kind === 'payees' && <PayeesView />}
        {view.kind === 'settings' && <Settings />}
      </main>
      {next && (
        <ImportDialog
          key={next.id}
          defaultAccountId={null}
          source={next}
          onClose={() => setQueue((q) => q.filter((s) => s.id !== next.id))}
        />
      )}
      {toast && (
        <div className="toast" role="alert" onClick={() => setToast(null)}>
          {toast}
        </div>
      )}
    </div>
  );
}

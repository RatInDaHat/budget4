import { useEffect, useState } from 'react';
import { api, pickArchiveSave, type AppInfo } from '../api.ts';
import { relativeTime } from '../format.ts';
import { useApp } from '../state.tsx';
import { PinSettings } from './LockScreen.tsx';

export function Settings() {
  const { state, run, version, setToast } = useApp();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const [name, setName] = useState('');

  useEffect(() => {
    api.app().then((i) => {
      setInfo(i);
      setName(i.friendlyName);
    });
  }, []);
  useEffect(() => {
    api.log().then(setLog).catch(() => {});
  }, [version]);

  if (!state || !info) return null;
  const s = state.sync;

  const save = async (patch: Parameters<typeof api.settings>[0]) => {
    await run(() => api.settings(patch));
    setInfo(await api.app());
  };

  return (
    <div className="settings">
      <header className="toolbar">
        <h2>Settings</h2>
      </header>
      <section>
        <h3>Sync</h3>
        <p>
          This computer is device <b>{state.budget.device.id}</b> in this budget. Changes are written as YNAB 4 change files in the budget's Dropbox folder, so
          YNAB 4 on other computers picks them up, and their changes appear here within a few seconds.
        </p>
        <table className="table compact">
          <tbody>
            <tr>
              <td>Budget folder</td>
              <td className="mono">{state.budget.folder}</td>
            </tr>
            <tr>
              <td>Last sync</td>
              <td>{relativeTime(s.lastSync)}</td>
            </tr>
            <tr>
              <td>Last full save</td>
              <td>{relativeTime(s.lastFullSave)}</td>
            </tr>
            <tr>
              <td>Change files waiting</td>
              <td>{s.blockedDiffs ? `${s.blockedDiffs} (Dropbox may still be downloading them)` : 'none'}</td>
            </tr>
            <tr>
              <td>Conflicts resolved</td>
              <td>{s.conflictsResolved}</td>
            </tr>
            {s.lastError && (
              <tr>
                <td>Last error</td>
                <td className="error">{s.lastError}</td>
              </tr>
            )}
          </tbody>
        </table>
        <h4>Other devices</h4>
        <ul>
          {state.budget.otherDevices.map((d) => (
            <li key={d.id}>
              <b>{d.id}</b> · {d.name} <span className="muted small">{d.version}</span>
            </li>
          ))}
        </ul>
        <div className="row">
          <button onClick={() => run(() => api.sync())}>Sync now</button>
          <button onClick={() => run(() => api.save())}>Save full file now</button>
          <button
            onClick={async () => {
              const r = await run(() => api.backup());
              if (r) setToast(`Backup written: ${r.file}`);
            }}
          >
            Write a backup
          </button>
          <button
            onClick={async () => {
              const today = new Date().toLocaleDateString('en-CA');
              const safe = state.budget.name.replace(/[\/:*?"<>|]/g, '-');
              const dest = await pickArchiveSave(`${safe} ${today}.zip`);
              if (!dest) return;
              const r = await run(() => api.archive(dest));
              if (r) setToast(`Archived to ${r.file}`);
            }}
          >
            Archive budget…
          </button>
        </div>
      </section>

      <section>
        <h3>This computer</h3>
        <label>
          Device name (shown to other devices)
          <div className="row">
            <input value={name} onChange={(e) => setName(e.target.value)} />
            <button onClick={() => save({ friendlyName: name })} disabled={!name.trim() || name === info.friendlyName}>
              Save
            </button>
          </div>
        </label>
        <label className="check">
          <input type="checkbox" checked={info.enterScheduledTransactions} onChange={(e) => save({ enterScheduledTransactions: e.target.checked })} />
          Enter due scheduled transactions automatically (as YNAB 4 does)
        </label>
        <label className="check">
          <input type="checkbox" checked={info.backupOnFirstSave} onChange={(e) => save({ backupOnFirstSave: e.target.checked })} />
          Write a .y4backup the first time each session saves
        </label>
        <p className="muted small">Device name and the scheduled-transaction setting apply the next time a budget is opened.</p>
      </section>

      <PinSettings setToast={setToast} />

      <section>
        <h3>Activity</h3>
        <pre className="log">{log.slice().reverse().join('\n') || 'Nothing yet.'}</pre>
      </section>
    </div>
  );
}
